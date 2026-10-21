//! Read-only SquashFS v3/v4 volume adapter built on the vendored `backhand` crate.
//!
//! The `backhand::FilesystemReader` is built once in [`SquashfsVolume::open`]
//! and reused for every operation — its reads take `&self` (backhand guards
//! the inner reader itself), so no per-volume lock is needed here. Parsing the
//! superblock decompresses the whole inode + directory tables, which is non-
//! trivial for large rootfs images, so a whole-volume snapshot walk would
//! otherwise pay that per directory; holding the reader (and caching the
//! `SquashfsVolume` itself in [`crate::core::filesystem::tree::fs_tree::FsSession`]) keeps
//! it to one parse per walk.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `squashfs:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the
//! volume root and is never emitted directly — the partition node itself
//! stands in for the root.
//! UBI-backed sources instead use `squashfs:ubi:part:{N}:{volume}:{hex(path)}`;
//! their empty-path directory is a child of the containing UBI partition.

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use backhand::traits::filesystem::{
    BackhandInnerNode as InnerNode, BackhandNode as Node, BackhandNodeHeader as NodeHeader,
};
use backhand::FilesystemReaderTrait;
#[path = "squashfs_reader.rs"]
mod reader;
use chrono::{TimeZone, Utc};
use reader::open_reader;

#[cfg(test)]
#[path = "squashfs_lzma_tests.rs"]
mod lzma_tests;

use crate::core::evidence::image::find_partition;
use crate::core::evidence::image::{DiskImage, Partition};
use crate::core::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;
/// Cap how much of the directory tree we surface to the UI in one listing.
/// SquashFS images can hold tens of thousands of nodes; the UI never paginates
/// past this, so anything beyond is silently truncated.
const MAX_CHILDREN_PER_DIR: usize = 50_000;

#[derive(Debug, Clone, Copy)]
pub enum SquashfsSource {
    Partition(u8),
    Ubi { part_id: u8, volume_id: u32 },
}

impl SquashfsSource {
    pub fn cache_key(self) -> String {
        match self {
            Self::Partition(part_id) => format!("squashfs:part:{part_id}"),
            Self::Ubi { part_id, volume_id } => format!("squashfs:ubi:part:{part_id}:{volume_id}"),
        }
    }

    pub fn node_id(self, path: &str) -> String {
        match self {
            Self::Partition(part_id) if path.is_empty() => format!("part:{part_id}"),
            Self::Partition(_) | Self::Ubi { .. } => {
                format!("{}:{}", self.cache_key(), hex::encode(path.as_bytes()))
            }
        }
    }
}

