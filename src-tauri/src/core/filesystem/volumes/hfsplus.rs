//! Read-only HFS+ / HFSX volume adapter built on the vendored `hfsplus`
//! crate.
//!
//! Each backend operation instantiates a fresh `hfsplus::HfsVolume` over a
//! transient cursor against the partition slice (mirrors `core::filesystem::volumes::apfs`)
//! — there is no shared mutable state, so the dispatcher can fan out
//! concurrent listings / reads without per-volume locking.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `hfsplus:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly — the partition node itself
//! stands in for the root.

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};
use hfsplus::{EntryKind, HfsVolume};

use crate::core::common::error::Failure;
use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::evidence::partitions::hfsplus::{
    WrappedHfsPlus, HFS_MDB_SIZE, HFS_VOLUME_HEADER_OFFSET,
};
use crate::core::evidence::partitions::partition_cursor::PartitionCursor;
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;

/// Seconds from 1904-01-01 UTC to the Unix epoch. HFS+ timestamps are UTC
/// seconds (TN1150); traditional HFS local-time fields are unsupported.
const HFS_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

#[derive(Debug, Clone, Copy)]
pub enum HfsPlusSource {
    Partition(u8),
}

/// Parse an `hfsplus:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_hfsplus_id(id: &str) -> Option<(HfsPlusSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("hfsplus"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((HfsPlusSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_hfsplus_source(image: &Arc<DiskImage>, source: HfsPlusSource) -> Result<HfsPlusVolume> {
    match source {
        HfsPlusSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            HfsPlusVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct HfsPlusVolume {
    partition_id: u8,
    cursor: PartitionCursor,
    embedded_signature: Option<u16>,
    decrypted: bool,
}

impl HfsPlusVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let mut cursor = PartitionCursor::new(Arc::clone(&image), partition)?;
        let wrapper = if cursor.length_bytes >= (HFS_VOLUME_HEADER_OFFSET + HFS_MDB_SIZE) as u64 {
            // An unreadable primary is left to the HFS+ reader's alternate-header path.
            match image.read_range(
                cursor.base_offset + HFS_VOLUME_HEADER_OFFSET as u64,
                HFS_MDB_SIZE,
            ) {
                Ok(mdb) => WrappedHfsPlus::parse(&mdb, cursor.length_bytes)?,
                Err(_) => None,
            }
        } else {
            None
        };
        if let Some(wrapper) = wrapper {
            cursor.base_offset += wrapper.offset;
            cursor.length_bytes = wrapper.length;
        }
        let volume = Self {
            partition_id: partition.id,
            cursor,
            embedded_signature: wrapper.map(|wrapper| wrapper.signature),
            decrypted: matches!(
                image.derived,
                Some(crate::core::evidence::image::DerivedImage::Decrypted { .. })
            ),
        };
        // Eager-validate the volume header so the caller sees parse errors
        // at open time instead of only when listing the root.
        volume.with_volume(|_| Ok(()))?;
        Ok(volume)
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Unallocated bytes in this volume, read straight from the HFS+ volume
    /// header's `free_blocks × block_size`. Exact and cheap — the header is
    /// already parsed on open.
    pub fn free_bytes(&self) -> Result<u64> {
        self.with_volume(|vol| {
            let header = vol.volume_header();
            Ok(header.free_blocks as u64 * header.block_size as u64)
        })
    }

    /// Lists the children of a path inside the volume. The path uses Unix-
    /// style separators with a leading `/` (or the empty string for the
    /// volume root).
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
                .map_err(|err| anyhow!("HFS+ list_directory {probe_path}: {err}"))?;

            let mut out = Vec::new();
            for entry in entries {
                let child_path = join_path(&parent_path, &entry.name);
                let kind = node_kind_from(entry.kind);
                let size = if kind == NodeKind::Dir { 0 } else { entry.size };
                let timestamps = hfsplus_timestamps(
                    entry.create_date,
                    entry.modify_date,
                    entry.attribute_mod_date,
                    entry.access_date,
                );
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name: entry.name,
                    parent_id: Some(parent_id.clone()),
                    kind,
                    size,
                    deleted: false,
                    inode: Some(entry.cnid as u64),
                    timestamps,
                    link_target: if kind == NodeKind::Symlink {
                        Some(
                            String::from_utf8(vol.read_file(&child_path)?)
                                .map_err(|error| anyhow!("HFS+ symlink {child_path}: {error}"))?,
                        )
                    } else {
                        None
                    },
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
            anyhow::bail!("cannot read HFS+ volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
            let mut out = Vec::new();
            let mut buf = vec![0u8; STREAM_FLOOR];
            while out.len() < max_bytes {
                let want = (max_bytes - out.len()).min(buf.len());
                let read = std::io::Read::read(&mut file, &mut buf[..want])
                    .map_err(|err| anyhow!("HFS+ read {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
            }
            Ok(out)
        })
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.with_volume(|volume| {
            let mut file = volume.open_file(path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut output = Vec::new();
            file.take(length as u64).read_to_end(&mut output)?;
            Ok(output)
        })
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream HFS+ volume root as a file");
        }
        let owned = path.to_string();
        self.with_volume(move |vol| {
            let mut file = vol
                .open_file(&owned)
                .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
            let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
            let mut total = 0u64;
            loop {
                let read = std::io::Read::read(&mut file, &mut buf)
                    .map_err(|err| anyhow!("HFS+ stream {owned}: {err}"))?;
                if read == 0 {
                    break;
                }
                visitor(&buf[..read])?;
                total += read as u64;
            }
            Ok(total)
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat HFS+ volume root as a file");
        }
        let owned = path.to_string();
        let node_id = node_id.to_string();
        self.with_volume(move |vol| {
            let stat = vol
                .stat(&owned)
                .map_err(|err| anyhow!("HFS+ stat {owned}: {err}"))?;

            let compressed = stat.compression.is_some();

            let sample = if matches!(stat.kind, EntryKind::File) && stat.size > 0 {
                let mut file = vol
                    .open_file(&owned)
                    .map_err(|err| anyhow!("HFS+ open_file {owned}: {err}"))?;
                let want = SAMPLE_BYTES.min(stat.size as usize);
                let mut buf = vec![0u8; want];
                let mut filled = 0usize;
                while filled < buf.len() {
                    let read = std::io::Read::read(&mut file, &mut buf[filled..])
                        .map_err(|err| anyhow!("HFS+ stat read {owned}: {err}"))?;
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

            let mime = detect_mime_bytes(&sample).to_string();
            let data_runs = if compressed || self.decrypted {
                Vec::new()
            } else {
                let block_size = u64::from(vol.volume_header().block_size);
                let mut logical_offset = 0u64;
                vol.data_extents(&owned)?
                    .into_iter()
                    .filter_map(|extent| {
                        let length = (u64::from(extent.block_count) * block_size)
                            .min(stat.size.saturating_sub(logical_offset));
                        let run = FsDataRun {
                            logical_offset,
                            physical_offset: self.cursor.base_offset
                                + u64::from(extent.start_block) * block_size,
                            length,
                        };
                        logical_offset += length;
                        (length > 0).then_some(run)
                    })
                    .collect()
            };

            Ok(FsStat {
                node_id,
                size: stat.size,
                inode: Some(stat.cnid as u64),
                mode: Some(stat.permissions.mode),
                mime,
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
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "hfsplus:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    /// Constructs a fresh `hfsplus::HfsVolume` over the partition slice and
    /// hands it to the closure. The volume is dropped immediately after the
    /// closure returns; the cursor is read-only by construction so the
    /// underlying image is never mutated.
    fn with_volume<R>(
        &self,
        op: impl FnOnce(&mut HfsVolume<PartitionCursor>) -> Result<R>,
    ) -> Result<R> {
        let mut vol = HfsVolume::open(self.cursor.clone())
            .map_err(|err| anyhow!("HFS+ open partition {}: {err}", self.partition_id))?;
        anyhow::ensure!(
            self.embedded_signature
                .is_none_or(|signature| signature == vol.volume_header().signature),
            Failure::corrupt("embedded HFS+ signature differs from the HFS wrapper")
        );
        op(&mut vol)
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

fn hfsplus_timestamps(
    create_date: u32,
    modify_date: u32,
    attribute_mod_date: u32,
    access_date: u32,
) -> Mactb {
    let created = hfs_time_to_rfc3339(create_date);
    Mactb {
        modified: hfs_time_to_rfc3339(modify_date),
        accessed: hfs_time_to_rfc3339(access_date),
        created: created.clone(),
        mft_changed: hfs_time_to_rfc3339(attribute_mod_date),
        birth: created,
    }
}

/// HFS+ stores 32-bit timestamps as seconds since 1904-01-01. Unset entries
/// are zero — pass those through as `None` so the UI doesn't render a 1904
/// date.
fn hfs_time_to_rfc3339(seconds: u32) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let unix_secs = seconds as i64 - HFS_EPOCH_OFFSET_SECS;
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::image::PartitionKind;
    use crate::core::filesystem::evidence_session::EvidenceSession;
    use crate::core::filesystem::tree::fs_tree::{self, FsSession};
    use flate2::read::ZlibDecoder;
    use sha2::{Digest, Sha256};

    const HFSPLUS_WRAPPED: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_data/hfsplus/wrapped-hfsplus.hfsz"
    ));
    const HFSX_WRAPPED: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test_data/hfsplus/wrapped-hfsx.hfsz"
    ));
    const EMBEDDED_START: usize = 45056;
    const EMBEDDED_LENGTH: usize = 16728064;

    fn inflate(compressed: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        ZlibDecoder::new(compressed).read_to_end(&mut raw).unwrap();
        raw
    }

    fn evidence(bytes: &[u8]) -> (tempfile::TempDir, Arc<EvidenceSession>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrapper.img");
        std::fs::write(&path, bytes).unwrap();
        let image = DiskImage::open(&path).unwrap();
        (directory, Arc::new(EvidenceSession::from(image)))
    }

    #[test]
    fn hfsplus_native_wrappers_keep_partition_identity_and_file_bytes() {
        for (compressed, expected_kind) in [
            (HFSPLUS_WRAPPED, PartitionKind::HfsPlus),
            (HFSX_WRAPPED, PartitionKind::HfsX),
        ] {
            let native = inflate(compressed);
            let mut source = Vec::new();
            let base = if expected_kind == PartitionKind::HfsX {
                source.resize(4096, 0);
                source[..2].copy_from_slice(b"ER");
                source[2..4].copy_from_slice(&512u16.to_be_bytes());
                let (map, volume) = source[512..1536].split_at_mut(512);
                for (entry, start, count, name, kind) in [
                    (
                        map,
                        1u32,
                        2u32,
                        b"Map".as_slice(),
                        b"Apple_partition_map".as_slice(),
                    ),
                    (
                        volume,
                        8,
                        (native.len() / 512) as u32,
                        b"Wrapped".as_slice(),
                        b"Apple_HFS".as_slice(),
                    ),
                ] {
                    entry[..2].copy_from_slice(b"PM");
                    entry[4..8].copy_from_slice(&2u32.to_be_bytes());
                    entry[8..12].copy_from_slice(&start.to_be_bytes());
                    entry[12..16].copy_from_slice(&count.to_be_bytes());
                    entry[16..16 + name.len()].copy_from_slice(name);
                    entry[48..48 + kind.len()].copy_from_slice(kind);
                }
                4096
            } else {
                0
            };
            source.extend_from_slice(&native);
            let (_directory, disk) = evidence(&source);
            let part = disk
                .partitions
                .iter()
                .find(|part| part.kind == expected_kind)
                .unwrap();
            assert_eq!(part.start_lba * disk.sector_size as u64, base as u64);
            assert_eq!(
                part.length_lba * disk.sector_size as u64,
                native.len() as u64
            );
            assert_eq!(disk.read_range(base as u64, native.len()).unwrap(), native);
            let id = part.id;
            assert_eq!(id, u8::from(expected_kind == PartitionKind::HfsX));
            let mut session = FsSession::new(&disk);
            let root = session.children(&format!("part:{id}")).unwrap();
            let folder = root.iter().find(|node| node.name == "Nested").unwrap();
            let link = root.iter().find(|node| node.name == "link").unwrap();
            assert_eq!(link.link_target.as_deref(), Some("Nested/Payload.bin"));
            let children = session.children(&folder.id).unwrap();
            let upper = children
                .iter()
                .find(|node| node.name == "Payload.bin")
                .unwrap();
            assert!(upper.id.starts_with(&format!("hfsplus:part:{id}:")));
            let full = session.read_file_limited(&upper.id, usize::MAX).unwrap();
            assert_eq!(
                hex::encode(Sha256::digest(&full)),
                "ac61e47d19009febed3505e10edcc20d651facb427fbc469d944982dbf941f1a"
            );
            assert_eq!(
                session.read_file_range(&upper.id, 4084, 47).unwrap(),
                full[4084..4131]
            );
            let mut streamed = Vec::new();
            let total = fs_tree::stream_file_chunks(&disk, &upper.id, 4096, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(total, full.len() as u64);
            assert_eq!(streamed, full);

            let stat = session.file_stat(&upper.id).unwrap();
            let start = (base + EMBEDDED_START) as u64;
            let end = start + EMBEDDED_LENGTH as u64;
            let mut from_source = Vec::new();
            for run in stat.data_runs {
                assert!(run.physical_offset >= start && run.physical_offset + run.length <= end);
                assert_eq!(run.logical_offset, from_source.len() as u64);
                from_source.extend(
                    disk.read_range(run.physical_offset, run.length as usize)
                        .unwrap(),
                );
            }
            assert_eq!(from_source, full);
            if expected_kind == PartitionKind::HfsX {
                let lower = children
                    .iter()
                    .find(|node| node.name == "payload.bin")
                    .unwrap();
                assert_ne!(lower.id, upper.id);
                assert_eq!(
                    session.read_file_limited(&lower.id, 100).unwrap(),
                    b"case-sensitive\n"
                );
            }
        }
    }

    #[test]
    fn hfsplus_wrapper_uses_embedded_alternate_header_and_refuses_invalid_windows() {
        let native = inflate(HFSPLUS_WRAPPED);
        let mut mbr = vec![0; 512];
        mbr[450] = 0xaf;
        mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
        mbr[458..462].copy_from_slice(&((native.len() / 512) as u32).to_le_bytes());
        mbr[510..512].copy_from_slice(&[0x55, 0xaa]);
        mbr.extend_from_slice(&native);
        let offset = 512 + EMBEDDED_START + 1024;
        mbr[offset..offset + 512].fill(0);
        let (_directory, disk) = evidence(&mbr);
        let partition = &disk.partitions[0];
        assert_eq!(partition.kind, PartitionKind::HfsPlus);
        assert_eq!(partition.start_lba, 1);
        let volume =
            open_hfsplus_source(disk.disk_image(), HfsPlusSource::Partition(partition.id)).unwrap();
        assert_eq!(volume.children("/Nested").unwrap()[0].name, "Payload.bin");

        for (offset, value) in [
            (1024 + 10, 0x0300u16),
            (1024 + 124, 0x4858),
            (1024 + 128, u16::MAX),
        ] {
            let mut damaged = mbr.clone();
            let offset = 512 + offset;
            damaged[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
            let (_directory, disk) = evidence(&damaged);
            assert!(HfsPlusVolume::open(Arc::clone(disk.disk_image()), partition).is_err());
        }
        let mut shorter = partition.clone();
        shorter.length_lba = (EMBEDDED_START + EMBEDDED_LENGTH - 512) as u64 / 512;
        assert!(HfsPlusVolume::open(Arc::clone(disk.disk_image()), &shorter).is_err());
    }

    fn hfs_timestamp(unix_seconds: i64) -> u32 {
        (unix_seconds + HFS_EPOCH_OFFSET_SECS) as u32
    }

    #[test]
    fn hfsplus_timestamps_include_access_and_attribute_change_times() {
        let timestamps = hfsplus_timestamps(
            hfs_timestamp(1_700_000_000),
            hfs_timestamp(1_700_000_001),
            hfs_timestamp(1_700_000_002),
            hfs_timestamp(1_700_000_003),
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
}
