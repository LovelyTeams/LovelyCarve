//! Read-only UFS2 volume adapter built on the vendored `rufs` crate.
//!
//! The parsed `rufs::Ufs` is built once in [`UfsVolume::open`] and reused for
//! every operation (behind a `Mutex`, since `rufs` reads take `&mut`). Re-
//! parsing the superblock + cylinder-group sanity check costs ~30–60 ms on a
//! cold image, so a whole-volume snapshot walk would otherwise pay that per
//! directory; holding the handle (and caching the `UfsVolume` itself in
//! [`crate::filesystem::tree::fs_tree::FsSession`]) keeps it to one parse per walk.
//! Live IPC calls still open their own `UfsVolume`, so the mutex is
//! uncontended outside the single-threaded walker.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex bytes:
//! `ufs:part:{N}:{hex(/path/to/entry)}`. The empty-hex form denotes the volume
//! root and is never emitted directly — the partition node itself stands in
//! for the root.

use std::ffi::OsStr;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use chrono::DateTime;
use rufs::{BlockReader, InodeAttr, InodeNum, InodeType, Ufs};

use crate::evidence::image::find_partition;
use crate::evidence::image::{DiskImage, Partition};
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};

const SAMPLE_BYTES: usize = 4096;
const STREAM_FLOOR: usize = 64 * 1024;
/// Block size handed to `BlockReader::new` for the cursor-backed `Ufs`.
/// rufs uses this purely as the read-ahead chunk size; UFS2 typically uses
/// 32 KiB filesystem blocks, so 64 KiB keeps a comfortable margin while still
/// being small enough to stream a multi-GB file without retaining a large
/// buffer in memory.
const RUFS_BLOCK_SIZE: usize = 64 * 1024;
/// Cap how much of a directory we surface to the UI in one listing. UFS
/// directories are flat byte streams of `Header` records; very large
/// directories on real installs (e.g. a `Maildir` cur/) can hold tens of
/// thousands of entries — beyond this we stop and let the user paginate via a
/// future query API.
const MAX_CHILDREN_PER_DIR: usize = 50_000;

#[derive(Debug, Clone, Copy)]
pub enum UfsSource {
    Partition(u8),
}

/// Parse a `ufs:part:{N}:{hex(path)}` ID. Returns the source plus the decoded
/// UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_ufs_id(id: &str) -> Option<(UfsSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("ufs"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((UfsSource::Partition(part_id), path))
        }
        _ => None,
    }
}

