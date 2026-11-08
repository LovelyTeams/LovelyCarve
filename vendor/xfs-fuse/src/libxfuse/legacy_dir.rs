//! Directory version 1: inline entries and bounded leaf/node traversal.
use super::{
    attributes::block,
    checked::*,
    compat::EIO,
    dinode::{DiU, Dinode},
    logical,
    sb::Sb,
    volume::DirEntry,
};
use bincode_next::de::read::Reader;
use std::{
    collections::HashSet,
    io::{BufRead, Seek},
};

fn entry(inode: u64, name: &[u8], index: usize) -> Result<DirEntry, i32> {
    if name.is_empty() || name.contains(&0) || name.contains(&b'/') {
        return Err(EIO);
    }
    Ok(DirEntry {
        ino: inode,
        name: name.to_vec(),
        kind: None,
        offset: index as i64 + 1,
    })
}

pub(crate) fn read<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &Dinode,
) -> Result<Vec<DirEntry>, i32> {
    let mut output = Vec::new();
    if let DiU::LegacyLocal(raw) = &inode.di_u {
        let count = *raw.get(8).ok_or(EIO)? as usize;
        let mut offset = 9;
        for index in 0..count {
            let number = u64be(raw, offset)?;
            let length = *raw.get(offset + 8).ok_or(EIO)? as usize;
            output.push(entry(number, bytes(raw, offset + 9, length)?, index)?);
            offset += 9 + length;
        }
        if offset != raw.len() {
            return Err(EIO);
        }
    } else {
        let records = logical::data_extents(reader, sb, inode)?;
        let mut pending = vec![(0, None)];
        let mut visited = HashSet::new();
        while let Some((logical_block, expected_level)) = pending.pop() {
            if !visited.insert(logical_block) || visited.len() > 100_000 {
                return Err(EIO);
            }
            let (raw, _) = block(reader, sb, &records, logical_block)?;
            let count = u16be(&raw, 12)? as usize;
            match u16be(&raw, 8)? {
                0xfeeb => {
                    if expected_level.is_some_and(|v| v != 0) {
                        return Err(EIO);
                    }
                    let end = 32 + count * 8;
                    bytes(&raw, 0, end)?;
                    for index in 0..count {
                        let record = 32 + index * 8;
                        let offset = u16be(&raw, record + 4)? as usize;
                        let length = raw[record + 6] as usize;
                        if offset < end {
                            return Err(EIO);
                        }
                        output.push(entry(
                            u64be(&raw, offset)?,
                            bytes(&raw, offset + 8, length)?,
                            output.len(),
                        )?);
                        if output.len() > 100_000 {
                            return Err(EIO);
                        }
                    }
                }
                0xfebe => {
                    let level = u16be(&raw, 14)?;
                    if level == 0
                        || level > 32
                        || count == 0
                        || expected_level.is_some_and(|v| v != level)
                    {
                        return Err(EIO);
                    }
                    bytes(&raw, 16, count * 8)?;
                    for index in (0..count).rev() {
                        pending.push((u64::from(u32be(&raw, 20 + index * 8)?), Some(level - 1)));
                    }
                }
                _ => return Err(EIO),
            }
        }
    }
    let mut names = HashSet::new();
    if output.iter().any(|entry| !names.insert(entry.name.clone())) {
        return Err(EIO);
    }
    Ok(output)
}
