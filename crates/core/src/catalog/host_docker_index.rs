//! Case-owned directory and source indexes for individual host Docker containers.
//! Only an explicit prepare builds an index; cold reads can restore source
//! mappings without walking OverlayFS again. Incomplete writes are never ready.
//!
//! The source mapping lives in the evidence cache (`evidence/<uid>/cache/docker`).
//! The container's directory snapshot and its file index are built in the case
//! staging directory outside the case lock and published through [`CaseState`]
//! like any other volume ([`publish`]).

use crate::filesystem::docker::mapping_cache::{load_mapping, mapping_path, save_mapping};

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;

use tempfile::NamedTempFile;

use crate::casework::case::{snapshot_publication, CaseState, SnapshotPublish};
use crate::casework::case_ledger_volumes::{SnapshotPublication, VolumeRecord};
use crate::catalog::filedb::filedb_store::{self, IndexSource};
use crate::filesystem::docker::host_docker::{self, HostDockerNodeId};
use crate::filesystem::docker::host_docker_overlay::MergedContainerFs;
use crate::filesystem::evidence_session::EvidenceSession;
use crate::{casework::case_paths, catalog::snapshot::volume_snapshot, filesystem::fs_all};
use volume_snapshot::{SnapshotHeader, VolumeSnapshotBuilt, SNAPSHOT_SCHEMA};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrepareMode {
    Status,
    Build,
}

/// The container volume as currently published in the case.
#[derive(Debug, Clone)]
pub struct PublishedVolume {
    pub record: VolumeRecord,
    pub snapshot_path: PathBuf,
    pub index_path: Option<PathBuf>,
}

/// The case side of a prepare, captured under the case lock so that the build
/// itself runs outside it.
#[derive(Debug, Clone)]
pub struct CaseTarget {
    /// Source mapping cache, `evidence/<uid>/cache/docker`.
    pub cache_dir: PathBuf,
    /// Where a new snapshot and index are built before publication.
    pub staging_dir: PathBuf,
    pub published: Option<PublishedVolume>,
}

impl CaseTarget {
    pub fn of(case: &CaseState, image_id: &str, root: &str) -> Self {
        let published = case.volume(image_id, root).map(|record| PublishedVolume {
            snapshot_path: case.snapshot_path(record),
            index_path: case.index_path(record),
            record: record.clone(),
        });
        Self {
            cache_dir: cache_directory(&case.dir, image_id),
            staging_dir: case.staging_dir(),
            published,
        }
    }
}

/// A freshly built snapshot and index in the case staging directory, waiting
/// for [`publish`]. Dropping it unpublished removes both files.
#[derive(Debug)]
pub struct StagedContainer {
    snapshot: PathBuf,
    index: PathBuf,
    publication: SnapshotPublication,
}

impl Drop for StagedContainer {
    fn drop(&mut self) {
        // Published files have been moved away; only leftovers remain here.
        for path in [&self.snapshot, &self.index] {
            if let Err(error) = case_paths::remove_database_files(path) {
                tracing::debug!(
                    "Docker staging cleanup of {} deferred: {error}",
                    path.display()
                );
            }
        }
    }
}

pub struct PreparedContainer {
    pub ready: bool,
    pub entry_count: u64,
    /// The case already serves a current snapshot and index of the container.
    pub persisted: bool,
    /// A new snapshot and index to [`publish`].
    pub staged: Option<StagedContainer>,
}

pub fn cache_directory(case_dir: &Path, image_id: &str) -> PathBuf {
    case_paths::cache_dir(case_dir, image_id).join("docker")
}

/// The published volume still describes this container: a complete snapshot
/// of the same evidence with a matching entry count, and an index imported
/// from that snapshot.
fn published_is_current(
    published: &PublishedVolume,
    image: &EvidenceSession,
    root: &str,
    vol_id: i64,
    entry_count: u64,
) -> bool {
    let record = &published.record;
    if record.truncated
        || record.entry_count != entry_count
        || record.vol_id != vol_id
        || record.source_size != image.len()
        || record.fingerprint != image.evidence_fingerprint()
        || !record.index_is_current()
        || !published
            .index_path
            .as_ref()
            .is_some_and(|index_path| index_path.is_file())
    {
        return false;
    }
    match volume_snapshot::load_snapshot_header(&published.snapshot_path) {
        Ok(header) => {
            header.schema_version == SNAPSHOT_SCHEMA
                && header.image_id == image.id
                && header.volume_key == root
                && header.source_volume_size == image.len()
        }
        Err(error) => {
            tracing::warn!("Docker directory snapshot for {root} will be rebuilt: {error:#}");
            false
        }
    }
}

