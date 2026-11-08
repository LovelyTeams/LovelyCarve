//! Attribute enumeration with exact namespace/name matching and bounded remote values.
use super::{
    bmbt_rec::BmbtRec,
    checked::*,
    compat::EIO,
    dinode::{DiA, Dinode},
    logical,
    sb::Sb,
};
use bincode_next::de::read::Reader;
use std::{
    collections::HashSet,
    io::{BufRead, Seek, SeekFrom},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedAttribute {
    /// Linux namespace prefix followed by the original name bytes.
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    pub flags: u8,
    pub extents: Vec<super::volume::Extent>,
}

fn name(flags: u8, raw: &[u8]) -> Result<Vec<u8>, i32> {
    if raw.is_empty()
        || raw.contains(&0)
        || !matches!(flags & 14, 0 | 2 | 4 | 8)
        || flags & !0x8f != 0
    {
        return Err(EIO);
    }
    let prefix: &[u8] = if flags & 8 != 0 {
        b"parent."
    } else if flags & 4 != 0 {
        b"security."
    } else if flags & 2 != 0 {
        b"trusted."
    } else {
        b"user."
    };
    Ok([prefix, raw].concat())
}

pub(crate) fn block<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    records: &[BmbtRec],
    logical_block: u64,
) -> Result<(Vec<u8>, u64), i32> {
    let record = records
        .iter()
        .find(|r| logical_block >= r.br_startoff && logical_block - r.br_startoff < r.br_blockcount)
        .ok_or(EIO)?;
    if record.br_flag {
        return Err(EIO);
    }
    let offset = sb
        .extent_offset(record.br_startblock, record.br_blockcount)?
        .checked_add(
            (logical_block - record.br_startoff)
                .checked_mul(u64::from(sb.sb_blocksize))
                .ok_or(EIO)?,
        )
        .ok_or(EIO)?;
    reader.seek(SeekFrom::Start(offset)).map_err(|_| EIO)?;
    let mut raw = vec![0; sb.sb_blocksize as usize];
    reader.read_exact(&mut raw).map_err(|_| EIO)?;
    Ok((raw, offset))
}

