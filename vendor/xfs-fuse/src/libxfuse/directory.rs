//! Enumerate directory data directly; index blocks are never interpreted as entries.
use super::{
    attributes::block,
    checked::*,
    compat::EIO,
    dinode::{DiU, Dinode},
    dir3::Dir3,
    logical,
    sb::Sb,
    volume::DirEntry,
};
use bincode_next::de::read::Reader;
use std::{
    collections::HashSet,
    io::{BufRead, Seek},
};

pub(crate) fn read<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &Dinode,
    owner: u64,
) -> Result<Vec<DirEntry>, i32> {
    if !sb.has_dirv2() {
        return super::legacy_dir::read(reader, sb, inode);
    }
    if let DiU::Dir2Sf(directory) = &inode.di_u {
        let mut output = Vec::new();
        let mut offset = 0;
        loop {
            match directory.next(reader, sb, offset) {
                Ok((ino, next, kind, name)) => {
                    if next <= offset {
                        return Err(EIO);
                    }
                    output.push(DirEntry {
                        ino,
                        offset: next,
                        kind,
                        name,
                    });
                    offset = next;
                }
                Err(-1) | Err(super::compat::ENOENT) => break,
                Err(error) => return Err(error),
            }
        }
        return validate(output);
    }
    let records = logical::data_extents(reader, sb, inode)?;
    if records.first().is_none_or(|record| record.br_startoff != 0) {
        return Err(EIO);
    }
    let block_count = 1u64 << sb.sb_dirblklog;
    let directory_size = (u64::from(sb.sb_blocksize) * block_count) as usize;
    let leaf_start = 1u64 << (35 - sb.sb_blocklog);
    let data_end = u64::try_from(inode.di_core.di_size).map_err(|_| EIO)?;
    if data_end == 0 || !data_end.is_multiple_of(directory_size as u64) {
        return Err(EIO);
    }
    let mut output = Vec::new();
    let mut visited = HashSet::new();
    for record in &records {
        if record.br_startoff >= leaf_start {
            break;
        }
        if record.br_flag {
            return Err(EIO);
        }
        let end =
            (record.br_startoff + record.br_blockcount).min(data_end / u64::from(sb.sb_blocksize));
        let mut logical_block = record.br_startoff;
        while logical_block < end {
            let start = logical_block - logical_block % block_count;
            logical_block = start + block_count;
            if !visited.insert(start) {
                continue;
            }
            if visited.len() > 100_000 {
                return Err(EIO);
            }
            let mut raw = Vec::with_capacity(directory_size);
            let mut physical_start = 0;
            for index in 0..block_count {
                let (bytes, physical) = block(reader, sb, &records, start + index)?;
                if index == 0 {
                    physical_start = physical;
                }
                raw.extend_from_slice(&bytes);
            }
            let (header_size, is_block) = match u32be(&raw, 0)? {
                0x58443242 if sb.version() < 5 => (16, true),
                0x58443244 if sb.version() < 5 => (16, false),
                0x58444233 | 0x58444433 if sb.version() == 5 => {
                    crc(&raw, 4)?;
                    if u64be(&raw, 8)? != physical_start / 512
                        || bytes(&raw, 24, 16)? != sb.metadata_uuid.as_bytes()
                        || u64be(&raw, 40)? != owner
                    {
                        return Err(EIO);
                    }
                    (64, u32be(&raw, 0)? == 0x58444233)
                }
                _ => return Err(EIO),
            };
            let end = if is_block {
                let count = u32be(&raw, raw.len() - 8)? as usize;
                raw.len()
                    .checked_sub(8 + count.checked_mul(8).ok_or(EIO)?)
                    .ok_or(EIO)?
            } else {
                raw.len()
            };
            if end < header_size {
                return Err(EIO);
            }
            let mut offset = header_size;
            while offset < end {
                if u16be(&raw, offset)? == 0xffff {
                    let length = u16be(&raw, offset + 2)? as usize;
                    if length < 8
                        || !length.is_multiple_of(8)
                        || offset + length > end
                        || u16be(&raw, offset + length - 2)? as usize != offset
                    {
                        return Err(EIO);
                    }
                    offset += length;
                    continue;
                }
                let number = u64be(&raw, offset)?;
                let name_length = *raw.get(offset + 8).ok_or(EIO)? as usize;
                let length =
                    (8 + 1 + name_length + usize::from(sb.has_ftype()) + 2).div_ceil(8) * 8;
                if offset + length > end || u16be(&raw, offset + length - 2)? as usize != offset {
                    return Err(EIO);
                }
                let name = bytes(&raw, offset + 9, name_length)?.to_vec();
                let kind = if sb.has_ftype() {
                    let value = raw[offset + 9 + name_length];
                    if value == 0 {
                        None
                    } else {
                        Some(super::utils::get_file_type(super::utils::FileKind::Type(
                            value,
                        ))?)
                    }
                } else {
                    None
                };
                output.push(DirEntry {
                    ino: number,
                    name,
                    kind,
                    offset: output.len() as i64 + 1,
                });
                if output.len() > 100_000 {
                    return Err(EIO);
                }
                offset += length;
            }
        }
    }
    validate(output)
}

fn validate(entries: Vec<DirEntry>) -> Result<Vec<DirEntry>, i32> {
    let mut names = HashSet::new();
    for entry in &entries {
        if entry.ino == 0
            || entry.name.is_empty()
            || entry.name.contains(&0)
            || entry.name.contains(&b'/')
            || !names.insert(entry.name.clone())
        {
            return Err(EIO);
        }
    }
    Ok(entries)
}
