//! APFS fast path for [`crate::catalog::snapshot::volume_snapshot::build_volume_snapshot`].
//!
//! The generic per-directory walker is pathological on APFS: every directory
//! listing resolves its full path from the volume root (one catalog B-tree
//! descent per path component) and then pays one more descent *per child* to
//! join the inode record for sizes/timestamps — each descent resolving every
//! tree-node hop through the volume OMAP, each block read going through the
//! E01 decompressor. A whole-volume walk multiplies all of that by the number
//! of directories.
//!
//! This module mirrors the NTFS `$MFT` fast path instead: per volume, one
//! streaming pass over the whole catalog B-tree (the OMAP preloaded into a
//! map) collects every inode and directory record, then the tree is rebuilt
//! in memory and streamed to the `.dirz` file. Output is identical to what
//! the generic walker records — same node ids, parent keys, paths, ordering
//! rules and timestamps — so snapshot consumers can't tell the difference.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;
use apfs::catalog::{DT_DIR, DT_LNK, ROOT_DIR_RECORD};
use apfs::CatalogRecord;
use tokio_util::sync::CancellationToken;

use crate::catalog::snapshot::volume_snapshot::{
    snapshot_file_name, volume_size_for_key, write_snapshot_entry, write_snapshot_header,
    SnapshotEntry, SnapshotHeader, VolumeSnapshotBuilt, PROGRESS_EVERY, SNAPSHOT_SCHEMA,
};
use crate::evidence::image::{Partition, PartitionKind};
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{fs_unalloc_nodes, FsNode, Mactb, NodeKind};
use crate::filesystem::volumes::apfs::{
    apfs_node_id, apfs_timestamps, apfs_volume_nodes, open_apfs_source, parse_apfs_id,
};

/// The partition behind `volume_key`, when that key names an APFS container
/// (`part:N` only — APFS never sits inside LVM here). `None` sends the caller
/// down the NTFS fast path / generic walker exactly as before.
pub(crate) fn apfs_container_for_key(
    image: &EvidenceSession,
    volume_key: &str,
) -> Option<Partition> {
    let part_id = volume_key.strip_prefix("part:")?.parse::<u8>().ok()?;
    image
        .partitions
        .iter()
        .find(|partition| partition.id == part_id && partition.kind == PartitionKind::ApfsContainer)
        .cloned()
}

/// Inode fields the snapshot needs, kept compact because the map holds one
/// entry per file on the volume.
struct InodeMeta {
    size: u64,
    needs_logical_metadata: bool,
    link_target: Option<String>,
    create_time: i64,
    modify_time: i64,
    change_time: i64,
    access_time: i64,
}

/// One directory record: a name inside its parent, pointing at the child OID.
struct DirentEdge {
    name: String,
    file_id: u64,
    kind: NodeKind,
}

