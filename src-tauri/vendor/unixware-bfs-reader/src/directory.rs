use std::collections::HashSet;

use lamfold::{BlockSource, DirEntry, FileKind, FoldError, NodeId, Result};

use crate::{ROOT, UnixwareBfs, le16, require};

impl<S: BlockSource> UnixwareBfs<S> {
    pub(crate) fn directory(&mut self, dir: NodeId) -> Result<Vec<DirEntry>> {
        let inode = self.inode(dir)?;
        if inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        require(
            inode.size <= 16 * 1_048_576,
            "UnixWare BFS directory scan limit",
        )?;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut offset = 0;
        let mut chunk = [0; 4096];
        while offset < inode.size {
            let length = (inode.size - offset).min(chunk.len() as u64) as usize;
            self.read_inode(inode, offset, &mut chunk[..length])?;
            for slot in chunk[..length].as_chunks::<16>().0 {
                let child = le16(slot, 0);
                if child == 0 {
                    continue;
                }
                let raw = slot[2..]
                    .split(|&byte| byte == 0)
                    .next()
                    .unwrap_or_default();
                let name = std::str::from_utf8(raw)
                    .map_err(|_| FoldError::Unsupported("UnixWare BFS non-UTF-8 filename"))?;
                require(
                    !name.is_empty() && !name.contains('/'),
                    "UnixWare BFS invalid filename",
                )?;
                require(
                    names.insert(name.to_owned()),
                    "UnixWare BFS duplicate directory name",
                )?;
                if matches!(name, "." | "..") {
                    require(
                        child == ROOT,
                        "UnixWare BFS root dot entry points outside root",
                    )?;
                    continue;
                }
                let child = self.inode(u64::from(child)).map_err(|_| {
                    FoldError::Corrupt("UnixWare BFS directory references an unallocated inode")
                })?;
                require(
                    child.kind == FileKind::Regular,
                    "UnixWare BFS additional directory inode",
                )?;
                entries.push(DirEntry {
                    name: name.to_owned(),
                    node: u64::from(child.number),
                    kind: child.kind,
                });
            }
            offset += length as u64;
        }
        Ok(entries)
    }
}
