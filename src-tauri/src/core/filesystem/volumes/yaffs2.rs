//! Native YAFFS2 NAND page/OOB view; original evidence bytes stay unchanged.
use super::ubi::UbiSource;
use crate::core::evidence::image::{find_partition, DiskImage, PartitionKind};
use crate::core::filesystem::{
    fs_dispatch::build_stat,
    tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind},
};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use std::sync::Arc;
use yaffs2_reader::{FileType, Format, Node, ReadAt, ReadOnlyFilesystem};

#[cfg(test)]
#[path = "yaffs2_tests.rs"]
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
    node_id_for_format(Format::Yaffs2, partition, path)
}
pub fn node_id_for_format(format: Format, partition: u8, path: &str) -> String {
    if path.is_empty() {
        format!("part:{partition}")
    } else {
        let prefix = match format {
            Format::Yaffs1 => "yaffs1",
            Format::Yaffs2 => "yaffs2",
        };
        format!("{prefix}:part:{partition}:{}", hex::encode(path))
    }
}
pub fn parse_yaffs2_id(id: &str) -> Option<(Format, u8, String)> {
    let (format, rest) = if let Some(rest) = id.strip_prefix("yaffs1:part:") {
        (Format::Yaffs1, rest)
    } else {
        (Format::Yaffs2, id.strip_prefix("yaffs2:part:")?)
    };
    let (partition, path) = rest.split_once(':')?;
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
    Some((format, partition.parse().ok()?, path))
}
pub struct Yaffs2Volume {
    partition: u8,
    bytes: Bytes,
    reader: Box<dyn ReadOnlyFilesystem>,
}
#[inline(never)]
pub fn open_yaffs2_source(image: &Arc<DiskImage>, partition: u8) -> Result<Yaffs2Volume> {
    let bytes = Bytes(UbiSource::partition(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )?);
    let reader = yaffs2_reader::open_filesystem(&bytes)
        .with_context(|| format!("YAFFS2 open partition {partition}"))?;
    let expected = match find_partition(image, &partition.to_string())?.kind {
        PartitionKind::Yaffs1 => Format::Yaffs1,
        PartitionKind::Yaffs2 => Format::Yaffs2,
        kind => anyhow::bail!("YAFFS partition has incompatible kind {kind:?}"),
    };
    let format = reader.format();
    ensure!(
        format == expected,
        "YAFFS partition kind disagrees with source layout"
    );
    Ok(Yaffs2Volume {
        partition,
        bytes,
        reader,
    })
}
impl Yaffs2Volume {
    #[inline(never)]
    pub(crate) fn format(&self) -> Format {
        self.reader.format()
    }
    #[inline(never)]
    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }
    #[inline(never)]
    fn timestamps(node: &Node) -> Mactb {
        let format = |seconds: Option<i64>| {
            seconds
                .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
                .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
        };
        Mactb {
            modified: format(node.modified),
            accessed: format(node.accessed),
            mft_changed: format(node.changed),
            created: None,
            birth: None,
        }
    }
    #[inline(never)]
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let mut children = Vec::new();
        for entry in self.reader.children(self.reader.resolve(path)?)? {
            let node = self.reader.content_node(entry.object_id)?;
            let kind = match node.kind {
                FileType::Directory => NodeKind::Dir,
                FileType::Symlink => NodeKind::Symlink,
                FileType::File | FileType::Special | FileType::HardLink => NodeKind::File,
            };
            children.push(FsNode {
                id: node_id_for_format(
                    self.format(),
                    self.partition,
                    &format!("{path}/{}", entry.name),
                ),
                parent_id: Some(node_id_for_format(self.format(), self.partition, path)),
                name: entry.name.clone(),
                kind,
                size: node.size,
                deleted: false,
                inode: Some(u64::from(node.object_id)),
                timestamps: Self::timestamps(node),
                link_target: (node.kind == FileType::Symlink).then(|| node.link_target.clone()),
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
    #[inline(never)]
    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let number = self.reader.resolve(path)?;
        let length = self.reader.content_node(number)?.size.min(max_bytes as u64) as usize;
        self.reader.read_range(&self.bytes, number, 0, length)
    }
    #[inline(never)]
    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.reader
            .read_range(&self.bytes, self.reader.resolve(path)?, offset, length)
    }
    #[inline(never)]
    pub fn stream_file(
        &self,
        path: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let number = self.reader.resolve(path)?;
        let node = self.reader.content_node(number)?;
        ensure!(
            matches!(node.kind, FileType::File | FileType::Symlink),
            "YAFFS2 entry has no file content"
        );
        let mut offset = 0;
        while offset < node.size {
            let count =
                (node.size - offset).min(chunk_size.clamp(4096, 1024 * 1024) as u64) as usize;
            let bytes = self.reader.read_range(&self.bytes, number, offset, count)?;
            ensure!(bytes.len() == count, "YAFFS2 short logical stream");
            visitor(&bytes)?;
            offset += count as u64;
        }
        Ok(offset)
    }
    #[inline(never)]
    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        let number = self.reader.resolve(path)?;
        let node = self.reader.content_node(number)?;
        let sample = if matches!(node.kind, FileType::File | FileType::Symlink) {
            self.reader.read_range(&self.bytes, number, 0, 4096)?
        } else {
            Vec::new()
        };
        Ok(build_stat(
            node_id,
            node.size,
            Some(u64::from(node.object_id)),
            node.mode.map(|mode| mode as u16),
            &sample,
            Vec::new(),
        ))
    }
}
