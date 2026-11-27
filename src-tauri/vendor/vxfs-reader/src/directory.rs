//! Enumerate allocated directory records without using the on-disk hash chains.

use std::collections::HashSet;

use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::{Vxfs, require};

const RECORD_HEADER: usize = 10;
const MAX_DIRECTORY_BLOCKS: u64 = 131_072;
const MAX_DIRECTORY_NAMES: usize = 1_000_000;

impl<S: BlockSource> Vxfs<S> {
    pub(crate) fn directory(&mut self, number: u64) -> Result<Vec<(String, u32)>> {
        let inode = self.user_inode(number)?;
        if inode.kind() != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let size = self.header.block_size;
        require(
            inode.size.div_ceil(size) <= MAX_DIRECTORY_BLOCKS,
            "VxFS directory block limit",
        )?;
        let endian = self.header.endian;
        let mut bytes = vec![0; size as usize];
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut offset = 0;
        while offset < inode.size {
            let count = (inode.size - offset).min(size) as usize;
            let block = &mut bytes[..count];
            self.read_user_bytes(number, &inode, offset, block)?;
            require(count >= 4, "VxFS truncated directory block header")?;
            let mut cursor = 4 + 2 * usize::from(endian.u16(block, 2));
            require(cursor <= count, "VxFS directory hash array exceeds block")?;
            while cursor < count {
                let tail = &block[cursor..];
                if tail.len() < RECORD_HEADER {
                    require(
                        tail.iter().all(|&byte| byte == 0),
                        "VxFS truncated directory record",
                    )?;
                    break;
                }
                let length = usize::from(endian.u16(tail, 4));
                if length == 0 {
                    break;
                }
                require(
                    length >= RECORD_HEADER && length.is_multiple_of(4) && length <= tail.len(),
                    "VxFS directory record crosses block",
                )?;
                cursor += length;
                let inode = endian.u32(tail, 0);
                if inode == 0 {
                    continue;
                }
                let n = usize::from(endian.u16(tail, 6));
                require(
                    (1..=256).contains(&n) && n + RECORD_HEADER <= length,
                    "VxFS invalid directory name length",
                )?;
                let name = std::str::from_utf8(&tail[RECORD_HEADER..RECORD_HEADER + n])
                    .map_err(|_| FoldError::Unsupported("VxFS non-UTF-8 directory name"))?;
                require(
                    !name.contains(['/', '\0'])
                        && !matches!(name, "." | "..")
                        && inode >= crate::ROOT,
                    "VxFS invalid native directory entry",
                )?;
                require(
                    entries.len() < MAX_DIRECTORY_NAMES && names.insert(name.to_owned()),
                    "VxFS duplicate directory name or name count limit",
                )?;
                entries.push((name.to_owned(), inode));
            }
            offset += count as u64;
        }
        Ok(entries)
    }
}
