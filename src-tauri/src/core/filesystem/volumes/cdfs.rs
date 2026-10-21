//! Read-only CDFS / ISO 9660 volume adapter built on the vendored `cdfs`
//! crate.
//!
//! Each operation opens a transient `ISO9660` reader over the partition slice.
//! ISO images tend to have shallow, compact metadata, and the crate stores its
//! reader behind `Rc<RefCell<...>>`, so keeping the adapter per-call avoids
//! sharing a non-`Send` handle across IPC worker threads.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `cdfs:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly; the partition node itself stands
//! in for the root.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use cdfs::{DirectoryEntry, ExtraAttributes, ISO9660};
use time::format_description::well_known::Rfc3339;

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::evidence::partitions::PartitionSlice;
use crate::core::evidence::readers::optical_disc::DataTrackLayout;
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum CdfsSource {
    Partition(u8),
}

/// Parse a `cdfs:part:{N}:{hex(path)}` ID. Returns the source plus the decoded
/// UTF-8 path, with a leading `/`, or empty for the volume root.
pub fn parse_cdfs_id(id: &str) -> Option<(CdfsSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("cdfs"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((CdfsSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_cdfs_source(image: &Arc<DiskImage>, source: CdfsSource) -> Result<CdfsVolume> {
    match source {
        CdfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            CdfsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct CdfsVolume {
    image: Arc<DiskImage>,
    partition_id: u8,
    base_offset: u64,
    length_bytes: u64,
    raw_layout: Option<DataTrackLayout>,
}

impl CdfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .context("CDFS base offset overflow")?;
        let physical_length = partition
            .length_lba
            .checked_mul(sector_size)
            .context("CDFS partition length overflow")?;
        ensure!(
            base_offset
                .checked_add(physical_length)
                .is_some_and(|end| end <= image.len()),
            "CDFS partition exceeds source"
        );
        let slice = PartitionSlice::new(image.as_ref(), base_offset, physical_length);
        let raw_layout = if image.optical_track_sector_size(partition.id) == Some(2336) {
            DataTrackLayout::probe_xa2336(&slice)?
        } else {
            match DataTrackLayout::probe(&slice)? {
                Some(layout) => Some(layout),
                None => DataTrackLayout::probe_cooked(&slice)?,
            }
        };
        let volume = Self {
            image,
            partition_id: partition.id,
            base_offset,
            length_bytes: raw_layout.map_or(physical_length, DataTrackLayout::logical_length),
            raw_layout,
        };
        volume.with_iso(|_| Ok(()))?;
        Ok(volume)
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.node_id(path);
        let parent_path = path.to_string();
        self.with_iso(move |iso| {
            let directory = if parent_path.is_empty() {
                DirectoryEntry::Directory(iso.root().clone())
            } else {
                iso.open(&parent_path)
                    .map_err(|err| anyhow!("CDFS open directory {parent_path}: {err}"))?
                    .ok_or_else(|| anyhow!("CDFS directory not found: {parent_path}"))?
            };
            let DirectoryEntry::Directory(directory) = directory else {
                anyhow::bail!("CDFS entry {parent_path} is not a directory");
            };

            let mut out = Vec::new();
            for entry in directory.contents() {
                let entry = entry.map_err(|err| anyhow!("CDFS list {parent_path}: {err}"))?;
                let name = entry.identifier();
                if name == "." || name == ".." || name.is_empty() {
                    continue;
                }
                if entry.relocated() {
                    continue;
                }
                let child_path = join_path(&parent_path, name);
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name: name.to_string(),
                    parent_id: Some(parent_id.clone()),
                    kind: entry_kind(&entry),
                    size: entry_size(&entry),
                    deleted: false,
                    inode: entry.inode().map(u64::from),
                    timestamps: entry_timestamps(&entry),
                    link_target: None,
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
            anyhow::bail!("cannot read CDFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_iso(move |iso| {
            let entry = iso
                .open(&owned)
                .map_err(|err| anyhow!("CDFS open file {owned}: {err}"))?
                .ok_or_else(|| anyhow!("CDFS file not found: {owned}"))?;
            match entry {
                DirectoryEntry::File(file) => {
                    let total = (file.size() as usize).min(max_bytes);
                    let mut reader = file.read();
                    let mut out = Vec::with_capacity(total);
                    read_into_vec(&mut reader, &mut out, total, &owned)?;
                    Ok(out)
                }
                DirectoryEntry::Symlink(link) => Ok(link
                    .target()
                    .map(|target| target.as_bytes().to_vec())
                    .unwrap_or_default()),
                DirectoryEntry::Directory(_) => {
                    anyhow::bail!("CDFS entry {owned} is not a regular file")
                }
            }
        })
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream CDFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_iso(move |iso| {
            let entry = iso
                .open(&owned)
                .map_err(|err| anyhow!("CDFS open file {owned}: {err}"))?
                .ok_or_else(|| anyhow!("CDFS file not found: {owned}"))?;
            match entry {
                DirectoryEntry::File(file) => {
                    let mut reader = file.read();
                    let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
                    let mut total = 0u64;
                    loop {
                        let read = reader
                            .read(&mut buf)
                            .map_err(|err| anyhow!("CDFS stream {owned}: {err}"))?;
                        if read == 0 {
                            break;
                        }
                        visitor(&buf[..read])?;
                        total += read as u64;
                    }
                    ensure!(
                        total == u64::from(file.size()),
                        "CDFS stream {owned} ended before its logical length"
                    );
                    Ok(total)
                }
                DirectoryEntry::Symlink(link) => {
                    let bytes = link
                        .target()
                        .map(|target| target.as_bytes())
                        .unwrap_or_default();
                    if !bytes.is_empty() {
                        visitor(bytes)?;
                    }
                    Ok(bytes.len() as u64)
                }
                DirectoryEntry::Directory(_) => {
                    anyhow::bail!("CDFS entry {owned} is not a regular file")
                }
            }
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat CDFS volume root as a file");
        }
        let owned = path.to_string();
        let node_id = node_id.to_string();
        self.with_iso(move |iso| {
            let entry = iso
                .open(&owned)
                .map_err(|err| anyhow!("CDFS stat {owned}: {err}"))?
                .ok_or_else(|| anyhow!("CDFS entry not found: {owned}"))?;
            let (size, sample, is_file) = match &entry {
                DirectoryEntry::File(file) => {
                    let total = file.size() as u64;
                    let want = SAMPLE_BYTES.min(file.size() as usize);
                    let mut sample = Vec::with_capacity(want);
                    if want > 0 {
                        read_into_vec(&mut file.read(), &mut sample, want, &owned)?;
                    }
                    (total, sample, true)
                }
                DirectoryEntry::Symlink(link) => {
                    let bytes = link
                        .target()
                        .map(|target| target.as_bytes().to_vec())
                        .unwrap_or_default();
                    (bytes.len() as u64, bytes, true)
                }
                DirectoryEntry::Directory(_) => (0, Vec::new(), false),
            };
            if !is_file {
                anyhow::bail!("CDFS entry {owned} is not a regular file");
            }
            Ok(FsStat {
                node_id,
                size,
                inode: entry.inode().map(u64::from),
                mode: entry.mode().map(|mode| (mode.bits() & 0xFFFF) as u16),
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
                data_runs: Vec::<FsDataRun>::new(),
            })
        })
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        ensure!(!path.is_empty(), "cannot read CDFS volume root as a file");
        self.with_iso(|iso| {
            let entry = iso
                .open(path)
                .map_err(|error| anyhow!("CDFS open {path}: {error}"))?
                .ok_or_else(|| anyhow!("CDFS entry not found: {path}"))?;
            match entry {
                DirectoryEntry::File(file) => {
                    let count =
                        (length as u64).min(u64::from(file.size()).saturating_sub(offset)) as usize;
                    if count == 0 {
                        return Ok(Vec::new());
                    }
                    let mut reader = file.read();
                    reader.seek(SeekFrom::Start(offset))?;
                    let mut bytes = Vec::with_capacity(count);
                    read_into_vec(&mut reader, &mut bytes, count, path)?;
                    Ok(bytes)
                }
                DirectoryEntry::Symlink(link) => {
                    let bytes = link
                        .target()
                        .map(String::as_str)
                        .unwrap_or_default()
                        .as_bytes();
                    let start = offset.min(bytes.len() as u64) as usize;
                    let end = start.saturating_add(length).min(bytes.len());
                    Ok(bytes[start..end].to_vec())
                }
                DirectoryEntry::Directory(_) => {
                    anyhow::bail!("CDFS entry {path} is not a regular file")
                }
            }
        })
    }

    fn node_id(&self, path: &str) -> String {
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "cdfs:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    fn with_iso<R>(&self, op: impl FnOnce(&ISO9660<PartitionCursor>) -> Result<R>) -> Result<R> {
        let cursor = PartitionCursor {
            image: Arc::clone(&self.image),
            base_offset: self.base_offset,
            length_bytes: self.length_bytes,
            position: 0,
            raw_layout: self.raw_layout,
        };
        let iso = ISO9660::new(cursor)
            .map_err(|err| anyhow!("CDFS open partition {}: {err}", self.partition_id))?;
        op(&iso)
    }
}

fn read_into_vec(
    reader: &mut impl Read,
    out: &mut Vec<u8>,
    max_bytes: usize,
    path: &str,
) -> Result<()> {
    let mut buf = vec![0u8; STREAM_FLOOR.min(max_bytes.max(1))];
    while out.len() < max_bytes {
        let want = (max_bytes - out.len()).min(buf.len());
        let read = reader
            .read(&mut buf[..want])
            .map_err(|err| anyhow!("CDFS read {path}: {err}"))?;
        if read == 0 {
            anyhow::bail!("CDFS read {path} ended before its logical length");
        }
        out.extend_from_slice(&buf[..read]);
    }
    Ok(())
}

fn entry_kind(entry: &DirectoryEntry<PartitionCursor>) -> NodeKind {
    match entry {
        DirectoryEntry::Directory(_) => NodeKind::Dir,
        DirectoryEntry::File(_) => NodeKind::File,
        DirectoryEntry::Symlink(_) => NodeKind::Symlink,
    }
}

fn entry_size(entry: &DirectoryEntry<PartitionCursor>) -> u64 {
    match entry {
        DirectoryEntry::File(file) => file.size() as u64,
        DirectoryEntry::Symlink(link) => {
            link.target().map(|target| target.len() as u64).unwrap_or(0)
        }
        DirectoryEntry::Directory(_) => 0,
    }
}

fn entry_timestamps(entry: &DirectoryEntry<PartitionCursor>) -> Mactb {
    let modified = offset_to_rfc3339(entry.modify_time());
    let accessed = offset_to_rfc3339(entry.access_time());
    let created = offset_to_rfc3339(entry.create_time());
    let changed = offset_to_rfc3339(entry.attribute_change_time());
    Mactb {
        modified: modified.clone(),
        accessed,
        created: created.clone(),
        mft_changed: changed,
        birth: created.or(modified),
    }
}

fn offset_to_rfc3339(datetime: time::OffsetDateTime) -> Option<String> {
    datetime.format(&Rfc3339).ok()
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

struct PartitionCursor {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
    position: u64,
    raw_layout: Option<DataTrackLayout>,
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // cdfs's block reader requires a complete logical block. Reject EOF
        // here so it cannot reuse uninitialised/stale bytes after a short read.
        if self
            .position
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.length_bytes)
        {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "CDFS logical block exceeds volume",
            ));
        }
        let bytes = if let Some(layout) = self.raw_layout {
            let source = PartitionSlice::new(
                self.image.as_ref(),
                self.base_offset,
                layout.physical_length(),
            );
            layout.read(&source, self.position, buf.len())
        } else {
            self.base_offset
                .checked_add(self.position)
                .context("CDFS read offset overflow")
                .and_then(|offset| self.image.read_range(offset, buf.len()))
        }
        .map_err(|err| io::Error::other(err.to_string()))?;
        if bytes.len() != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "CDFS source short read",
            ));
        }
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
        self.position = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "CDFS seek outside address space",
            )
        })?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_real_cdfs_root_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_CDFS_TEST") else {
            return;
        };
        let image = Arc::new(DiskImage::open(path).expect("CDFS image should open"));
        let cdfs_partition = image
            .partitions
            .iter()
            .find(|p| p.kind == crate::core::evidence::image::PartitionKind::Cdfs)
            .cloned()
            .expect("expected at least one CDFS partition");
        let volume = CdfsVolume::open(Arc::clone(&image), &cdfs_partition).expect("open CDFS");
        let children = volume.root_children().expect("list CDFS root");
        assert!(!children.is_empty(), "CDFS root should expose entries");
        if let Some(first_file) = children.iter().find(|node| node.kind == NodeKind::File) {
            let path = format!("/{}", first_file.name);
            let bytes = volume.read_file(&path, 8192).expect("CDFS file read");
            assert!(!bytes.is_empty(), "CDFS file should yield bytes");
        }
    }
}
