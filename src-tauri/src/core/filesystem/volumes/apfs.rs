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
//! the selected volume root.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use apfs::{ApfsVolume as ContainerVolume, EntryKind, VolumeInfo};
use chrono::{TimeZone, Utc};

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum ApfsSource {
    Partition { part_id: u8, volume_index: usize },
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
            } => format!("apfs:part:{part_id}:{volume_index}"),
            ApfsSource::LegacyPartition(part_id) => format!("apfs:part:{part_id}:legacy"),
        }
    }
}

/// Parse an `apfs:part:{N}:{volume_index}:{hex(path)}` ID. Returns the source
/// plus the decoded UTF-8 path (with a leading `/`, or empty for the volume
/// root). The legacy `apfs:part:{N}:{hex(path)}` form is still accepted for
/// existing snapshots and bookmarks, and maps to the container's first volume.
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
            let volume_index = volume_index.parse().ok()?;
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
        } => {
            let partition = find_partition(image, &part_id.to_string())?;
            ApfsVolume::open_index(Arc::clone(image), partition, volume_index)
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

pub struct ApfsVolume {
    partition_id: u8,
    volume_index: usize,
    base_offset: u64,
    /// Parsed selected APFS volume, built once in [`ApfsVolume::open`] and
    /// reused behind a mutex (the `apfs` crate's reads take `&mut`). See the
    /// module header for why it is held rather than rebuilt per call.
    volume: Mutex<ContainerVolume<PartitionCursor>>,
}

impl ApfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        Self::open_with(image, partition, None)
    }

    pub fn open_index(
        image: Arc<DiskImage>,
        partition: &Partition,
        volume_index: usize,
    ) -> Result<Self> {
        Self::open_with(image, partition, Some(volume_index))
    }

    fn open_with(
        image: Arc<DiskImage>,
        partition: &Partition,
        volume_index: Option<usize>,
    ) -> Result<Self> {
        let cursor = PartitionCursor::new(image, partition)?;
        let base_offset = cursor.base_offset;
        let volume = match volume_index {
            Some(index) => ContainerVolume::open_index(cursor, index).map_err(|err| {
                anyhow!("APFS open partition {} volume {index}: {err}", partition.id)
            })?,
            None => ContainerVolume::open(cursor)
                .map_err(|err| anyhow!("APFS open partition {}: {err}", partition.id))?,
        };
        let volume_index = volume.volume_info().index;
        Ok(Self {
            partition_id: partition.id,
            volume_index,
            base_offset,
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

            let block_size = u64::from(vol.volume_info().block_size);
            let mut data_runs = Vec::new();
            if let Some(extents) = vol.file_extents(&owned)? {
                for record in extents {
                    if record.value.phys_block_num == 0 || record.logical_addr >= stat.size {
                        continue;
                    }
                    let physical_offset = record
                        .value
                        .phys_block_num
                        .checked_mul(block_size)
                        .and_then(|offset| self.base_offset.checked_add(offset))
                        .ok_or_else(|| anyhow!("APFS source extent offset overflow"))?;
                    data_runs.push(FsDataRun {
                        logical_offset: record.logical_addr,
                        physical_offset,
                        length: record.value.length().min(stat.size - record.logical_addr),
                    });
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
        apfs_node_id(self.partition_id, self.volume_index, path)
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
    let cursor = PartitionCursor::new(image, partition)?;
    ContainerVolume::list_volumes(cursor)
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

/// Cursor exposing a partition slice of `DiskImage` as `std::io::Read +
/// Seek`. Read-only — APFS parsing never writes back, so no `Write` impl
/// is needed (unlike `core::fat_volume::PartitionCursor`).
struct PartitionCursor {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
    position: u64,
}

impl PartitionCursor {
    fn new(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("APFS partition offset overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("APFS partition length overflow"))?;
        if base_offset
            .checked_add(length_bytes)
            .is_none_or(|end| end > image.len())
        {
            anyhow::bail!("APFS partition exceeds source");
        }
        Ok(Self {
            image,
            base_offset,
            length_bytes,
            position: 0,
        })
    }
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.length_bytes || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length_bytes - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let bytes = self
            .image
            .read_range(self.base_offset + self.position, want)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => self.length_bytes as i128 + delta as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "APFS cursor seek before start",
            ));
        }
        self.position = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "APFS seek out of range"))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        disk[446 + 12..446 + 16].copy_from_slice(&128u32.to_le_bytes());
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
                volume_index: 7
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
