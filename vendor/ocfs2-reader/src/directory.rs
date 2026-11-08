use crate::{extents::Stream, header::Header, inode::Inode, le16, le64, require};
use lamfold::{BlockSource, FileKind, FoldError, Result};
use std::collections::HashSet;

pub(crate) struct Entry {
    pub name: String,
    pub node: u64,
    pub kind: Option<FileKind>,
    pub block: u64,
}
pub(crate) struct Directory {
    pub entries: Vec<Entry>,
    pub blocks: HashSet<u64>,
}
impl Directory {
    pub fn read(
        source: &mut impl BlockSource,
        header: &Header,
        inode: &Inode,
        stream: &Stream,
    ) -> Result<Self> {
        require(
            inode.kind() == FileKind::Directory,
            "ocfs2: directory on non-directory inode",
        )?;
        let mut directory = Self {
            entries: Vec::new(),
            blocks: HashSet::new(),
        };
        if let Some(raw) = inode.inline()? {
            require(
                inode.dynamic & 8 == 0 && inode.directory_index == 0,
                "ocfs2: indexed inline directory",
            )?;
            directory.records(header, raw, inode.address)?;
        } else {
            require(
                inode.size.is_multiple_of(header.block_size as u64)
                    && inode.size / header.block_size as u64 <= 131072,
                "ocfs2: directory size/work limit",
            )?;
            for offset in (0..inode.size).step_by(header.block_size as usize) {
                let physical = stream
                    .physical_at(offset)?
                    .ok_or(FoldError::Corrupt("ocfs2: directory hole"))?;
                require(
                    physical.is_multiple_of(header.block_size as u64),
                    "ocfs2: directory block alignment",
                )?;
                let block = physical / header.block_size as u64;
                require(
                    directory.blocks.insert(block),
                    "ocfs2: directory data block alias",
                )?;
                let raw = header.read_block(source, block)?;
                let end = if header.has_metadata_crc() || inode.dynamic & 8 != 0 {
                    let at = raw.len() - 64;
                    let trailer = &raw[at..];
                    require(
                        le64(trailer, 0)? == 0
                            && le16(trailer, 8)? == 64
                            && trailer[10] == 0
                            && trailer.get(16..24) == Some(b"DIRTRL1\0"),
                        "ocfs2: directory trailer signature/layout",
                    )?;
                    require(
                        le64(trailer, 40)? == block && le64(trailer, 48)? == inode.address,
                        "ocfs2: directory trailer owner/address",
                    )?;
                    header.verify(&raw, at + 56)?;
                    at
                } else {
                    raw.len()
                };
                directory.records(header, &raw[..end], block)?;
            }
        }
        let mut names = HashSet::new();
        for entry in &directory.entries {
            require(
                names.insert(entry.name.clone()),
                "ocfs2: duplicate directory name",
            )?;
        }
        let own = directory
            .entries
            .iter()
            .find(|entry| entry.name == ".")
            .ok_or(FoldError::Corrupt("ocfs2: missing dot entry"))?;
        require(
            own.node == inode.address && own.kind.is_none_or(|kind| kind == FileKind::Directory),
            "ocfs2: dot identity",
        )?;
        let parent = directory
            .entries
            .iter()
            .find(|entry| entry.name == "..")
            .ok_or(FoldError::Corrupt("ocfs2: missing parent entry"))?;
        require(
            parent.kind.is_none_or(|kind| kind == FileKind::Directory),
            "ocfs2: parent kind",
        )?;
        Ok(directory)
    }
    fn records(&mut self, header: &Header, raw: &[u8], block: u64) -> Result<()> {
        let mut offset = 0;
        while offset < raw.len() {
            let record = &raw[offset..];
            let node = le64(record, 0)?;
            let length = le16(record, 8)? as usize;
            require(
                record.len() >= 12
                    && length >= 16
                    && length.is_multiple_of(4)
                    && length <= record.len(),
                "ocfs2: directory record bounds",
            )?;
            let name_length = record[10] as usize;
            require(12 + name_length <= length, "ocfs2: directory name length")?;
            if node != 0 {
                header.block_offset(node)?;
                require(
                    name_length != 0 && self.entries.len() < 1_000_000,
                    "ocfs2: empty/excessive names",
                )?;
                let name = &record[12..12 + name_length];
                require(
                    !name.iter().any(|byte| matches!(byte, 0 | b'/')),
                    "ocfs2: invalid name component",
                )?;
                let name = String::from_utf8(name.to_vec())
                    .map_err(|_| FoldError::Unsupported("ocfs2: non-UTF-8 directory name"))?;
                let kind = match record[11] {
                    0 => None,
                    1 => Some(FileKind::Regular),
                    2 => Some(FileKind::Directory),
                    7 => Some(FileKind::Symlink),
                    3..=6 => Some(FileKind::Other),
                    _ => return Err(FoldError::Corrupt("ocfs2: directory entry type")),
                };
                self.entries.push(Entry {
                    name,
                    node,
                    kind,
                    block,
                });
            }
            offset += length;
        }
        Ok(())
    }
}
