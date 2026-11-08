//! Read-only XFS volume adapter built on the vendored `xfs-fuse` crate.
//!
//! The `xfs_fuse::Volume` is built once in [`XfsVolume::open`] and reused for
//! every operation behind a `Mutex` (the crate's reads take `&mut`). Re-
//! parsing the superblock + root inode costs ~5–15 ms on a cold image, so a
//! whole-volume snapshot walk would otherwise pay that per directory; holding
//! the handle (and caching the `XfsVolume` itself in
//! [`crate::filesystem::tree::fs_tree::FsSession`]) keeps it to one parse per walk.
//! Live IPC calls still open their own `XfsVolume`, so the mutex is
//! uncontended outside the single-threaded walker.
//!
//! Node IDs encode the absolute raw POSIX path inside the volume as hex bytes:
//! `xfs:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly — the partition node itself
//! stands in for the root.
//!
//! The vendored `xfs-fuse` originally kept the active superblock in a
//! process-global slot because a few `Decode` impls reach for `sb.has_ftype()`
//! / `sb.sb_uuid` during bincode decoding. LovelyCarve now binds that slot per
//! thread before each volume operation, so a boot XFS partition and an LVM XFS
//! root volume with different UUIDs can be browsed in one process.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use xfs_fuse::{FileType, Volume, XfsIno};

use crate::evidence::image::find_partition;
use crate::evidence::image::{DiskImage, Partition};
use crate::evidence::partitions::lvm2::layout::LinearLayout;
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};
use crate::filesystem::volumes::lvm::{physical_runs, read_backing_range, LvmVolume};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;
/// Cap how much of a directory we surface to the UI in one listing. XFS
/// directories scale far beyond this on real filesystems (think `cur/` for a
/// large maildir), but past this the UI becomes unusable; pagination can be
/// added later.
const MAX_CHILDREN_PER_DIR: usize = 50_000;

#[derive(Debug, Clone, Copy)]
pub enum XfsSource {
    Partition(u8),
    Lvm { part_id: u8, lv_index: usize },
}

/// Parse a `xfs:part:{N}:{hex(path)}` or `xfs:lvm:{N}:{M}:{hex(path)}` ID.
/// Returns the source plus the decoded raw POSIX path (with a leading `/`, or
/// empty for the volume root).
pub fn parse_xfs_id(id: &str) -> Option<(XfsSource, Vec<u8>)> {
    let mut parts = id.split(':');
    match parts.next()? {
        "xfs" => {}
        _ => return None,
    }
    match parts.next()? {
        "part" => {
            let part_id = parts.next()?.parse().ok()?;
            let hex_path = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let path = decode_hex_path(hex_path)?;
            Some((XfsSource::Partition(part_id), path))
        }
        "lvm" => {
            let part_id = parts.next()?.parse().ok()?;
            let lv_index = parts.next()?.parse().ok()?;
            let hex_path = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let path = decode_hex_path(hex_path)?;
            Some((XfsSource::Lvm { part_id, lv_index }, path))
        }
        _ => None,
    }
}

fn decode_hex_path(hex_path: &str) -> Option<Vec<u8>> {
    if hex_path.is_empty() {
        Some(Vec::new())
    } else {
        let bytes = hex::decode(hex_path).ok()?;
        Some(bytes)
    }
}

pub fn open_xfs_source(image: &Arc<DiskImage>, source: XfsSource) -> Result<XfsVolume> {
    match source {
        XfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            let sector_size = image.sector_size as u64;
            XfsVolume::open(
                Arc::clone(image),
                source,
                partition
                    .start_lba
                    .checked_mul(sector_size)
                    .ok_or_else(|| anyhow!("XFS partition offset overflow"))?,
                partition
                    .length_lba
                    .checked_mul(sector_size)
                    .ok_or_else(|| anyhow!("XFS partition length overflow"))?,
                format!("part:{part_id}"),
            )
        }
        XfsSource::Lvm { part_id, lv_index } => {
            let partition = find_partition(image, &part_id.to_string())?;
            let lvm = LvmVolume::open(image, partition)?;
            let lv = lvm
                .logical_volumes
                .get(lv_index)
                .ok_or_else(|| anyhow!("logical volume not found: lvm:{part_id}:{lv_index}"))?;
            XfsVolume::open(
                Arc::clone(image),
                source,
                lv.absolute_offset,
                lv.size_bytes,
                format!("lvm:{part_id}:{lv_index}"),
            )
        }
    }
}