/// Parse a `squashfs:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_squashfs_id(id: &str) -> Option<(SquashfsSource, String)> {
    if let Some(rest) = id.strip_prefix("squashfs:ubi:part:") {
        let mut parts = rest.splitn(3, ':');
        let part_id = parts.next()?.parse().ok()?;
        let volume_id = parts.next()?.parse().ok()?;
        let path = String::from_utf8(hex::decode(parts.next()?).ok()?).ok()?;
        return Some((SquashfsSource::Ubi { part_id, volume_id }, path));
    }
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("squashfs"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((SquashfsSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_squashfs_source(
    image: &Arc<DiskImage>,
    source: SquashfsSource,
) -> Result<SquashfsVolume> {
    match source {
        SquashfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            SquashfsVolume::open(Arc::clone(image), partition)
        }
        SquashfsSource::Ubi { part_id, volume_id } => {
            let ubi = crate::core::filesystem::volumes::ubi::open_ubi_source(image, part_id)?;
            let reader = BufReader::with_capacity(64 * 1024, ubi.reader(volume_id)?);
            let fs = open_reader(reader).map_err(|error| {
                anyhow!("SquashFS open UBI part:{part_id} volume:{volume_id}: {error}")
            })?;
            Ok(SquashfsVolume { source, fs })
        }
    }
}

pub struct SquashfsVolume {
    source: SquashfsSource,
    /// Parsed filesystem (inode + dir tables decompressed), built once in
    /// [`SquashfsVolume::open`] and reused. backhand's reads take `&self`, so
    /// no lock is needed. See the module header.
    fs: Box<dyn FilesystemReaderTrait>,
}

impl SquashfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("SquashFS base overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .ok_or_else(|| anyhow!("SquashFS length overflow"))?;
        anyhow::ensure!(
            base_offset
                .checked_add(length_bytes)
                .is_some_and(|end| end <= image.len()),
            "SquashFS partition exceeds source"
        );
        let cursor = PartitionCursor {
            image,
            base_offset,
            length_bytes,
            position: 0,
        };
        let reader = BufReader::with_capacity(64 * 1024, cursor);
        let fs = open_reader(reader)
            .map_err(|err| anyhow!("SquashFS open part:{}: {err}", partition.id))?;
        Ok(Self {
            source: SquashfsSource::Partition(partition.id),
            fs,
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Lists the direct children of `path` inside the volume. `path` uses
    /// Unix-style separators with a leading `/`, or is empty for the volume
    /// root.
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_path_buf = parent_pathbuf(path);
        let parent_id = self.node_id(path);
        self.with_fs(|fs| {
            let mut out = Vec::with_capacity(64);
            for node in fs.files() {
                if !is_direct_child(&node.fullpath, &parent_path_buf) {
                    continue;
                }
                let name = match node.fullpath.file_name().and_then(|n| n.to_str()) {
                    Some(name) if !name.is_empty() => name.to_string(),
                    _ => continue, // skip the synthetic root entry and any non-utf8 names
                };
                let child_path = join_unix(path, &name);
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name,
                    parent_id: Some(parent_id.clone()),
                    kind: node_kind_of(&node),
                    size: node_size(&node),
                    deleted: false,
                    inode: None,
                    timestamps: header_timestamps(&node.header),
                    link_target: match &node.inner {
                        InnerNode::Symlink { link } => Some(link.to_string_lossy().into_owned()),
                        _ => None,
                    },
                    has_snapshot: false,
                });
                if out.len() >= MAX_CHILDREN_PER_DIR {
                    break;
                }
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
            anyhow::bail!("cannot read SquashFS volume root as a file");
        }
        let target = PathBuf::from(path);
        self.with_fs(move |fs| {
            let node = find_node(fs, &target)?;
            match &node.inner {
                InnerNode::File(file) => Ok(fs.file_data_range(file, 0, max_bytes)?),
                InnerNode::Symlink { link } => Ok(link.to_string_lossy().into_owned().into_bytes()),
                _ => anyhow::bail!("SquashFS entry {path} is not a regular file"),
            }
        })
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let node = find_node(self.fs.as_ref(), Path::new(path))?;
        match &node.inner {
            InnerNode::File(file) => Ok(self.fs.file_data_range(file, offset, length)?),
            InnerNode::Symlink { link } => {
                let bytes = link.to_string_lossy().into_owned().into_bytes();
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                Ok(bytes[start..start.saturating_add(length).min(bytes.len())].to_vec())
            }
            _ => anyhow::bail!("SquashFS entry {path} is not a regular file"),
        }
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream SquashFS volume root as a file");
        }
        let target = PathBuf::from(path);
        self.with_fs(move |fs| {
            let node = find_node(fs, &target)?;
            let InnerNode::File(file) = &node.inner else {
                anyhow::bail!("SquashFS entry {path} is not a regular file");
            };
            struct Visitor<'a, F>(&'a mut F, usize);
            impl<F: FnMut(&[u8]) -> Result<()>> io::Write for Visitor<'_, F> {
                fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                    for chunk in bytes.chunks(self.1) {
                        (self.0)(chunk).map_err(io::Error::other)?;
                    }
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let mut writer = Visitor(&mut visitor, chunk_size.max(STREAM_FLOOR));
            let total = fs.file_data_to_writer(file, &mut writer)?;
            anyhow::ensure!(
                total == file.file_len() as u64,
                "SquashFS short file stream: {path}"
            );
            Ok(total)
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat SquashFS volume root as a file");
        }
        let target = PathBuf::from(path);
        self.with_fs(move |fs| {
            let node = find_node(fs, &target)?;
            let (size, sample) = match &node.inner {
                InnerNode::File(file) => {
                    let sample = fs.file_data_range(file, 0, SAMPLE_BYTES)?;
                    (file.file_len() as u64, sample)
                }
                InnerNode::Symlink { link } => {
                    let bytes = link.to_string_lossy().into_owned().into_bytes();
                    let len = bytes.len() as u64;
                    (len, bytes)
                }
                _ => anyhow::bail!("SquashFS entry {path} is not a regular file"),
            };
            Ok(FsStat {
                node_id: node_id.to_string(),
                size,
                inode: None,
                mode: Some(node.header.permissions),
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
                // SquashFS stores files as a list of compressed blocks plus an
                // optional fragment; the uncompressed bytes do not have a
                // single contiguous physical range. Surface an empty data-run
                // list rather than fabricating compressed-block offsets.
                data_runs: Vec::<FsDataRun>::new(),
            })
        })
    }

    fn node_id(&self, path: &str) -> String {
        self.source.node_id(path)
    }

    /// Runs `op` against the parsed filesystem held by this volume. Built once
    /// in [`SquashfsVolume::open`]; backhand serialises access to its inner
    /// reader internally, so a shared borrow is sufficient.
    fn with_fs<R>(&self, op: impl FnOnce(&dyn FilesystemReaderTrait) -> Result<R>) -> Result<R> {
        op(self.fs.as_ref())
    }
}

fn parent_pathbuf(path: &str) -> PathBuf {
    if path.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(path)
    }
}

/// True when `candidate` lives directly under `parent` — i.e. its parent
/// component equals `parent`. backhand stores every node's `fullpath` rooted
/// at `/`, so `/etc` has parent `/` and `/etc/passwd` has parent `/etc`.
fn is_direct_child(candidate: &Path, parent: &Path) -> bool {
    matches!(candidate.parent(), Some(p) if p == parent)
}

fn join_unix(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn find_node(fs: &dyn FilesystemReaderTrait, target: &Path) -> Result<Node> {
    fs.files()
        .find(|node| node.fullpath == target)
        .ok_or_else(|| anyhow!("SquashFS entry not found: {}", target.display()))
}

fn node_kind_of(node: &Node) -> NodeKind {
    match &node.inner {
        InnerNode::Dir => NodeKind::Dir,
        InnerNode::File(_) => NodeKind::File,
        InnerNode::Symlink { .. } => NodeKind::Symlink,
        // Special files (devices, fifos, sockets) are surfaced as the generic
        // "file" bucket so they show up in the tree but cannot be opened.
        InnerNode::CharacterDevice { .. }
        | InnerNode::BlockDevice { .. }
        | InnerNode::NamedPipe
        | InnerNode::Socket => NodeKind::File,
    }
}

fn node_size(node: &Node) -> u64 {
    match &node.inner {
        InnerNode::File(file) => file.file_len() as u64,
        InnerNode::Symlink { link } => link.as_os_str().len() as u64,
        _ => 0,
    }
}

fn header_timestamps(header: &NodeHeader) -> Mactb {
    let modified = Utc
        .timestamp_opt(header.mtime as i64, 0)
        .single()
        .map(|dt| dt.to_rfc3339());
    Mactb {
        modified: modified.clone(),
        accessed: None,
        created: None,
        mft_changed: None,
        birth: None,
    }
}

/// Cursor that exposes a partition slice of `DiskImage` as `std::io::Read +
/// Seek`. backhand wraps it in `BufReader` for the `BufRead` half of the
/// `BufReadSeek` bound.
struct PartitionCursor {
    image: Arc<DiskImage>,
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
        let offset = self
            .base_offset
            .checked_add(self.position)
            .ok_or_else(|| io::Error::other("SquashFS cursor overflow"))?;
        let decoded = match &self.image.firmware {
            Some(firmware) => firmware
                .read_plaintext(offset, want)
                .map_err(io::Error::other)?,
            None => None,
        };
        let bytes = match decoded {
            Some(bytes) => bytes,
            None => self
                .image
                .read_range(offset, want)
                .map_err(io::Error::other)?,
        };
        if bytes.len() != want {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "SquashFS source short read",
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
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SquashFS cursor seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the adapter against a real SquashFS image when the path is
    /// supplied via `LOVELYCARVE_SQUASHFS_TEST`. Walks the root, samples
    /// the first regular file, and confirms `read_file` returns bytes.
    #[test]
    fn lists_real_squashfs_root_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_SQUASHFS_TEST") else {
            return;
        };
        let image = Arc::new(DiskImage::open(path).expect("squashfs image should open"));
        let squashfs_partition = image
            .partitions
            .iter()
            .find(|p| p.kind == crate::core::evidence::image::PartitionKind::Squashfs)
            .cloned()
            .expect("expected at least one SquashFS partition");
        let volume =
            SquashfsVolume::open(Arc::clone(&image), &squashfs_partition).expect("open SquashFS");
        let children = volume.root_children().expect("list SquashFS root");
        assert!(!children.is_empty(), "SquashFS root should expose entries");
        if let Some(first_file) = children.iter().find(|node| node.kind == NodeKind::File) {
            let path = format!("/{}", first_file.name);
            let bytes = volume.read_file(&path, 8192).expect("SquashFS file read");
            assert!(!bytes.is_empty(), "SquashFS file should yield bytes");
        }
    }
}