pub fn open_ufs_source(image: &Arc<DiskImage>, source: UfsSource) -> Result<UfsVolume> {
    match source {
        UfsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            UfsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct UfsVolume {
    partition_id: u8,
    /// The parsed `rufs::Ufs` is built once in [`UfsVolume::open`] and reused
    /// for every operation behind a mutex. Re-parsing the superblock + every
    /// cylinder group costs ~30–60 ms (see module header); a whole-volume
    /// snapshot walk would otherwise pay that per directory. `rufs` reads take
    /// `&mut Ufs`, so the handle lives behind a `Mutex` — uncontended in
    /// practice since each live IPC call still opens its own `UfsVolume`; only
    /// the single-threaded snapshot walker reuses one instance.
    fs: Mutex<Ufs<PartitionCursor>>,
}

impl UfsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition.start_lba * sector_size;
        let length_bytes = partition.length_lba * sector_size;
        let cursor = PartitionCursor {
            image,
            base_offset,
            length_bytes,
            position: 0,
        };
        let block_reader = BlockReader::new(cursor, RUFS_BLOCK_SIZE, false);
        let fs = Ufs::new(block_reader)
            .map_err(|err| anyhow!("UFS open part:{}: {err}", partition.id))?;
        Ok(Self {
            partition_id: partition.id,
            fs: Mutex::new(fs),
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Unallocated bytes in this volume, from the UFS superblock's free-block
    /// count (`bfree × bsize`). Exact; the superblock is parsed on open.
    pub fn free_bytes(&self) -> Result<u64> {
        self.with_fs(|fs| {
            let info = fs.info();
            Ok(info.bfree * info.bsize as u64)
        })
    }

    /// Lists the direct children of `path` inside the volume. `path` uses
    /// Unix-style separators with a leading `/`, or is empty for the volume
    /// root.
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.node_id(path);
        let parent_path = path.to_string();
        self.with_fs(|fs| {
            let inr = lookup_path(fs, path)?;
            let mut entries: Vec<DirEntry> = Vec::with_capacity(64);
            fs.dir_iter(inr, |name, child_inr, kind| {
                if entries.len() >= MAX_CHILDREN_PER_DIR {
                    return Some(()); // stop the iteration
                }
                let bytes = name.as_encoded_bytes();
                if bytes == b"." || bytes == b".." {
                    return None;
                }
                entries.push(DirEntry {
                    name: String::from_utf8_lossy(bytes).into_owned(),
                    inr: child_inr,
                    kind,
                });
                None::<()>
            })
            .map_err(|err| anyhow!("UFS dir_iter {path}: {err}"))?;

            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                let attr = match fs.inode_attr(entry.inr) {
                    Ok(attr) => Some(attr),
                    Err(err) => {
                        tracing::warn!(
                            "UFS inode_attr {} (in {parent_path}/{}): {err}",
                            entry.inr,
                            entry.name
                        );
                        None
                    }
                };
                let child_path = join_unix(&parent_path, &entry.name);
                let kind = match entry.kind {
                    InodeType::Directory => NodeKind::Dir,
                    InodeType::Symlink => NodeKind::Symlink,
                    _ => NodeKind::File,
                };
                let size = attr.as_ref().map(|a| a.size).unwrap_or(0);
                let timestamps = attr.as_ref().map(attr_timestamps).unwrap_or_default();
                out.push(FsNode {
                    id: self.node_id(&child_path),
                    name: entry.name,
                    parent_id: Some(parent_id.clone()),
                    kind,
                    size,
                    deleted: false,
                    inode: Some(entry.inr.get64()),
                    timestamps,
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
            anyhow::bail!("cannot read UFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_fs(move |fs| {
            let inr = lookup_path(fs, &owned)?;
            let attr = fs
                .inode_attr(inr)
                .map_err(|err| anyhow!("UFS inode_attr {owned}: {err}"))?;
            match attr.kind {
                InodeType::RegularFile => {
                    let total = (attr.size as usize).min(max_bytes);
                    let mut out = vec![0u8; total];
                    let mut filled = 0usize;
                    while filled < out.len() {
                        let read = fs
                            .inode_read(inr, filled as u64, &mut out[filled..])
                            .map_err(|err| anyhow!("UFS inode_read {owned}: {err}"))?;
                        if read == 0 {
                            break;
                        }
                        filled += read;
                    }
                    out.truncate(filled);
                    Ok(out)
                }
                InodeType::Symlink => {
                    let bytes = fs
                        .symlink_read(inr)
                        .map_err(|err| anyhow!("UFS symlink_read {owned}: {err}"))?;
                    Ok(bytes)
                }
                _ => anyhow::bail!("UFS entry {owned} is not a regular file"),
            }
        })
    }

    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream UFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_fs(move |fs| {
            let inr = lookup_path(fs, &owned)?;
            let attr = fs
                .inode_attr(inr)
                .map_err(|err| anyhow!("UFS inode_attr {owned}: {err}"))?;
            if attr.kind != InodeType::RegularFile {
                anyhow::bail!("UFS entry {owned} is not a regular file");
            }
            let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
            let mut offset = 0u64;
            let total = attr.size;
            while offset < total {
                let want = ((total - offset) as usize).min(buf.len());
                let read = fs
                    .inode_read(inr, offset, &mut buf[..want])
                    .map_err(|err| anyhow!("UFS inode_read {owned}@{offset}: {err}"))?;
                if read == 0 {
                    break;
                }
                visitor(&buf[..read])?;
                offset += read as u64;
            }
            Ok(offset)
        })
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat UFS volume root as a file");
        }
        let owned = path.to_string();
        self.with_fs(move |fs| {
            let inr = lookup_path(fs, &owned)?;
            let attr = fs
                .inode_attr(inr)
                .map_err(|err| anyhow!("UFS inode_attr {owned}: {err}"))?;
            let mut sample = Vec::new();
            if attr.kind == InodeType::RegularFile {
                let want = SAMPLE_BYTES.min(attr.size as usize);
                let mut buf = vec![0u8; want];
                let mut filled = 0usize;
                while filled < buf.len() {
                    let read = fs
                        .inode_read(inr, filled as u64, &mut buf[filled..])
                        .map_err(|err| anyhow!("UFS stat read {owned}: {err}"))?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                buf.truncate(filled);
                sample = buf;
            } else if attr.kind == InodeType::Symlink {
                if let Ok(link) = fs.symlink_read(inr) {
                    sample = link;
                }
            }
            Ok(FsStat {
                node_id: node_id.to_string(),
                size: attr.size,
                inode: Some(inr.get64()),
                mode: Some(attr.perm),
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
                // UFS files are described by direct + 1/2/3-level indirect
                // block pointers; surfacing them as `FsDataRun`s would require
                // walking every indirect block per file, which `rufs` does not
                // expose. Leave empty for now — see TODO in `data_runs`.
                data_runs: Vec::<FsDataRun>::new(),
            })
        })
    }

    fn node_id(&self, path: &str) -> String {
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "ufs:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    /// Runs `op` against the parsed `rufs::Ufs` held by this volume. The
    /// handle is built once in [`UfsVolume::open`]; reads seek to absolute
    /// offsets, so reusing one cursor across calls is safe for read-only
    /// access. A poisoned mutex (a prior panic mid-read) surfaces as an error.
    fn with_fs<R>(&self, op: impl FnOnce(&mut Ufs<PartitionCursor>) -> Result<R>) -> Result<R> {
        let mut fs = self
            .fs
            .lock()
            .map_err(|_| anyhow!("UFS volume part:{} mutex poisoned", self.partition_id))?;
        op(&mut fs)
    }
}

struct DirEntry {
    name: String,
    inr: InodeNum,
    kind: InodeType,
}

fn attr_timestamps(attr: &InodeAttr) -> Mactb {
    Mactb {
        modified: system_time_to_rfc3339(attr.mtime),
        accessed: system_time_to_rfc3339(attr.atime),
        created: system_time_to_rfc3339(attr.btime),
        mft_changed: system_time_to_rfc3339(attr.ctime),
        birth: system_time_to_rfc3339(attr.btime),
    }
}

fn system_time_to_rfc3339(t: std::time::SystemTime) -> Option<String> {
    DateTime::<chrono::Utc>::from(t).to_rfc3339().into()
}

/// Resolve a `/`-rooted UTF-8 path inside the UFS volume to an inode number
/// by walking each component through `dir_lookup` from the root inode (2).
fn lookup_path(fs: &mut Ufs<PartitionCursor>, path: &str) -> Result<InodeNum> {
    let mut current = InodeNum::ROOT;
    for component in path.split('/').filter(|c| !c.is_empty()) {
        let name = OsStr::new(component);
        current = fs
            .dir_lookup(current, name)
            .map_err(|err| anyhow!("UFS lookup {path}: {component}: {err}"))?;
    }
    Ok(current)
}

fn join_unix(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Cursor that exposes a partition slice of `DiskImage` as `std::io::Read +
/// Seek + Write`. `Write` is a no-op so any drop-time bookkeeping the rufs
/// `Decoder` tries to flush silently disappears — our access is read-only by
/// contract, regardless of what the upstream code path believes it is doing.
pub struct PartitionCursor {
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
                "UFS cursor seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

impl Write for PartitionCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Forensic read-only: pretend the write succeeded so the library's
        // drop-time bookkeeping cannot touch the underlying image. The bytes
        // are produced from `read_range`, which returns owned copies, so no
        // aliasing risk.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::image::PartitionKind;

    /// Drive the adapter against a real UFS image when the path is supplied
    /// via `LOVELYCARVE_UFS_TEST`. Walks the root, samples the first regular
    /// file, and confirms `read_file` returns bytes. Mirrors the
    /// `lists_real_squashfs_root_when_env_path_is_set` pattern.
    #[test]
    fn lists_real_ufs_root_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_UFS_TEST") else {
            return;
        };
        let image = Arc::new(DiskImage::open(path).expect("UFS image should open"));
        let ufs_partition = image
            .partitions
            .iter()
            .find(|p| p.kind == PartitionKind::Ufs)
            .cloned()
            .expect("expected at least one UFS partition");
        let volume = UfsVolume::open(Arc::clone(&image), &ufs_partition).expect("open UFS");
        let children = volume.root_children().expect("list UFS root");
        assert!(!children.is_empty(), "UFS root should expose entries");
        if let Some(first_file) = children.iter().find(|node| node.kind == NodeKind::File) {
            let path = format!("/{}", first_file.name);
            let bytes = volume.read_file(&path, 8192).expect("UFS file read");
            assert!(!bytes.is_empty(), "UFS file should yield bytes");
        }
    }
}
