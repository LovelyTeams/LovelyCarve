//! Variable inline-directory geometry and occupied multi-slot names.
use super::*;
use std::collections::BTreeSet;

impl<S: BlockSource> F2fs<S> {
    pub(super) fn directory(&mut self, nid: u32) -> Result<Vec<DirEntry>> {
        let inode = self.inode(nid)?;
        require(inode.kind == FileKind::Directory, "F2FS not a directory")?;
        if let Some(reason) = inode.unreadable {
            return Err(FoldError::Unsupported(reason));
        }
        let mut result = Vec::new();
        if inode.inline_flags & 4 != 0 {
            let data = &inode.bytes[inode.inline_start..inode.inline_end];
            let slots = data.len() * 8 / 153;
            let bitmap = slots.div_ceil(8);
            let reserved = data.len() - slots * 19 - bitmap;
            entries(data, slots, bitmap + reserved, &mut result)?;
        } else {
            let blocks = inode.size.div_ceil(4096);
            require(
                blocks <= self.sb.block_count && blocks <= 16384,
                "F2FS directory block limit",
            )?;
            for index in 0..blocks {
                let address = self.physical(nid, &inode, index)?;
                if address == 0 || address == u32::MAX {
                    continue;
                }
                entries(&self.data_block(address)?, 214, 30, &mut result)?;
                require(result.len() <= 1_000_000, "F2FS directory entry limit")?;
            }
        }
        let mut names = BTreeSet::new();
        for entry in &result {
            require(
                names.insert(entry.name.as_str()),
                "F2FS duplicate directory name",
            )?;
        }
        Ok(result)
    }
}

fn entries(
    bytes: &[u8],
    slots: usize,
    dentry_start: usize,
    result: &mut Vec<DirEntry>,
) -> Result<()> {
    let names_start = dentry_start + slots * 11;
    require(
        names_start + slots * 8 <= bytes.len(),
        "F2FS dentry area bounds",
    )?;
    let occupied = |slot: usize| bytes[slot / 8] & (1 << (slot % 8)) != 0;
    let mut slot = 0;
    while slot < slots {
        if !occupied(slot) {
            slot += 1;
            continue;
        }
        let offset = dentry_start + slot * 11;
        let nid = u32_at(bytes, offset + 4)?;
        let size = u16_at(bytes, offset + 8)? as usize;
        require((1..=255).contains(&size), "F2FS invalid dentry name length")?;
        let count = size.div_ceil(8);
        require(
            slot + count <= slots && (slot..slot + count).all(occupied),
            "F2FS incomplete dentry slots",
        )?;
        let start = names_start + slot * 8;
        let name = std::str::from_utf8(&bytes[start..start + size])
            .map_err(|_| FoldError::Unsupported("F2FS non-UTF-8 name"))?;
        require(!name.contains(['/', '\\', '\0']), "F2FS unsafe name")?;
        if name != "." && name != ".." {
            require(nid >= 3, "F2FS reserved inode in directory")?;
            let kind = match bytes[offset + 10] {
                1 => FileKind::Regular,
                2 => FileKind::Directory,
                7 => FileKind::Symlink,
                _ => FileKind::Other,
            };
            result.push(DirEntry {
                name: name.to_owned(),
                node: nid as u64,
                kind,
            });
        }
        slot += count;
    }
    Ok(())
}