pub struct XfsVolume {
    source: XfsSource,
    root_parent_id: String,
    /// Parsed `xfs_fuse::Volume`, built once in [`XfsVolume::open`] and reused
    /// behind a mutex (the crate's reads take `&mut`). See the module header.
    volume: Mutex<Volume<PartitionCursor>>,
    base_offset: u64,
    layout: Option<LinearLayout>,
}

impl XfsVolume {
    pub fn open(
        image: Arc<DiskImage>,
        source: XfsSource,
        base_offset: u64,
        length_bytes: u64,
        root_parent_id: String,
    ) -> Result<Self> {
        let (base_offset, layout) = match source {
            XfsSource::Lvm { part_id, lv_index } => {
                let partition = find_partition(&image, &part_id.to_string())?;
                let volume = LvmVolume::open(&image, partition)?;
                volume
                    .logical_volumes
                    .get(lv_index)
                    .ok_or_else(|| anyhow!("LVM LV missing"))?
                    .reader_geometry()?
            }
            XfsSource::Partition(_) => (base_offset, None),
        };
        let cursor = PartitionCursor {
            image,
            layout: layout.clone(),
            base_offset,
            length_bytes,
            position: 0,
        };
        let volume = Volume::open(cursor).map_err(|errno| {
            anyhow!("XFS open: invalid or unsupported metadata (errno {errno})")
        })?;
        let declared_size = volume
            .sb
            .sb_dblocks
            .checked_mul(u64::from(volume.sb.sb_blocksize))
            .ok_or_else(|| anyhow!("XFS volume size overflow"))?;
        if declared_size > length_bytes {
            anyhow::bail!("XFS declared volume exceeds evidence slice");
        }
        Ok(Self {
            source,
            root_parent_id,
            volume: Mutex::new(volume),
            base_offset,
            layout,
        })
    }

