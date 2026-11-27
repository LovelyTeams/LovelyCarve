use std::collections::HashSet;

use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::{AixJfs, PAGE, ROOT, be16, be32, require};

const DIRECTORY_CHUNK: usize = 512;
const MAX_DIRECTORY_PAGES: u64 = 131072;
const MAX_DIRECTORY_NAMES: usize = 1_000_000;

impl<S: BlockSource> AixJfs<S> {
    pub(crate) fn entries(&mut self, inode_number: u64) -> Result<Vec<(String, u64)>> {
        let inode = self.inode(inode_number)?;
        if inode.kind() != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        require(
            inode.size.is_multiple_of(DIRECTORY_CHUNK as u64)
                && inode.size.div_ceil(PAGE) <= MAX_DIRECTORY_PAGES,
            "AIX JFS directory geometry or work limit",
        )?;
        let mut page = [0; PAGE as usize];
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut offset = 0;
        while offset < inode.size {
            let count = (inode.size - offset).min(PAGE) as usize;
            self.read_inode(&inode, offset, &mut page[..count])?;
            for chunk in page[..count].as_chunks::<DIRECTORY_CHUNK>().0 {
                let mut cursor = 0;
                while cursor < chunk.len() {
                    let tail = &chunk[cursor..];
                    require(tail.len() >= 8, "AIX JFS truncated directory entry")?;
                    let size = usize::from(be16(tail, 4));
                    if size == 0 {
                        break;
                    }
                    require(
                        size >= 8 && size.is_multiple_of(4) && size <= tail.len(),
                        "AIX JFS directory entry crosses a chunk",
                    )?;
                    cursor += size;
                    let index = u64::from(be32(tail, 0));
                    if index == 0 {
                        continue;
                    }
                    let len = usize::from(be16(tail, 6));
                    require(
                        (1..=255).contains(&len) && 8 + len <= size && index >= ROOT,
                        "AIX JFS invalid directory name length or inode",
                    )?;
                    let name = std::str::from_utf8(&tail[8..8 + len])
                        .map_err(|_| FoldError::Unsupported("AIX JFS non-UTF-8 directory name"))?;
                    if matches!(name, "." | "..") {
                        continue;
                    }
                    require(
                        !name.contains(['/', '\0'])
                            && names.insert(name.to_owned())
                            && entries.len() < MAX_DIRECTORY_NAMES,
                        "AIX JFS invalid or duplicate directory filename",
                    )?;
                    entries.push((name.to_owned(), index));
                }
            }
            offset += count as u64;
        }
        Ok(entries)
    }
}