/// Synchronous worker, serialized per container. A status probe never composes
/// layers. The mapping and a current published snapshot and index must all
/// exist before reuse; a failed build can reuse its already saved mapping on
/// retry.
pub fn prepare(
    image: &Arc<EvidenceSession>,
    root: &str,
    case: Option<&CaseTarget>,
    mode: PrepareMode,
    built_at: String,
) -> Result<PreparedContainer> {
    let Some(HostDockerNodeId::Container { container_id, path }) =
        host_docker::parse_host_docker_id(root)
    else {
        return Err(anyhow!("请选择一个 Docker 容器"));
    };
    ensure!(path.is_empty(), "Docker 索引必须从容器根目录开始");
    image.validate_source_file_stable()?;
    let catalog = host_docker::catalog(image).ok_or_else(|| anyhow!("未找到 Docker 容器"))?;
    let container = catalog.container(&container_id)?;
    let _guard = container.index_lock.lock();
    let mut merged = container.merged.lock().clone();
    let mut saved_mapping = false;
    if let Some(case) = case {
        // Cold loads validate the serialized mapping; warm loads retain the
        // immutable image's verified mapping, but still detect a removed file.
        saved_mapping = mapping_path(&case.cache_dir, &container_id).is_file();
        if merged.is_none() {
            merged = load_mapping(&case.cache_dir, image, &container_id).map(Arc::new);
            saved_mapping = merged.is_some();
            if let Some(merged) = merged.as_ref() {
                *container.merged.lock() = Some(Arc::clone(merged));
            }
        }
    }
    let vol_id = fs_all::vol_id_for_key(image, root)?;
    if let Some(tree) = merged.as_ref() {
        let entry_count = tree.entry_count();
        let Some(case) = case else {
            return Ok(PreparedContainer {
                ready: true,
                entry_count,
                persisted: false,
                staged: None,
            });
        };
        let reusable = saved_mapping
            && case.published.as_ref().is_some_and(|published| {
                published_is_current(published, image, root, vol_id, entry_count)
            });
        if reusable {
            return Ok(PreparedContainer {
                ready: true,
                entry_count,
                persisted: true,
                staged: None,
            });
        }
    }
    if matches!(mode, PrepareMode::Status) {
        return Ok(PreparedContainer {
            ready: false,
            entry_count: 0,
            persisted: false,
            staged: None,
        });
    }
    let tree = container.merged(image)?;
    let Some(case) = case else {
        return Ok(PreparedContainer {
            ready: true,
            entry_count: tree.entry_count(),
            persisted: false,
            staged: None,
        });
    };
    save_mapping(
        &case.cache_dir,
        &image.id,
        &container_id,
        &image.evidence_fingerprint(),
        &tree,
    )
    .context("保存 Docker 文件来源索引失败")?;
    let staged = stage_volume(image, root, &container_id, vol_id, &tree, case, built_at)?;
    Ok(PreparedContainer {
        ready: true,
        entry_count: tree.entry_count(),
        persisted: false,
        staged: Some(staged),
    })
}

/// Writes the container's directory snapshot into the staging directory and
/// imports its file index next to it.
fn stage_volume(
    image: &EvidenceSession,
    root: &str,
    container_id: &str,
    vol_id: i64,
    tree: &MergedContainerFs,
    case: &CaseTarget,
    built_at: String,
) -> Result<StagedContainer> {
    std::fs::create_dir_all(&case.staging_dir)?;
    let mut temporary = NamedTempFile::new_in(&case.staging_dir)?;
    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: root.to_owned(),
        image_id: image.id.clone(),
        source_volume_size: image.len(),
        built_at,
    };
    let mut encoder =
        zstd::stream::write::Encoder::new(BufWriter::new(temporary.as_file_mut()), 3)?;
    volume_snapshot::write_snapshot_header(&mut encoder, &header)?;
    let mut scratch = Vec::new();
    for (parent, path, node) in tree.walk_entries(container_id) {
        let entry = volume_snapshot::SnapshotEntry { parent, node, path };
        volume_snapshot::write_snapshot_entry(&mut encoder, &entry, &mut scratch)?;
    }
    encoder.finish()?.flush()?;
    temporary.as_file().sync_all()?;
    image.validate_source_file_stable()?;
    let file_name = format!(
        "{}-{}.dirz",
        case_paths::volume_segment(root),
        case_paths::new_token()
    );
    let snapshot = case.staging_dir.join(&file_name);
    temporary.persist(&snapshot)?;
    let fingerprint = image.evidence_fingerprint();
    let built = VolumeSnapshotBuilt {
        header,
        file_name,
        entry_count: tree.entry_count(),
        truncated: false,
    };
    let staged = StagedContainer {
        index: filedb_store::staged_index_path(&case.staging_dir, root),
        publication: snapshot_publication(vol_id, &built, &fingerprint),
        snapshot,
    };
    filedb_store::build_index(
        &staged.snapshot,
        &staged.index,
        &IndexSource {
            image_id: &image.id,
            volume_key: root,
            vol_id,
            built_at: &built.header.built_at,
            fingerprint: &fingerprint,
            truncated: false,
        },
        None,
        None,
    )
    .context("保存 Docker 查询索引失败")?;
    Ok(staged)
}

/// Publishes a staged container snapshot and its index. Returns `false` when
/// the case already holds a newer snapshot or the evidence left the case; the
/// staged files are removed either way.
pub fn publish(case: &mut CaseState, staged: StagedContainer) -> Result<bool> {
    match case.publish_snapshot(&staged.snapshot, staged.publication.clone())? {
        SnapshotPublish::Published(record) => case.publish_index(
            &record.image_id,
            &record.volume_key,
            &record.built_at,
            &staged.index,
        ),
        SnapshotPublish::Discarded => Ok(false),
    }
}
