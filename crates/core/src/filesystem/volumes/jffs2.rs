//! JFFS2 partition view; original evidence bytes remain unchanged.
use super::ubi::UbiSource;
use crate::evidence::image::{find_partition, DiskImage};
use crate::filesystem::fs_dispatch::build_stat;
use crate::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use jffs2::{FileType, Inode, Jffs2Image, ReadAt};
use std::sync::Arc;
#[cfg(test)]
#[path = "jffs2_tests.rs"]
mod tests;

struct Bytes(UbiSource);
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        ubidump::ReadAt::size(&self.0)
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        ubidump::ReadAt::read_at(&self.0, offset, length)
    }
}
pub fn node_id(partition: u8, path: &str) -> String {
    if path.is_empty() {
        format!("part:{partition}")
    } else {
        format!("jffs2:part:{partition}:{}", hex::encode(path))
    }
}
pub fn parse_jffs2_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("jffs2:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    if !path.is_empty()
        && (!path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path
                .split('/')
                .skip(1)
                .any(|part| matches!(part, "" | "." | "..")))
    {
        return None;
    }
    Some((partition.parse().ok()?, path))
}
pub struct Jffs2Volume {
    partition: u8,
    bytes: Bytes,
    reader: Jffs2Image,
}
pub fn open_jffs2_source(image: &Arc<DiskImage>, partition: u8) -> Result<Jffs2Volume> {
    let bytes = Bytes(UbiSource::partition(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )?);
    let reader =
        Jffs2Image::open(&bytes).with_context(|| format!("JFFS2 open partition {partition}"))?;
    Ok(Jffs2Volume {
        partition,
        bytes,
        reader,
    })
}
impl Jffs2Volume {
    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }
    fn timestamps(inode: &Inode) -> Mactb {
        let format = |seconds| {
            DateTime::<Utc>::from_timestamp(i64::from(seconds), 0)
                .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
        };
        Mactb {
            modified: format(inode.modified),
            accessed: format(inode.accessed),
            mft_changed: format(inode.changed),
            created: None,
            birth: None,
        }
    }
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let mut children = Vec::new();
        for entry in self.reader.children(self.reader.resolve(path)?)? {
            let inode = self.reader.inode(entry.inode)?;
            let kind = match inode.kind {
                FileType::Directory => NodeKind::Dir,
                FileType::Symlink => NodeKind::Symlink,
                FileType::File
                | FileType::BlockDevice
                | FileType::CharacterDevice
                | FileType::Fifo
                | FileType::Socket => NodeKind::File,
            };
            let link_target = if inode.kind == FileType::Symlink {
                ensure!(inode.size <= 65536, "JFFS2 symlink target exceeds limit");
                Some(
                    String::from_utf8(self.reader.read_range(
                        &self.bytes,
                        entry.inode,
                        0,
                        inode.size as usize,
                    )?)
                    .context("unsupported non-UTF8 JFFS2 symlink")?,
                )
            } else {
                None
            };
            children.push(FsNode {
                id: node_id(self.partition, &format!("{path}/{}", entry.name)),
                parent_id: Some(node_id(self.partition, path)),
                name: entry.name.clone(),
                kind,
                size: if matches!(inode.kind, FileType::File | FileType::Symlink) {
                    inode.size
                } else {
                    0
                },
                deleted: false,
                inode: Some(u64::from(entry.inode)),
                timestamps: Self::timestamps(inode),
                link_target,
                has_snapshot: false,
            });
        }
        children.sort_by(|a, b| {
            (a.kind != NodeKind::Dir)
                .cmp(&(b.kind != NodeKind::Dir))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(children)
    }
    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let number = self.reader.resolve(path)?;
        let length = self.reader.inode(number)?.size.min(max_bytes as u64) as usize;
        self.reader.read_range(&self.bytes, number, 0, length)
    }
    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.reader
            .read_range(&self.bytes, self.reader.resolve(path)?, offset, length)
    }
    pub fn stream_file(
        &self,
        path: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let number = self.reader.resolve(path)?;
        let inode = self.reader.inode(number)?;
        ensure!(
            matches!(inode.kind, FileType::File | FileType::Symlink),
            "JFFS2 entry has no file content"
        );
        let mut offset = 0u64;
        while offset < inode.size {
            let count =
                (inode.size - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64) as usize;
            let bytes = self.reader.read_range(&self.bytes, number, offset, count)?;
            ensure!(bytes.len() == count, "JFFS2 short logical file stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }
    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let number = self.reader.resolve(path)?;
        let inode = self.reader.inode(number)?;
        let sample = if matches!(inode.kind, FileType::File | FileType::Symlink) {
            self.reader.read_range(&self.bytes, number, 0, 4096)?
        } else {
            Vec::new()
        };
        Ok(build_stat(
            node_id,
            inode.size,
            Some(u64::from(number)),
            Some(inode.mode as u16),
            &sample,
            Vec::new(),
        ))
    }
}