/// Build a `.dirz` snapshot of an APFS container with one catalog scan per
/// volume. Same contract as the generic builder: cancellation finalises and
/// removes the partial file and bails with "snapshot build cancelled"; a
/// volume that fails to open or scan is logged and skipped (its subtree
/// stays uncovered, so browsing falls back to live listings); `max_entries`
/// flips `truncated`.
pub(crate) fn build_apfs_volume_snapshot(
    image: &Arc<EvidenceSession>,
    volume_key: &str,
    out_path: &Path,
    max_entries: usize,
    built_at: String,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let partition = apfs_container_for_key(image, volume_key)
        .ok_or_else(|| anyhow!("not an APFS container key: {volume_key}"))?;

    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image.id.clone(),
        source_volume_size: volume_size_for_key(image, volume_key),
        built_at,
    };

    let output = crate::catalog::snapshot::volume_snapshot::PartialOutput::new(out_path);
    let file = File::create(out_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    write_snapshot_header(&mut encoder, &header)?;
    let mut scratch: Vec<u8> = Vec::new();

    // The container root listing the generic walker records: one node per
    // APFS volume, plus the whole-volume "$未分配簇" leaf (APFS has no
    // recovery container). Volume nodes are recorded first, then descended.
    let mut root_nodes = apfs_volume_nodes(Arc::clone(image), &partition)?;
    root_nodes.extend(fs_unalloc_nodes(image, &partition));

    let mut entry_count: u64 = 0;
    let mut truncated = false;

    'volumes: for root_node in &root_nodes {
        if cancel.is_cancelled() {
            anyhow::bail!("snapshot build cancelled");
        }
        if entry_count as usize >= max_entries {
            truncated = true;
            break;
        }
        let root_path = format!("/{}", root_node.name);
        let line = SnapshotEntry {
            parent: volume_key.to_string(),
            node: root_node.clone(),
            path: root_path.clone(),
        };
        write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
        entry_count += 1;
        if entry_count.is_multiple_of(PROGRESS_EVERY) {
            on_progress(entry_count);
        }
        if root_node.kind != NodeKind::Dir || root_node.deleted {
            continue;
        }

        // A volume node. Its id parses back to the (partition, volume_index)
        // source the live dispatcher opens.
        let Some((source, _)) = parse_apfs_id(&root_node.id) else {
            continue;
        };
        let volume = match open_apfs_source(image, source) {
            Ok(volume) => volume,
            Err(err) => {
                tracing::warn!("snapshot: APFS open {} failed: {err}", root_node.id);
                continue;
            }
        };

        // One streaming pass over the catalog: inode metadata by OID, and
        // directory edges grouped by parent OID. Cancellation is polled by
        // record count; `false` from the visitor stops the walk cleanly.
        let mut inodes: HashMap<u64, InodeMeta> = HashMap::new();
        let mut children: HashMap<u64, Vec<DirentEdge>> = HashMap::new();
        let mut scanned: u64 = 0;
        let scan = volume.walk_catalog(&mut |record| {
            scanned += 1;
            if scanned.is_multiple_of(PROGRESS_EVERY) {
                on_progress(entry_count);
                if cancel.is_cancelled() {
                    return false;
                }
            }
            match record {
                CatalogRecord::Inode { oid, inode } => {
                    inodes.insert(
                        oid,
                        InodeMeta {
                            size: inode.size(),
                            needs_logical_metadata: inode.bsd_flags & 0x20 != 0
                                || inode.kind() == apfs::catalog::INODE_SYMLINK_TYPE,
                            link_target: None,
                            create_time: inode.create_time,
                            modify_time: inode.modify_time,
                            change_time: inode.change_time,
                            access_time: inode.access_time,
                        },
                    );
                }
                CatalogRecord::DirEntry {
                    parent_oid,
                    name,
                    drec,
                } => {
                    let kind = match drec.file_type() {
                        DT_DIR => NodeKind::Dir,
                        DT_LNK => NodeKind::Symlink,
                        _ => NodeKind::File,
                    };
                    children.entry(parent_oid).or_default().push(DirentEdge {
                        name,
                        file_id: drec.file_id,
                        kind,
                    });
                }
            }
            true
        });
        match scan {
            Ok(true) => {}
            Ok(false) => {
                anyhow::bail!("snapshot build cancelled");
            }
            Err(err) => {
                // One unreadable volume shouldn't abort the container snapshot
                // — its subtree stays uncovered and browsing falls back live,
                // matching the generic walker's per-subtree lenience.
                tracing::warn!("snapshot: APFS catalog scan {} failed: {err}", root_node.id);
                continue;
            }
        }

        for (oid, metadata) in &mut inodes {
            if cancel.is_cancelled() {
                anyhow::bail!("snapshot build cancelled");
            }
            if metadata.needs_logical_metadata {
                let (size, target) = volume.catalog_logical_metadata(*oid)?;
                metadata.size = size;
                metadata.link_target = target;
            }
        }

        // Each listing in live order: directories first, then case-insensitive
        // name — the exact comparator in `ApfsVolume::children`.
        for edges in children.values_mut() {
            edges.sort_by(|a, b| {
                let ak = u8::from(a.kind != NodeKind::Dir);
                let bk = u8::from(b.kind != NodeKind::Dir);
                ak.cmp(&bk)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }

        let (part_id, volume_index) = match source {
            crate::filesystem::volumes::apfs::ApfsSource::Partition {
                part_id,
                volume_index,
            } => (part_id, volume_index),
            crate::filesystem::volumes::apfs::ApfsSource::LegacyPartition(part_id) => (part_id, 0),
        };

        // Explicit-stack DFS from the volume root OID, rebuilding volume-
        // relative paths (node ids hex-encode them) and full snapshot paths
        // (prefixed with the volume node's name). `visited` guards OID cycles
        // from corrupt parent references.
        let mut stack: Vec<(u64, String, String)> =
            vec![(ROOT_DIR_RECORD, String::new(), root_node.id.clone())];
        let mut visited: HashSet<u64> = HashSet::new();
        visited.insert(ROOT_DIR_RECORD);

        while let Some((dir_oid, dir_rel_path, parent_node_id)) = stack.pop() {
            if cancel.is_cancelled() {
                anyhow::bail!("snapshot build cancelled");
            }
            let Some(edges) = children.get(&dir_oid) else {
                continue;
            };
            for edge in edges {
                if entry_count as usize >= max_entries {
                    truncated = true;
                    break 'volumes;
                }
                let rel_path = if dir_rel_path.is_empty() {
                    format!("/{}", edge.name)
                } else {
                    format!("{dir_rel_path}/{}", edge.name)
                };
                let node_id = apfs_node_id(part_id, volume_index, &rel_path);
                let meta = inodes.get(&edge.file_id);
                let timestamps = meta.map_or_else(Mactb::default, |meta| {
                    apfs_timestamps(
                        meta.create_time,
                        meta.modify_time,
                        meta.change_time,
                        meta.access_time,
                    )
                });
                let size = if edge.kind == NodeKind::Dir {
                    0
                } else {
                    meta.map_or(0, |meta| meta.size)
                };
                let node = FsNode {
                    id: node_id.clone(),
                    name: edge.name.clone(),
                    parent_id: Some(parent_node_id.clone()),
                    kind: edge.kind,
                    size,
                    deleted: false,
                    inode: Some(edge.file_id),
                    timestamps,
                    link_target: meta.and_then(|metadata| metadata.link_target.clone()),
                    has_snapshot: false,
                };
                let line = SnapshotEntry {
                    parent: parent_node_id.clone(),
                    node,
                    path: format!("{root_path}{rel_path}"),
                };
                write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
                entry_count += 1;
                if entry_count.is_multiple_of(PROGRESS_EVERY) {
                    on_progress(entry_count);
                }
                if edge.kind == NodeKind::Dir && visited.insert(edge.file_id) {
                    stack.push((edge.file_id, rel_path, node_id));
                }
            }
        }
    }

    encoder.finish()?;
    output.keep();
    on_progress(entry_count);

    let file_name = out_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| snapshot_file_name(&header.image_id, volume_key));

    Ok(VolumeSnapshotBuilt {
        header,
        file_name,
        entry_count,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::snapshot::volume_snapshot::load_snapshot;
    use crate::filesystem::tree::fs_tree::children_for_image;

    #[test]
    fn apfs_synthetic_snapshot_matches_live_compressed_and_symlink_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.apfs");
        std::fs::write(
            &path,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../vendor/apfs/tests/data/basic.apfs"
            )),
        )
        .unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::ApfsContainer)
            .unwrap();
        let key = format!("part:{}", partition.id);
        let output = directory.path().join("snapshot.dirz");
        let cancel = CancellationToken::new();
        build_apfs_volume_snapshot(
            &image,
            &key,
            &output,
            100,
            "2026-10-13T00:00:00Z".into(),
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        let snapshot = load_snapshot(&output).unwrap();
        let root = apfs_node_id(partition.id, 0, "");
        let live = children_for_image(&image, &root).unwrap();
        let stored = snapshot.children(&root).unwrap();
        assert_eq!(
            serde_json::to_value(stored).unwrap(),
            serde_json::to_value(&live).unwrap()
        );
        assert_eq!(
            live.iter()
                .find(|node| node.name == "compressed.txt")
                .unwrap()
                .size,
            16
        );
        assert_eq!(
            live.iter()
                .find(|node| node.name == "link")
                .unwrap()
                .link_target
                .as_deref(),
            Some("../plain.bin")
        );
        cancel.cancel();
        let cancelled_output = directory.path().join("cancelled.dirz");
        assert!(build_apfs_volume_snapshot(
            &image,
            &key,
            &cancelled_output,
            100,
            "2026-10-13T00:00:00Z".into(),
            &cancel,
            &mut |_| {}
        )
        .is_err());
        assert!(!cancelled_output.exists());
    }

    /// Env-gated equivalence guard: `LOVELYCARVE_APFS_SNAPSHOT_TEST` points at
    /// an image with an APFS container. Builds a fast-path snapshot and checks
    /// that, for the container root and a bounded breadth-first sample of
    /// directories, the snapshot listing is JSON-identical to the live
    /// `children_for_image` listing — the exact property `fs_children` relies
    /// on when serving from a snapshot.
    #[test]
    fn fast_path_matches_live_listings_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_APFS_SNAPSHOT_TEST") else {
            return;
        };
        let image = Arc::new(EvidenceSession::open(path).expect("APFS test image should open"));
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::ApfsContainer)
            .cloned()
            .expect("expected an APFS container partition");
        let volume_key = format!("part:{}", partition.id);

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("apfs.dirz");
        let cancel = CancellationToken::new();
        let built = build_apfs_volume_snapshot(
            &image,
            &volume_key,
            &out,
            crate::catalog::snapshot::volume_snapshot::DEFAULT_MAX_SNAPSHOT_ENTRIES,
            "2026-06-12T00:00:00Z".to_string(),
            &cancel,
            &mut |_| {},
        )
        .expect("fast path build should succeed");
        assert!(built.entry_count > 0);
        let snapshot = load_snapshot(&out).expect("snapshot should load");

        // Breadth-first sample: the root plus the first directories discovered,
        // bounded so the live side stays fast on a big volume.
        const MAX_DIRS: usize = 40;
        let mut queue: Vec<String> = vec![volume_key.clone()];
        let mut compared = 0usize;
        while let Some(parent_id) = queue.pop() {
            if compared >= MAX_DIRS {
                break;
            }
            compared += 1;
            let live = children_for_image(&image, &parent_id).expect("live listing");
            // A genuinely empty directory records no entries, so the snapshot
            // deliberately reports "uncovered" and the caller goes live.
            let Some(from_snapshot) = snapshot.children(&parent_id) else {
                assert!(
                    live.is_empty(),
                    "snapshot should cover non-empty directory {parent_id}"
                );
                continue;
            };
            assert_eq!(
                serde_json::to_value(&from_snapshot).unwrap(),
                serde_json::to_value(&live).unwrap(),
                "snapshot listing for {parent_id} diverges from live"
            );
            for node in live {
                if node.kind == NodeKind::Dir && !node.deleted {
                    queue.push(node.id);
                }
            }
        }
        assert!(compared > 1, "sample should descend past the root");
    }
}
