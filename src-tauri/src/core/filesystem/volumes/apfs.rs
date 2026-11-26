//! Read-only APFS volume adapter built on the vendored `apfs` crate.
//!
//! The parsed `apfs::ApfsVolume` (one selected volume inside the container,
//! including the checkpoint scan and dual OMAP B-tree lookups) is built once in
//! [`ApfsVolume::open`] and reused for every operation behind a `Mutex` (the
//! crate's reads take `&mut`). That open is expensive enough that a whole-
//! volume snapshot walk would otherwise pay it per directory; holding the
//! handle (and caching the `ApfsVolume` in
//! [`crate::core::filesystem::tree::fs_tree::FsSession`]) keeps it to one parse per walk.
//! Live IPC calls still open their own `ApfsVolume`, so the mutex is
//! uncontended outside the single-threaded walker.
//!
//! The APFS partition node lists each filesystem volume. Node IDs then encode
//! the volume slot plus absolute UTF-8 path as hex bytes:
//! `apfs:part:{N}:{volume_index}:{hex(/path/to/entry)}`. The empty-hex form is
//! the selected volume root. Historical sources append @<XID> to the slot.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use apfs::{ApfsVolume as ContainerVolume, EntryKind, FusionTier, VolumeInfo};
use chrono::{TimeZone, Utc};

use crate::core::common::error::Failure;
use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition, PartitionKind};
use crate::core::evidence::partitions::apfs::{open_members, PartitionCursor};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum ApfsSource {
    Partition {
        part_id: u8,
        volume_index: usize,
        snapshot_xid: Option<u64>,
    },
    LegacyPartition(u8),
}

impl ApfsSource {
    pub fn part_id(self) -> u8 {
        match self {
            ApfsSource::Partition { part_id, .. } | ApfsSource::LegacyPartition(part_id) => part_id,
        }
    }

    pub(crate) fn cache_key(self) -> String {
        match self {
            ApfsSource::Partition {
                part_id,
                volume_index,
                snapshot_xid: None,
            } => format!("apfs:part:{part_id}:{volume_index}"),
            ApfsSource::Partition {
                part_id,
                volume_index,
                snapshot_xid: Some(xid),
            } => format!("apfs:part:{part_id}:{volume_index}@{xid}"),
            ApfsSource::LegacyPartition(part_id) => format!("apfs:part:{part_id}:legacy"),
        }
    }
}