pub(crate) fn read<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &Dinode,
    owner: u64,
) -> Result<Vec<ExtendedAttribute>, i32> {
    if let Some(DiA::Attrsf(attributes)) = &inode.di_a {
        let mut output = Vec::new();
        let mut storage = sb.inode_offset(owner)?
            + inode.di_core.literal_area_offset() as u64
            + u64::from(inode.di_core.di_forkoff) * 8
            + 4;
        for entry in &attributes.list {
            let value_offset = storage + 3 + u64::from(entry.namelen);
            storage += 3 + entry.nameval.len() as u64;
            if entry.flags & 0x80 != 0 {
                continue;
            }
            output.push(ExtendedAttribute {
                name: name(
                    entry.flags,
                    bytes(&entry.nameval, 0, entry.namelen as usize)?,
                )?,
                value: entry
                    .nameval
                    .get(entry.namelen as usize..)
                    .ok_or(EIO)?
                    .to_vec(),
                flags: entry.flags,
                extents: vec![super::volume::Extent {
                    logical_offset: 0,
                    physical_offset: value_offset,
                    length: (entry.nameval.len() - entry.namelen as usize) as u64,
                    unwritten: false,
                }],
            });
        }
        return unique(output);
    }
    if inode.di_a.is_none() || inode.di_core.anextents == 0 {
        return Ok(Vec::new());
    }
    let records = logical::attr_extents(reader, sb, inode)?;
    let mut pending = vec![(0, None)];
    let mut visited = HashSet::new();
    let mut output = Vec::new();
    let mut total = 0usize;
    while let Some((logical_block, expected_level)) = pending.pop() {
        if !visited.insert(logical_block) || visited.len() > 65536 {
            return Err(EIO);
        }
        let (raw, offset) = block(reader, sb, &records, logical_block)?;
        let magic = u16be(&raw, 8)?;
        let info_size = match magic {
            0xfbee | 0xfebe if sb.version() < 5 => 12,
            0x3bee | 0x3ebe if sb.version() == 5 => {
                crc(&raw, 12)?;
                if u64be(&raw, 16)? != offset / 512
                    || bytes(&raw, 32, 16)? != sb.metadata_uuid.as_bytes()
                    || u64be(&raw, 48)? != owner
                {
                    return Err(EIO);
                }
                56
            }
            _ => return Err(EIO),
        };
        let count = u16be(&raw, info_size)? as usize;
        if magic == 0xfebe || magic == 0x3ebe {
            let level = u16be(&raw, info_size + 2)?;
            if level == 0 || level > 32 || expected_level.is_some_and(|v| v != level) {
                return Err(EIO);
            }
            let entries = if info_size == 56 { 64 } else { 16 };
            bytes(&raw, entries, count.checked_mul(8).ok_or(EIO)?)?;
            for index in (0..count).rev() {
                pending.push((
                    u64::from(u32be(&raw, entries + index * 8 + 4)?),
                    Some(level - 1),
                ));
            }
            if count == 0 {
                return Err(EIO);
            }
        } else {
            if expected_level.is_some_and(|v| v != 0) {
                return Err(EIO);
            }
            let entries = if info_size == 56 { 80 } else { 32 };
            let entry_end = entries + count.checked_mul(8).ok_or(EIO)?;
            bytes(&raw, 0, entry_end)?;
            for index in 0..count {
                let entry = entries + index * 8;
                let flags = raw[entry + 6];
                if flags & 0x80 != 0 {
                    continue;
                }
                let value_offset = u16be(&raw, entry + 4)? as usize;
                if value_offset < entry_end {
                    return Err(EIO);
                }
                let (attribute_name, value, extents) = if flags & 1 != 0 {
                    let value_size = u16be(&raw, value_offset)? as usize;
                    let name_size = *raw.get(value_offset + 2).ok_or(EIO)? as usize;
                    (
                        name(flags, bytes(&raw, value_offset + 3, name_size)?)?,
                        bytes(&raw, value_offset + 3 + name_size, value_size)?.to_vec(),
                        vec![super::volume::Extent {
                            logical_offset: 0,
                            physical_offset: offset + (value_offset + 3 + name_size) as u64,
                            length: value_size as u64,
                            unwritten: false,
                        }],
                    )
                } else {
                    let value_block = u32be(&raw, value_offset)?;
                    let value_size = u32be(&raw, value_offset + 4)? as usize;
                    let name_size = *raw.get(value_offset + 8).ok_or(EIO)? as usize;
                    if value_size > 65536 {
                        return Err(EIO);
                    }
                    let (value, extents) =
                        remote(reader, sb, &records, owner, value_block, value_size)?;
                    (
                        name(flags, bytes(&raw, value_offset + 9, name_size)?)?,
                        value,
                        extents,
                    )
                };
                total = total
                    .checked_add(attribute_name.len() + value.len())
                    .ok_or(EIO)?;
                if total > 64 * 1024 * 1024 {
                    return Err(EIO);
                }
                output.push(ExtendedAttribute {
                    name: attribute_name,
                    value,
                    flags,
                    extents,
                });
            }
        }
    }
    unique(output)
}

fn unique(output: Vec<ExtendedAttribute>) -> Result<Vec<ExtendedAttribute>, i32> {
    let mut names = HashSet::new();
    if output.iter().any(|a| {
        (a.flags & 8 == 0 && !names.insert(a.name.clone()))
            || (a.flags & 8 != 0 && a.value.len() != 12)
    }) {
        return Err(EIO);
    }
    Ok(output)
}

fn remote<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    records: &[BmbtRec],
    owner: u64,
    start: u32,
    length: usize,
) -> Result<(Vec<u8>, Vec<super::volume::Extent>), i32> {
    let mut output = Vec::with_capacity(length);
    let mut extents = Vec::new();
    let mut logical_block = u64::from(start);
    while output.len() < length {
        let (raw, offset) = block(reader, sb, records, logical_block)?;
        let (header, count) = if sb.version() == 5 {
            crc(&raw, 12)?;
            let count = u32be(&raw, 8)? as usize;
            if u32be(&raw, 0)? != 0x5841524d
                || u32be(&raw, 4)? as usize != output.len()
                || count == 0
                || count > raw.len() - 56
                || count > length - output.len()
                || bytes(&raw, 16, 16)? != sb.metadata_uuid.as_bytes()
                || u64be(&raw, 32)? != owner
                || u64be(&raw, 40)? != offset / 512
            {
                return Err(EIO);
            }
            (56, count)
        } else {
            (0, raw.len().min(length - output.len()))
        };
        extents.push(super::volume::Extent {
            logical_offset: output.len() as u64,
            physical_offset: offset + header as u64,
            length: count as u64,
            unwritten: false,
        });
        output.extend_from_slice(bytes(&raw, header, count)?);
        logical_block = logical_block.checked_add(1).ok_or(EIO)?;
    }
    Ok((output, extents))
}