    pub fn open_partition(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        Self::open(
            image,
            XfsSource::Partition(partition.id),
            partition
                .start_lba
                .checked_mul(sector_size)
                .ok_or_else(|| anyhow!("XFS partition offset overflow"))?,
            partition
                .length_lba
                .checked_mul(sector_size)
                .ok_or_else(|| anyhow!("XFS partition length overflow"))?,
            format!("part:{}", partition.id),
        )
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Lists the direct children of `path` inside the volume. `path` uses
    /// Unix-style separators with a leading `/`, or is empty for the volume
    /// root.
    pub fn children(&self, path: impl AsRef<[u8]>) -> Result<Vec<FsNode>> {
        let path = path.as_ref();
        let parent_id = self.node_id(path);
        let parent_path = path.to_vec();
        self.with_volume(|vol| {
            let parent_ino = lookup_path_resilient(vol, path)
                .map_err(|err| anyhow!("XFS lookup_path {path:?}: errno {err}"))?;
            let entries = vol
                .readdir(parent_ino)
                .map_err(|err| anyhow!("XFS readdir {path:?}: errno {err}"))?;

            if entries.len() > MAX_CHILDREN_PER_DIR {
                anyhow::bail!("XFS directory exceeds listing limit");
            }
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries.into_iter() {
                if entry.name == b"." || entry.name == b".." {
                    continue;
                }
                let name = String::from_utf8_lossy(&entry.name).into_owned();
                let attr = match vol.getattr(entry.ino) {
                    Ok(attr) => attr,
                    Err(err) => {
                        tracing::debug!(
                            "skip unreadable XFS entry {}/{} ino={} errno={err}",
                            String::from_utf8_lossy(&parent_path),
                            name,
                            entry.ino
                        );
                        continue;
                    }
                };
                if entry.kind.is_some_and(|kind| kind != attr.kind) {
                    tracing::warn!("XFS directory type differs from inode {} mode", entry.ino);
                }
                let kind = node_kind_from_filetype(attr.kind);
                let size = attr.size;
                let timestamps = attr_timestamps(&attr);
                let child_path = join_unix(&parent_path, &entry.name);
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name,
                    parent_id: Some(parent_id.clone()),
                    kind,
                    size,
                    deleted: false,
                    inode: Some(entry.ino),
                    timestamps,
                    link_target: if kind == NodeKind::Symlink {
                        match vol.readlink(entry.ino) {
                            Ok(target) => Some(String::from_utf8_lossy(&target).into_owned()),
                            Err(errno) => {
                                tracing::warn!(
                                    "XFS inode {} target unavailable: errno {errno}",
                                    entry.ino
                                );
                                None
                            }
                        }
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

    pub fn read_file(&self, path: impl AsRef<[u8]>, max_bytes: usize) -> Result<Vec<u8>> {
        let path = path.as_ref();
        if path.is_empty() {
            anyhow::bail!("cannot read XFS volume root as a file");
        }
        let owned = path.to_vec();
        self.with_volume(|vol| {
            let ino = lookup_path_resilient(vol, &owned)
                .map_err(|err| anyhow!("XFS lookup_path {owned:?}: errno {err}"))?;
            let attr = vol
                .getattr(ino)
                .map_err(|err| anyhow!("XFS getattr {owned:?}: errno {err}"))?;
            match attr.kind {
                FileType::RegularFile => read_file_chunks(vol, ino, attr.size, max_bytes),
                FileType::Symlink => {
                    let mut target = vol
                        .readlink(ino)
                        .map_err(|err| anyhow!("XFS readlink {owned:?}: errno {err}"))?;
                    target.truncate(max_bytes);
                    Ok(target)
                }
                _ => anyhow::bail!("XFS entry {owned:?} is not a regular file"),
            }
        })
    }

    pub fn stream_file<F>(
        &self,
        path: impl AsRef<[u8]>,
        chunk_size: usize,
        mut visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let path = path.as_ref();
        if path.is_empty() {
            anyhow::bail!("cannot stream XFS volume root as a file");
        }
        let owned = path.to_vec();
        self.with_volume(|vol| {
            let ino = lookup_path_resilient(vol, &owned)
                .map_err(|err| anyhow!("XFS lookup_path {owned:?}: errno {err}"))?;
            let attr = vol
                .getattr(ino)
                .map_err(|err| anyhow!("XFS getattr {owned:?}: errno {err}"))?;
            if attr.kind == FileType::Symlink {
                let bytes = vol
                    .readlink(ino)
                    .map_err(|err| anyhow!("XFS readlink: errno {err}"))?;
                visitor(&bytes)?;
                return Ok(bytes.len() as u64);
            }
            if attr.kind != FileType::RegularFile {
                anyhow::bail!("XFS entry {owned:?} is not a regular file");
            }
            let chunk = chunk_size.max(STREAM_FLOOR);
            let mut offset: u64 = 0;
            while offset < attr.size {
                let want = (attr.size - offset)
                    .min(chunk as u64)
                    .min(u64::from(u32::MAX)) as u32;
                let bytes = vol
                    .read(ino, offset as i64, want)
                    .map_err(|err| anyhow!("XFS read {owned:?}@{offset}: errno {err}"))?;
                if bytes.len() != want as usize {
                    anyhow::bail!("XFS short read at {offset}");
                }
                visitor(&bytes)?;
                offset += bytes.len() as u64;
            }
            Ok(offset)
        })
    }

    pub fn file_stat(&self, path: impl AsRef<[u8]>, node_id: &str) -> Result<FsStat> {
        let path = path.as_ref();
        if path.is_empty() {
            anyhow::bail!("cannot stat XFS volume root as a file");
        }
        let owned = path.to_vec();
        self.with_volume(|vol| {
            let ino = lookup_path_resilient(vol, &owned)
                .map_err(|err| anyhow!("XFS lookup_path {owned:?}: errno {err}"))?;
            let attr = vol
                .getattr(ino)
                .map_err(|err| anyhow!("XFS getattr {owned:?}: errno {err}"))?;
            let mut sample = Vec::new();
            match attr.kind {
                FileType::RegularFile => {
                    let want = (SAMPLE_BYTES as u64).min(attr.size) as u32;
                    if want > 0 {
                        sample = vol
                            .read(ino, 0, want)
                            .map_err(|err| anyhow!("XFS sample read: errno {err}"))?;
                    }
                }
                FileType::Symlink => {
                    sample = vol
                        .readlink(ino)
                        .map_err(|err| anyhow!("XFS link sample: errno {err}"))?;
                }
                _ => {}
            }
            Ok(FsStat {
                node_id: node_id.to_string(),
                size: attr.size,
                inode: Some(ino),
                mode: Some(attr.mode),
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
                data_runs: if attr.kind == FileType::RegularFile {
                    let mut runs = Vec::new();
                    for extent in vol
                        .extents(ino)
                        .map_err(|err| anyhow!("XFS extent map: errno {err}"))?
                    {
                        if extent.unwritten || extent.logical_offset >= attr.size {
                            continue;
                        }
                        runs.push(FsDataRun {
                            logical_offset: extent.logical_offset,
                            physical_offset: self
                                .base_offset
                                .checked_add(extent.physical_offset)
                                .ok_or_else(|| {
                                anyhow!("XFS physical offset overflow")
                            })?,
                            length: extent.length.min(attr.size - extent.logical_offset),
                        });
                    }
                    physical_runs(self.layout.as_ref(), runs)?
                } else {
                    Vec::new()
                },
            })
        })
    }

    /// Seekable reads use the same validated extent plan as full/streamed reads.
    pub fn read_file_range(
        &self,
        path: impl AsRef<[u8]>,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let path = path.as_ref();
        self.with_volume(|volume| {
            let inode = lookup_path_resilient(volume, path)
                .map_err(|err| anyhow!("XFS lookup: errno {err}"))?;
            let attributes = volume
                .getattr(inode)
                .map_err(|err| anyhow!("XFS getattr: errno {err}"))?;
            if attributes.kind == FileType::Symlink {
                let bytes = volume
                    .readlink(inode)
                    .map_err(|err| anyhow!("XFS readlink: errno {err}"))?;
                let start = offset.min(bytes.len() as u64) as usize;
                return Ok(bytes[start..start.saturating_add(length).min(bytes.len())].to_vec());
            }
            if attributes.kind != FileType::RegularFile {
                anyhow::bail!("XFS entry is not a regular file");
            }
            let total = attributes.size.saturating_sub(offset).min(length as u64) as usize;
            let mut output = Vec::with_capacity(total);
            while output.len() < total {
                let current = offset
                    .checked_add(output.len() as u64)
                    .ok_or_else(|| anyhow!("XFS read offset overflow"))?;
                let current = i64::try_from(current)
                    .map_err(|_| anyhow!("XFS read offset exceeds format limit"))?;
                let want = (total - output.len()).min(1 << 20) as u32;
                let bytes = volume
                    .read(inode, current, want)
                    .map_err(|err| anyhow!("XFS range read: errno {err}"))?;
                if bytes.len() != want as usize {
                    anyhow::bail!("XFS short range read");
                }
                output.extend_from_slice(&bytes);
            }
            Ok(output)
        })
    }

    pub fn extended_attributes(
        &self,
        path: impl AsRef<[u8]>,
    ) -> Result<Vec<xfs_fuse::ExtendedAttribute>> {
        self.with_volume(|volume| {
            let inode = lookup_path_resilient(volume, path.as_ref())
                .map_err(|err| anyhow!("XFS lookup: errno {err}"))?;
            let mut attributes = volume
                .extended_attributes(inode)
                .map_err(|err| anyhow!("XFS attributes: errno {err}"))?;
            for attribute in &mut attributes {
                let runs = attribute
                    .extents
                    .iter()
                    .filter(|extent| extent.length > 0)
                    .map(|extent| {
                        Ok(FsDataRun {
                            logical_offset: extent.logical_offset,
                            physical_offset: self
                                .base_offset
                                .checked_add(extent.physical_offset)
                                .ok_or_else(|| {
                                anyhow!("XFS attribute physical offset overflow")
                            })?,
                            length: extent.length,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                attribute.extents = physical_runs(self.layout.as_ref(), runs)?
                    .into_iter()
                    .map(|run| xfs_fuse::Extent {
                        logical_offset: run.logical_offset,
                        physical_offset: run.physical_offset,
                        length: run.length,
                        unwritten: false,
                    })
                    .collect();
            }
            Ok(attributes)
        })
    }

    /// Retains ownership, link count, device number, inode version and full POSIX mode.
    pub fn inode_metadata(&self, path: impl AsRef<[u8]>) -> Result<xfs_fuse::FileAttr> {
        self.with_volume(|volume| {
            let inode = lookup_path_resilient(volume, path.as_ref())
                .map_err(|err| anyhow!("XFS lookup: errno {err}"))?;
            volume
                .getattr(inode)
                .map_err(|err| anyhow!("XFS inode metadata: errno {err}"))
        })
    }

    pub fn volume_metadata(&self) -> Result<xfs_fuse::Sb> {
        self.with_volume(|volume| Ok(*volume.superblock()))
    }

    fn node_id(&self, path: &[u8]) -> String {
        if path.is_empty() {
            return self.root_parent_id.clone();
        }
        let hex = hex::encode(path);
        match self.source {
            XfsSource::Partition(part_id) => format!("xfs:part:{part_id}:{hex}"),
            XfsSource::Lvm { part_id, lv_index } => {
                format!("xfs:lvm:{part_id}:{lv_index}:{hex}")
            }
        }
    }

    /// Runs `op` against the parsed `xfs_fuse::Volume` held by this volume.
    /// Built once in [`XfsVolume::open`]; reads address absolute offsets, so
    /// reusing one cursor across calls is safe for read-only access. A
    /// poisoned mutex (a prior panic mid-read) surfaces as an error.
    fn with_volume<R>(
        &self,
        op: impl FnOnce(&mut Volume<PartitionCursor>) -> Result<R>,
    ) -> Result<R> {
        let mut volume = self
            .volume
            .lock()
            .map_err(|_| anyhow!("XFS volume mutex poisoned"))?;
        op(&mut volume)
    }
}

fn lookup_path_resilient(
    vol: &mut Volume<PartitionCursor>,
    path: &[u8],
) -> std::result::Result<XfsIno, i32> {
    match vol.lookup_path_bytes(path) {
        Ok(ino) => Ok(ino),
        Err(fast_error) => {
            let fallback = lookup_path_by_readdir(vol, path);
            if fallback.is_ok() {
                tracing::debug!(
                    "XFS lookup_path {path:?} failed with errno {fast_error}; resolved via readdir"
                );
            }
            fallback.map_err(|_| fast_error)
        }
    }
}

fn lookup_path_by_readdir(
    vol: &mut Volume<PartitionCursor>,
    path: &[u8],
) -> std::result::Result<XfsIno, i32> {
    let mut current = vol.lookup_path("")?;
    for component in path
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty())
    {
        let target = component;
        let entries = vol.readdir(current)?;
        let Some(entry) = entries.into_iter().find(|entry| entry.name == target) else {
            return vol.lookup_path_bytes(path);
        };
        current = entry.ino;
    }
    Ok(current)
}

fn read_file_chunks(
    vol: &mut Volume<PartitionCursor>,
    ino: XfsIno,
    size: u64,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let total = size.min(max_bytes as u64) as usize;
    let mut out = Vec::with_capacity(total);
    let chunk: usize = 1 << 20; // 1 MiB
    while out.len() < total {
        let want = (total - out.len()).min(chunk).min(u32::MAX as usize) as u32;
        let bytes = vol
            .read(ino, out.len() as i64, want)
            .map_err(|err| anyhow!("XFS read errno {err}"))?;
        if bytes.len() != want as usize {
            anyhow::bail!("XFS short read at {}", out.len());
        }
        out.extend_from_slice(&bytes);
    }
    out.truncate(total);
    Ok(out)
}

fn node_kind_from_filetype(kind: FileType) -> NodeKind {
    match kind {
        FileType::Directory => NodeKind::Dir,
        FileType::Symlink => NodeKind::Symlink,
        _ => NodeKind::File,
    }
}

fn attr_timestamps(attr: &xfs_fuse::FileAttr) -> Mactb {
    Mactb {
        modified: raw_time_to_rfc3339(attr.raw_mtime),
        accessed: raw_time_to_rfc3339(attr.raw_atime),
        created: attr.raw_crtime.and_then(raw_time_to_rfc3339),
        mft_changed: raw_time_to_rfc3339(attr.raw_ctime),
        birth: attr.raw_crtime.and_then(raw_time_to_rfc3339),
    }
}

fn raw_time_to_rfc3339((seconds, nanoseconds): (i64, u32)) -> Option<String> {
    DateTime::<Utc>::from_timestamp(seconds, nanoseconds).map(|time| time.to_rfc3339())
}

fn join_unix(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = parent.to_vec();
    if !path.ends_with(b"/") {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}

/// Cursor that exposes a partition slice of `DiskImage` as `std::io::Read +
/// Seek`. Read-only by contract; the inner `image.read_range` returns owned
/// copies, so there's no aliasing risk.
pub struct PartitionCursor {
    image: Arc<DiskImage>,
    layout: Option<LinearLayout>,
    base_offset: u64,
    length_bytes: u64,
    position: u64,
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.length_bytes || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length_bytes - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let bytes = read_backing_range(
            &self.image,
            self.layout.as_ref(),
            self.base_offset
                .checked_add(self.position)
                .ok_or_else(|| io::Error::other("XFS backing offset overflow"))?,
            want,
        )
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
        if target < 0 || target > u64::MAX as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "XFS cursor seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests;
