//! UBIFS over bare/firmware partitions and reconstructed UBI logical volumes.
//! Volume and path identities remain independent of source byte transformations.
use super::ubi::{open_ubi_source, UbiSource};
use crate::core::evidence::image::{find_partition, DiskImage};
use crate::core::filesystem::fs_dispatch::build_stat;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use std::sync::Arc;
use ubidump::{FileType, Inode, ReadAt, UbiReader, UbifsImage};
#[cfg(test)]
#[path = "ubifs_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UbifsSource {
    Partition(u8),
    Ubi { part_id: u8, volume_id: u32 },
}
impl UbifsSource {
    pub fn cache_key(self) -> String {
        match self {
            Self::Partition(part_id) => format!("ubifs:part:{part_id}"),
            Self::Ubi { part_id, volume_id } => format!("ubifs:ubi:part:{part_id}:{volume_id}"),
        }
    }
    pub fn node_id(self, path: &str) -> String {
        match self {
            Self::Partition(part_id) if path.is_empty() => format!("part:{part_id}"),
            Self::Partition(_) | Self::Ubi { .. } => {
                format!("{}:{}", self.cache_key(), hex::encode(path))
            }
        }
    }
}
pub fn parse_ubifs_id(id: &str) -> Option<(UbifsSource, String)> {
    let (source, path) = if let Some(rest) = id.strip_prefix("ubifs:ubi:part:") {
        let mut fields = rest.splitn(3, ':');
        (
            UbifsSource::Ubi {
                part_id: fields.next()?.parse().ok()?,
                volume_id: fields.next()?.parse().ok()?,
            },
            fields.next()?,
        )
    } else {
        let (partition, path) = id.strip_prefix("ubifs:part:")?.split_once(':')?;
        (UbifsSource::Partition(partition.parse().ok()?), path)
    };
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    if !path.is_empty()
        && (!path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return None;
    }
    Some((source, path))
}
enum Bytes {
    Partition(UbiSource),
    Ubi(UbiReader<UbiSource>),
}
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        match self {
            Self::Partition(source) => source.size(),
            Self::Ubi(source) => source.size(),
        }
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        match self {
            Self::Partition(source) => source.read_at(offset, length),
            Self::Ubi(source) => source.read_at(offset, length),
        }
    }
}
pub struct UbifsVolume {
    source: UbifsSource,
    bytes: Bytes,
    reader: UbifsImage,
}
pub fn open_ubifs_source(image: &Arc<DiskImage>, source: UbifsSource) -> Result<UbifsVolume> {
    let bytes = match source {
        UbifsSource::Partition(partition) => Bytes::Partition(UbiSource::partition(
            Arc::clone(image),
            find_partition(image, &partition.to_string())?,
        )?),
        UbifsSource::Ubi { part_id, volume_id } => {
            Bytes::Ubi(open_ubi_source(image, part_id)?.reader(volume_id)?)
        }
    };
    let reader =
        UbifsImage::open(&bytes).with_context(|| format!("UBIFS open {}", source.cache_key()))?;
    Ok(UbifsVolume {
        source,
        bytes,
        reader,
    })
}
impl UbifsVolume {
    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }
    fn timestamps(inode: &Inode) -> Mactb {
        let format = |(seconds, nanos)| {
            DateTime::<Utc>::from_timestamp(seconds, nanos)
                .map(|time| time.to_rfc3339_opts(SecondsFormat::AutoSi, true))
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
        let parent = self.reader.resolve(path)?;
        let mut children = Vec::new();
        for entry in self.reader.children(parent)? {
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
                Some(
                    String::from_utf8(inode.inline.clone())
                        .context("unsupported non-UTF8 UBIFS symlink")?,
                )
            } else {
                None
            };
            children.push(FsNode {
                id: self.source.node_id(&format!("{path}/{}", entry.name)),
                name: entry.name.clone(),
                parent_id: Some(self.source.node_id(path)),
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
        self.read_file_range(path, 0, max_bytes)
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
            "UBIFS entry has no file content"
        );
        let mut offset = 0u64;
        while offset < inode.size {
            let count =
                (inode.size - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64) as usize;
            let bytes = self.reader.read_range(&self.bytes, number, offset, count)?;
            ensure!(bytes.len() == count, "UBIFS short logical file stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }
    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let number = self.reader.resolve(path)?;
        let inode = self.reader.inode(number)?;
        let sample = self.reader.read_range(&self.bytes, number, 0, 4096)?;
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
