//! Read-only JFFS2, adapted from xmoezzz/jffs2-rs 0.2.1 (MIT).
//! See LOVELYCARVE.md for compatibility, limits, and upstream attribution.
mod codec;
mod index;
mod nodes;
mod scan;

use anyhow::{Context, Result, ensure};
pub use nodes::{ByteOrder, DirectoryEntry, FileType, Inode, validate_header};
use nodes::{DataNode, checksum};
use std::collections::BTreeMap;

/// Bounded source access. Implementers return exactly the requested bytes or
/// an error, preserving source failures rather than replacing them with zeroes.
pub trait ReadAt {
    fn size(&self) -> u64;
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>>;
}
pub(crate) fn read_exact(source: &dyn ReadAt, offset: u64, length: usize) -> Result<Vec<u8>> {
    ensure!(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= source.size()),
        "JFFS2 read outside source"
    );
    let bytes = source.read_at(offset, length)?;
    ensure!(
        bytes.len() == length,
        "JFFS2 source short read at {offset:#x}"
    );
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fragment {
    end: u64,
    node: usize,
    within: usize,
}
struct File {
    inode: Inode,
    fragments: BTreeMap<u64, Fragment>,
}

/// An immutable namespace and fragment index. File bytes remain in the source;
/// CRC and decompression failures are reported when their data nodes are read.
pub struct Jffs2Image {
    order: ByteOrder,
    length: u64,
    nodes: Vec<DataNode>,
    files: BTreeMap<u32, File>,
    children: BTreeMap<u32, Vec<DirectoryEntry>>,
}
impl Jffs2Image {
    pub fn open(source: &dyn ReadAt) -> Result<Self> {
        Self::open_with_cancel(source, &|| false)
    }
    /// The cancellation callback is polled during scanning and index assembly.
    pub fn open_with_cancel(source: &dyn ReadAt, cancelled: &dyn Fn() -> bool) -> Result<Self> {
        let scan = scan::scan(source, cancelled)?;
        let (files, children) = index::build(&scan.nodes, scan.dirents, cancelled)?;
        Ok(Self {
            order: scan.order,
            length: source.size(),
            nodes: scan.nodes,
            files,
            children,
        })
    }
    pub fn byte_order(&self) -> ByteOrder {
        self.order
    }
    pub fn inode(&self, number: u32) -> Result<&Inode> {
        Ok(&self
            .files
            .get(&number)
            .context("JFFS2 inode not found")?
            .inode)
    }
    pub fn children(&self, number: u32) -> Result<&[DirectoryEntry]> {
        ensure!(
            self.inode(number)?.kind == FileType::Directory,
            "JFFS2 entry is not a directory"
        );
        Ok(self
            .children
            .get(&number)
            .map(Vec::as_slice)
            .unwrap_or_default())
    }
    pub fn resolve(&self, path: &str) -> Result<u32> {
        if path.is_empty() || path == "/" {
            return Ok(1);
        }
        ensure!(
            path.starts_with('/') && !path.contains(['\\', '\0']),
            "JFFS2 invalid path"
        );
        let mut number = 1;
        for (depth, name) in path[1..].split('/').enumerate() {
            ensure!(
                depth < 256 && !matches!(name, "" | "." | ".."),
                "JFFS2 invalid/deep path"
            );
            number = self
                .children(number)?
                .iter()
                .find(|entry| entry.name == name)
                .context("JFFS2 path not found")?
                .inode;
        }
        Ok(number)
    }
    pub fn read_range(
        &self,
        source: &dyn ReadAt,
        number: u32,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        ensure!(source.size() == self.length, "JFFS2 source size changed");
        ensure!(length <= 256 * 1024 * 1024, "JFFS2 range exceeds limit");
        let file = self.files.get(&number).context("JFFS2 inode not found")?;
        ensure!(
            matches!(file.inode.kind, FileType::File | FileType::Symlink),
            "JFFS2 entry has no file content"
        );
        let length = (length as u64).min(file.inode.size.saturating_sub(offset)) as usize;
        let mut output = vec![0; length];
        if length == 0 {
            return Ok(output);
        }
        let end = offset + length as u64;
        let start = file
            .fragments
            .range(..=offset)
            .next_back()
            .map(|(start, _)| *start)
            .unwrap_or(offset);
        for (start, fragment) in file.fragments.range(start..end) {
            if fragment.end <= offset {
                continue;
            }
            let node = &self.nodes[fragment.node];
            let bytes = read_exact(source, node.source, node.compressed as usize)?;
            ensure!(
                checksum(&bytes) == node.data_crc,
                "JFFS2 data CRC mismatch at {:#x}",
                node.source
            );
            let decoded = codec::decode(&bytes, node.compression, node.size as usize)?;
            let from = (*start).max(offset);
            let to = fragment.end.min(end);
            let within = fragment.within + (from - start) as usize;
            let bytes = decoded
                .get(within..within + (to - from) as usize)
                .context("JFFS2 fragment outside decoded node")?;
            output[(from - offset) as usize..(to - offset) as usize].copy_from_slice(bytes);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests;