/// Parse an `apfs:part:{N}:{volume_index}:{hex(path)}` ID. Returns the source
/// plus the decoded UTF-8 path (with a leading `/`, or empty for the volume
/// root). The legacy `apfs:part:{N}:{hex(path)}` form is still accepted for
/// existing indexes and bookmarks, and maps to the container's first volume.
pub fn parse_apfs_id(id: &str) -> Option<(ApfsSource, String)> {
    let mut parts = id.splitn(5, ':');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("apfs"), Some("part"), Some(part_id), Some(volume_index), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let (volume_index, snapshot_xid) = match volume_index.split_once('@') {
                Some((index, xid)) => (
                    index.parse().ok()?,
                    Some(xid.parse::<u64>().ok().filter(|xid| *xid > 0)?),
                ),
                None => (volume_index.parse().ok()?, None),
            };
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((
                ApfsSource::Partition {
                    part_id,
                    volume_index,
                    snapshot_xid,
                },
                path,
            ))
        }
        (Some("apfs"), Some("part"), Some(part_id), Some(hex_path), None) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((ApfsSource::LegacyPartition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_apfs_source(image: &Arc<DiskImage>, source: ApfsSource) -> Result<ApfsVolume> {
    match source {
        ApfsSource::Partition {
            part_id,
            volume_index,
            snapshot_xid,
        } => {
            let partition = find_partition(image, &part_id.to_string())?;
            ApfsVolume::open_with(
                Arc::clone(image),
                partition,
                Some(volume_index),
                snapshot_xid,
            )
        }
        ApfsSource::LegacyPartition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            ApfsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub fn apfs_volume_nodes(image: Arc<DiskImage>, partition: &Partition) -> Result<Vec<FsNode>> {
    let infos = list_apfs_volumes(Arc::clone(&image), partition)?;
    let parent_id = format!("part:{}", partition.id);
    Ok(infos
        .into_iter()
        .map(|info| apfs_volume_node(partition.id, &parent_id, info))
        .collect())
}

/// APFS snapshots are separate browse roots. The current-volume index is
/// also usable without source bytes, so only live browsing adds these roots.
pub(crate) fn has_live_apfs_roots(image: &DiskImage, parent_id: &str) -> bool {
    !image.is_index_only()
        && parent_id
            .strip_prefix("part:")
            .and_then(|id| id.parse::<u8>().ok())
            .is_some_and(|id| {
                image.partitions.iter().any(|partition| {
                    partition.id == id && partition.kind == PartitionKind::ApfsContainer
                })
            })
}

pub(crate) fn apfs_browse_nodes(
    image: Arc<DiskImage>,
    partition: &Partition,
) -> Result<Vec<FsNode>> {
    let parent_id = format!("part:{}", partition.id);
    let infos = list_apfs_volumes(Arc::clone(&image), partition)?;
    let mut nodes = Vec::new();
    for info in infos {
        let volume_index = info.index;
        let can_read_snapshots = !info.encrypted && info.num_snapshots > 0;
        nodes.push(apfs_volume_node(partition.id, &parent_id, info));
        if !can_read_snapshots {
            continue;
        }
        let snapshots = ApfsVolume::open_index(Arc::clone(&image), partition, volume_index)
            .and_then(|volume| volume.with_volume(|reader| Ok(reader.list_snapshots()?)));
        match snapshots {
            Ok(snapshots) => {
                for snapshot in snapshots {
                    let xid = snapshot.transaction_id;
                    let source = ApfsSource::Partition {
                        part_id: partition.id,
                        volume_index,
                        snapshot_xid: Some(xid),
                    };
                    let name = format!("APFS_{volume_index}_snapshot_{xid}_{}", snapshot.name);
                    nodes.push(FsNode {
                        id: format!("{}:", source.cache_key()),
                        name: crate::core::common::navigation_name::volume_name(&name),
                        parent_id: Some(parent_id.clone()),
                        kind: NodeKind::Dir,
                        size: 0,
                        deleted: false,
                        inode: None,
                        timestamps: apfs_timestamps(
                            snapshot.create_time,
                            0,
                            snapshot.change_time,
                            0,
                        ),
                        link_target: None,
                        has_snapshot: false,
                    });
                }
            }
            Err(error) => tracing::warn!(
                partition = partition.id, volume_index, %error, "APFS snapshot discovery failed"
            ),
        }
    }
    Ok(nodes)
}

pub struct ApfsVolume {
    partition_id: u8,
    volume_index: usize,
    snapshot_xid: Option<u64>,
    base_offset: u64,
    tier2_base: Option<u64>,
    /// Parsed selected APFS volume, built once in [`ApfsVolume::open`] and
    /// reused behind a mutex (the `apfs` crate's reads take `&mut`). See the
    /// module header for why it is held rather than rebuilt per call.
    volume: Mutex<ContainerVolume<PartitionCursor>>,
}

impl ApfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        Self::open_with(image, partition, None, None)
    }

    pub fn open_index(
        image: Arc<DiskImage>,
        partition: &Partition,
        volume_index: usize,
    ) -> Result<Self> {
        Self::open_with(image, partition, Some(volume_index), None)
    }

    fn open_with(
        image: Arc<DiskImage>,
        partition: &Partition,
        volume_index: Option<usize>,
        snapshot_xid: Option<u64>,
    ) -> Result<Self> {
        let (cursor, tier2) = open_members(image, partition)?;
        let base_offset = cursor.base_offset;
        let tier2_base = tier2.as_ref().map(|cursor| cursor.base_offset);
        let volume = ContainerVolume::open_sources(cursor, tier2, volume_index)
            .map_err(|err| anyhow!("APFS open partition {}: {err}", partition.id))?;
        let volume = if let Some(xid) = snapshot_xid {
            volume.into_snapshot(xid).map_err(|error| match error {
                apfs::ApfsError::FileNotFound(message) => anyhow!(Failure::not_found(message)),
                other => anyhow!(other),
            })?
        } else {
            volume
        };
        let volume_index = volume.volume_info().index;
        Ok(Self {
            partition_id: partition.id,
            volume_index,
            snapshot_xid,
            base_offset,
            tier2_base,
            volume: Mutex::new(volume),
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// The selected volume's APFS_INCOMPAT_CASE_INSENSITIVE flag.
    pub(crate) fn names_case_insensitive(&self) -> Result<bool> {
        self.with_volume(|volume| Ok(volume.volume_info().incompatible_features & 1 != 0))
    }

    /// One streaming pass over the volume's whole catalog B-tree (the OMAP is
    /// preloaded inside the vendored crate). The bulk primitive behind the
    /// snapshot fast path in [`crate::core::catalog::snapshot::volume_snapshot_apfs`] — callers
    /// join `DirEntry.drec.file_id` → `Inode.oid` in memory instead of paying
    /// one B-tree descent per child the way [`children`](Self::children) does.
    /// The visitor returns `false` to stop early (cancellation); the walk then
    /// returns `Ok(false)`.
    pub fn walk_catalog(&self, visit: &mut dyn FnMut(apfs::CatalogRecord) -> bool) -> Result<bool> {
        let partition_id = self.partition_id;
        self.with_volume(move |vol| {
            vol.walk_catalog(visit)
                .map_err(|err| anyhow!("APFS walk_catalog partition {partition_id}: {err}"))
        })
    }

    /// Resolve the logical metadata that a compressed/symlink inode alone omits.
    pub(crate) fn catalog_logical_metadata(&self, oid: u64) -> Result<(u64, Option<String>)> {
        self.with_volume(|volume| {
            let stat = volume.stat_by_id(oid)?;
            let target = if stat.kind == EntryKind::Symlink {
                let mut bytes = Vec::new();
                volume.open_file_by_id(oid)?.read_to_end(&mut bytes)?;
                Some(
                    String::from_utf8(bytes)
                        .map_err(|error| anyhow!("APFS symlink {oid}: {error}"))?,
                )
            } else {
                None
            };
            Ok((stat.size, target))
        })
    }

    /// Lists the children of a path inside the volume. The path uses Unix-
    /// style separators with a leading `/` (or the empty string for the
    /// container's first volume root).
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.node_id(path);
        let parent_path = path.to_string();
        let probe_path = if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        };
        self.with_volume(move |vol| {
            let entries = vol
                .list_directory(&probe_path)
                .map_err(|err| anyhow!("APFS list_directory {probe_path}: {err}"))?;

            let mut out = Vec::new();
            for entry in entries {
                let child_path = join_path(&parent_path, &entry.name);
                let kind = node_kind_from(entry.kind);
                let size = if kind == NodeKind::Dir { 0 } else { entry.size };
                let timestamps = apfs_timestamps(
                    entry.create_time,
                    entry.modify_time,
                    entry.change_time,
                    entry.access_time,
                );
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name: entry.name,
                    parent_id: Some(parent_id.clone()),
                    kind,
                    size,
                    deleted: false,
                    inode: Some(entry.oid),
                    timestamps,
                    link_target: entry.link_target,
                    has_snapshot: false,
                });
            }
            out.sort_by(|a, b| {
                let ak = u8::from(a.kind != NodeKind::Dir);
                let bk = u8::from(b.kind != NodeKind::Dir);
                ak.cmp(&bk)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(out)
        })
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        if path.is_empty() {
            anyhow::bail!("cannot read APFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("APFS open_file {owned}: {err}"))?;
            let mut out = Vec::new();
            let mut buf = vec![0u8; STREAM_FLOOR];
            while out.len() < max_bytes {
                let want = (max_bytes - out.len()).min(buf.len());
                let read = std::io::Read::read(&mut file, &mut buf[..want])
                    .map_err(|err| anyhow!("APFS read {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
            }
            Ok(out)
        })
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream APFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("APFS open_file {owned}: {err}"))?;
            let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
            let mut total = 0u64;
            loop {
                let read = std::io::Read::read(&mut file, &mut buf)
                    .map_err(|err| anyhow!("APFS stream {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                visitor(&buf[..read])?;
                total += read as u64;
            }
            Ok(total)
        })
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        if path.is_empty() {
            anyhow::bail!("cannot read APFS volume root as a file");
        }
        self.with_volume(|volume| {
            volume
                .read_file_range(path, offset, length)
                .map_err(|error| anyhow!("APFS range {path} at {offset}: {error}"))
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat APFS volume root as a file");
        }
        let owned = path.to_string();
        let node_id = node_id.to_string();
        self.with_volume(move |vol| {
            let stat = vol
                .stat(&owned)
                .map_err(|err| anyhow!("APFS stat {owned}: {err}"))?;

            let sample = if matches!(stat.kind, EntryKind::File) && stat.size > 0 {
                let mut file = vol
                    .open_file(&owned)
                    .map_err(|err| anyhow!("APFS open_file {owned}: {err}"))?;
                let want = SAMPLE_BYTES.min(stat.size as usize);
                let mut buf = vec![0u8; want];
                let mut filled = 0usize;
                while filled < buf.len() {
                    let read = std::io::Read::read(&mut file, &mut buf[filled..])
                        .map_err(|err| anyhow!("APFS stat read {owned}: {err}"))?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                buf.truncate(filled);
                buf
            } else {
                Vec::new()
            };

            let mut data_runs = Vec::new();
            if let Some(extents) = vol.file_extents(&owned)? {
                for record in extents {
                    if record.value.phys_block_num == 0 || record.logical_addr >= stat.size {
                        continue;
                    }
                    let mut logical_offset = record.logical_addr;
                    let length = record.value.length().min(stat.size - logical_offset);
                    for span in vol.extent_source_spans(record.value.phys_block_num, length)? {
                        let base = match span.tier {
                            FusionTier::Main => Some(self.base_offset),
                            FusionTier::Secondary => self.tier2_base,
                        };
                        let physical_offset = base
                            .and_then(|base| base.checked_add(span.offset))
                            .ok_or_else(|| {
                            anyhow!(Failure::corrupt("APFS source extent offset overflow"))
                        })?;
                        data_runs.push(FsDataRun {
                            logical_offset,
                            physical_offset,
                            length: span.length,
                        });
                        logical_offset += span.length;
                    }
                }
            }
            Ok(FsStat {
                node_id,
                size: stat.size,
                inode: Some(stat.oid),
                mode: Some(stat.mode),
                mime: detect_mime_bytes(&sample).to_string(),
                magic_hex: if sample.is_empty() {
                    None
                } else {
                    Some(
                        sample
                            .iter()
                            .take(16)
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                },
                entropy: if sample.is_empty() {
                    None
                } else {
                    Some(shannon_entropy(&sample))
                },
                data_runs,
            })
        })
    }

    fn node_id(&self, path: &str) -> String {
        let source = ApfsSource::Partition {
            part_id: self.partition_id,
            volume_index: self.volume_index,
            snapshot_xid: self.snapshot_xid,
        };
        format!("{}:{}", source.cache_key(), hex::encode(path.as_bytes()))
    }

    /// Runs `op` against the parsed `apfs::ApfsVolume` held by this volume.
    /// Built once in [`ApfsVolume::open`]; reads address absolute offsets, so
    /// reusing one cursor across calls is safe for read-only access. A
    /// poisoned mutex (a prior panic mid-read) surfaces as an error.
    fn with_volume<R>(
        &self,
        op: impl FnOnce(&mut ContainerVolume<PartitionCursor>) -> Result<R>,
    ) -> Result<R> {
        let mut volume = self
            .volume
            .lock()
            .map_err(|_| anyhow!("APFS volume partition {} mutex poisoned", self.partition_id))?;
        op(&mut volume)
    }
}

fn list_apfs_volumes(image: Arc<DiskImage>, partition: &Partition) -> Result<Vec<VolumeInfo>> {
    let (main, tier2) = open_members(image, partition)?;
    ContainerVolume::list_source_volumes(main, tier2)
        .map_err(|err| anyhow!("APFS list volumes partition {}: {err}", partition.id))
}

fn apfs_volume_node(partition_id: u8, parent_id: &str, info: VolumeInfo) -> FsNode {
    let name = if info.name.trim().is_empty() {
        format!("APFS_{}", info.index)
    } else {
        format!("APFS_{}_{}", info.index, info.name)
    };
    FsNode {
        id: format!("apfs:part:{partition_id}:{}:", info.index),
        name: crate::core::common::navigation_name::volume_name(&name),
        parent_id: Some(parent_id.to_string()),
        kind: NodeKind::Dir,
        size: 0,
        deleted: false,
        inode: Some(info.oid),
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }
}

/// The `apfs:part:{N}:{volume_index}:{hex(path)}` node id for a volume-
/// relative path (empty path = the volume root, empty hex). Shared with the
/// snapshot fast path so its ids round-trip through [`parse_apfs_id`] exactly
/// like live-listing ids.
pub(crate) fn apfs_node_id(partition_id: u8, volume_index: usize, path: &str) -> String {
    if path.is_empty() {
        format!("apfs:part:{partition_id}:{volume_index}:")
    } else {
        format!(
            "apfs:part:{partition_id}:{volume_index}:{}",
            hex::encode(path.as_bytes())
        )
    }
}

fn node_kind_from(kind: EntryKind) -> NodeKind {
    match kind {
        EntryKind::Directory => NodeKind::Dir,
        EntryKind::Symlink => NodeKind::Symlink,
        EntryKind::File => NodeKind::File,
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub(crate) fn apfs_timestamps(
    create_time: i64,
    modify_time: i64,
    change_time: i64,
    access_time: i64,
) -> Mactb {
    let created = epoch_ns_to_rfc3339(create_time);
    Mactb {
        modified: epoch_ns_to_rfc3339(modify_time),
        accessed: epoch_ns_to_rfc3339(access_time),
        created: created.clone(),
        mft_changed: epoch_ns_to_rfc3339(change_time),
        birth: created,
    }
}

/// APFS create/modify timestamps are nanoseconds since the Unix epoch
/// (i64). The catalog stores 0 for unset entries — pass that through as
/// `None` so the UI doesn't render a 1970 date.
fn epoch_ns_to_rfc3339(nanos: i64) -> Option<String> {
    if nanos == 0 {
        return None;
    }
    let secs = nanos.div_euclid(1_000_000_000);
    let sub = nanos.rem_euclid(1_000_000_000) as u32;
    Utc.timestamp_opt(secs, sub)
        .single()
        .map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apfs_fusion_partitions_keep_member_run_offsets() {
        use crate::core::filesystem::evidence_session::EvidenceSession;
        use crate::core::filesystem::fs_dispatch::dispatch;

        let main = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/apfs/tests/data/fusion-main.apfs"
        ));
        let tier2 = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/apfs/tests/data/fusion-tier2.apfs"
        ));
        let mut disk = vec![0; 4096];
        disk[510..512].copy_from_slice(&[0x55, 0xaa]);
        let offsets = [4096, 4096 + tier2.len()];
        for (slot, (fixture, offset)) in [(&tier2[..], offsets[0]), (&main[..], offsets[1])]
            .into_iter()
            .enumerate()
        {
            let entry = 446 + 16 * slot;
            disk[entry + 4] = 0xaf;
            disk[entry + 8..entry + 12].copy_from_slice(&((offset / 512) as u32).to_le_bytes());
            disk[entry + 12..entry + 16]
                .copy_from_slice(&((fixture.len() / 512) as u32).to_le_bytes());
            disk.extend_from_slice(fixture);
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fusion.raw");
        std::fs::write(&path, disk).unwrap();
        let session = Arc::new(EvidenceSession::open(&path).unwrap());
        assert_eq!(session.partitions.len(), 2);
        for partition in &session.partitions {
            assert_eq!(partition.kind, PartitionKind::ApfsContainer);
            let id = apfs_node_id(partition.id, 0, "/plain.bin");
            let volume = dispatch(&session, &id).unwrap().unwrap();
            assert_eq!(volume.read_range(&id, 4095, 3).unwrap().unwrap(), b"ABB");
            let stat = volume.file_stat(&id, &id).unwrap();
            assert_eq!(stat.data_runs.len(), 2);
            assert_eq!(
                stat.data_runs[0].physical_offset,
                (offsets[0] + 3 * 4096) as u64
            );
            assert_eq!(
                stat.data_runs[1].physical_offset,
                (offsets[1] + 13 * 4096) as u64
            );
        }
    }

    #[test]
    fn apfs_synthetic_partition_supports_source_runs_symlinks_and_direct_ranges() {
        use crate::core::evidence::partitions::PartitionKind;
        use crate::core::filesystem::evidence_session::EvidenceSession;
        use crate::core::filesystem::fs_dispatch::dispatch;
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/apfs/tests/data/basic.apfs"
        ));
        let mut disk = vec![0u8; 4096];
        disk[446 + 4] = 0xaf;
        disk[446 + 8..446 + 12].copy_from_slice(&8u32.to_le_bytes());
        disk[446 + 12..446 + 16].copy_from_slice(&((fixture.len() / 512) as u32).to_le_bytes());
        disk[510..512].copy_from_slice(&[0x55, 0xaa]);
        disk.extend_from_slice(fixture);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.raw");
        std::fs::write(&path, disk).unwrap();
        let session = Arc::new(EvidenceSession::open(&path).unwrap());
        let partition = session
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::ApfsContainer)
            .unwrap();
        assert_eq!(partition.start_lba, 8);
        let id = apfs_node_id(partition.id, 0, "/plain.bin");
        let volume = dispatch(&session, &id).unwrap().unwrap();
        let range = volume.read_range(&id, 4090, 20).unwrap().unwrap();
        assert_eq!(range, [vec![b'A'; 6], vec![b'B'; 14]].concat());
        let stat = volume.file_stat(&id, &id).unwrap();
        assert_eq!(stat.data_runs.len(), 2);
        assert_eq!(stat.data_runs[0].physical_offset, 4096 + 12 * 4096);
        assert_eq!(stat.data_runs[1].length, 904);
        let root = apfs_node_id(partition.id, 0, "");
        let children = volume
            .children(
                &session,
                &root,
                crate::core::filesystem::tree::fs_tree::ChildrenOptions {
                    include_deleted_ntfs: false,
                    include_unallocated_nodes: false,
                },
            )
            .unwrap();
        assert_eq!(
            children
                .iter()
                .find(|node| node.name == "link")
                .unwrap()
                .link_target
                .as_deref(),
            Some("../plain.bin")
        );

        use crate::core::filesystem::tree::fs_tree::FsSession;
        let mut fs = FsSession::new(&session);
        let root_nodes = fs.children(&format!("part:{}", partition.id)).unwrap();
        let snapshot = root_nodes
            .iter()
            .find(|node| node.name.ends_with("snap-one"))
            .unwrap();
        assert_eq!(snapshot.id, format!("apfs:part:{}:0@5:", partition.id));
        let snapshot_nodes = fs.children(&snapshot.id).unwrap();
        let historical = snapshot_nodes
            .iter()
            .find(|node| node.name == "old.bin")
            .unwrap();
        let historical_clone = snapshot_nodes
            .iter()
            .find(|node| node.name == "clone.bin")
            .unwrap();
        let live_clone = children
            .iter()
            .find(|node| node.name == "clone.bin")
            .unwrap();
        assert_ne!(historical_clone.id, live_clone.id);
        assert_eq!(
            fs.read_file_range(&historical.id, 4090, 20).unwrap(),
            [vec![b'P'; 6], vec![b'B'; 14]].concat()
        );
        assert_eq!(fs.read_file_range(&live_clone.id, 0, 8).unwrap(), [b'A'; 8]);
        assert_eq!(
            fs.read_file_range(&historical_clone.id, 0, 8).unwrap(),
            [b'P'; 8]
        );
    }
    use crate::core::evidence::image::{DiskImage, PartitionKind};

    #[test]
    fn parse_apfs_id_with_volume_index() {
        let encoded = hex::encode("/Users".as_bytes());
        let (source, path) =
            parse_apfs_id(&format!("apfs:part:2:7:{encoded}")).expect("APFS id should parse");

        assert!(matches!(
            source,
            ApfsSource::Partition {
                part_id: 2,
                volume_index: 7,
                snapshot_xid: None,
            }
        ));
        assert_eq!(path, "/Users");
    }

    #[test]
    fn parse_legacy_apfs_id_without_volume_index() {
        let encoded = hex::encode("/System".as_bytes());
        let (source, path) =
            parse_apfs_id(&format!("apfs:part:2:{encoded}")).expect("legacy APFS id should parse");

        assert!(matches!(source, ApfsSource::LegacyPartition(2)));
        assert_eq!(path, "/System");
    }

    #[test]
    fn apfs_timestamps_include_access_and_change_times() {
        let timestamps = apfs_timestamps(
            1_700_000_000_000_000_000,
            1_700_000_001_000_000_000,
            1_700_000_002_000_000_000,
            1_700_000_003_000_000_000,
        );

        assert_eq!(
            timestamps.created.as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
        assert_eq!(
            timestamps.modified.as_deref(),
            Some("2023-11-14T22:13:21+00:00")
        );
        assert_eq!(
            timestamps.mft_changed.as_deref(),
            Some("2023-11-14T22:13:22+00:00")
        );
        assert_eq!(
            timestamps.accessed.as_deref(),
            Some("2023-11-14T22:13:23+00:00")
        );
        assert_eq!(timestamps.birth, timestamps.created);
    }

    /// Env-gated regression guard for the decmpfs read path:
    /// `LOVELYCARVE_APFS_DECMPFS_TEST` should point at a macOS APFS
    /// image whose `/Library/Bundles/OSXNotification.bundle/Contents/version.plist`
    /// is stored with `UF_COMPRESSED` (true on every Catalina+ system
    /// volume). The test asserts the file reads back as a non-empty
    /// plist text. Pre-fix this raised "logical offset beyond extent
    /// map" because lookup_extents returned an empty Vec for compressed
    /// files; post-fix the read flows through `vendor/apfs::decmpfs`.
    #[test]
    fn reads_decmpfs_compressed_plist_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_APFS_DECMPFS_TEST") else {
            return;
        };
        let image =
            std::sync::Arc::new(DiskImage::open(path).expect("APFS test image should open"));
        let apfs_part = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::ApfsContainer)
            .cloned()
            .expect("expected at least one APFS partition");
        let volume = ApfsVolume::open(std::sync::Arc::clone(&image), &apfs_part)
            .expect("APFS volume should open");
        let bytes = volume
            .read_file(
                "/Library/Bundles/OSXNotification.bundle/Contents/version.plist",
                64 * 1024,
            )
            .expect("compressed version.plist should read without 'beyond extent map'");
        assert!(
            !bytes.is_empty(),
            "decompressed version.plist should not be empty"
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("plist") || text.contains("<?xml") || text.contains("CFBundle"),
            "decompressed bytes don't look like a plist: first 64 bytes = {:?}",
            &bytes[..bytes.len().min(64)]
        );
    }
}
