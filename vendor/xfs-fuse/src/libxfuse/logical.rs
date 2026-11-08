//! Validated extent plans shared by data reads and forensic provenance.
use super::{
    bmbt_rec::BmbtRec,
    btree::{BmbtKey, BmdrBlock},
    checked::*,
    compat::{EINVAL, EIO},
    dinode::{DiA, DiU, Dinode},
    sb::Sb,
    utils::decode,
};
use bincode_next::de::read::Reader;
use std::{
    collections::HashSet,
    io::{BufRead, Seek, SeekFrom},
};

pub(crate) fn data_extents<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &Dinode,
) -> Result<Vec<BmbtRec>, i32> {
    let records = match &inode.di_u {
        DiU::Bmx(records) => records.clone(),
        DiU::Bmbt((header, keys, pointers)) => {
            tree(reader, sb, inode.di_core.di_ino, header, keys, pointers)?
        }
        _ => return Err(EINVAL),
    };
    validate(sb, &records)?;
    if records.len() as u64 != inode.di_core.nextents {
        return Err(EIO);
    }
    Ok(records)
}

pub(crate) fn attr_extents<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &Dinode,
) -> Result<Vec<BmbtRec>, i32> {
    let records = match &inode.di_a {
        Some(DiA::Abmx(records)) => records.clone(),
        Some(DiA::Abmbt((header, keys, pointers))) => {
            tree(reader, sb, inode.di_core.di_ino, header, keys, pointers)?
        }
        _ => return Err(EINVAL),
    };
    validate(sb, &records)?;
    if records.len() != inode.di_core.anextents as usize || records.iter().any(|r| r.br_flag) {
        return Err(EIO);
    }
    Ok(records)
}

fn validate(sb: &Sb, records: &[BmbtRec]) -> Result<(), i32> {
    let mut end = 0;
    for record in records {
        if record.br_startoff < end {
            return Err(EIO);
        }
        end = record
            .br_startoff
            .checked_add(record.br_blockcount)
            .ok_or(EIO)?;
        end.checked_mul(u64::from(sb.sb_blocksize)).ok_or(EIO)?;
        sb.extent_offset(record.br_startblock, record.br_blockcount)?;
    }
    Ok(())
}

fn tree<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    owner: u64,
    header: &BmdrBlock,
    keys: &[BmbtKey],
    pointers: &[u64],
) -> Result<Vec<BmbtRec>, i32> {
    if header.bb_level == 0
        || header.bb_level > 32
        || pointers.is_empty()
        || pointers.len() != header.bb_numrecs as usize
        || pointers.len() != keys.len()
        || keys
            .windows(2)
            .any(|v| v[0].br_startoff >= v[1].br_startoff)
    {
        return Err(EIO);
    }
    let mut pending: Vec<_> = pointers
        .iter()
        .zip(keys)
        .rev()
        .map(|(p, k)| (*p, header.bb_level - 1, k.br_startoff))
        .collect();
    let mut visited = HashSet::new();
    let mut records = Vec::new();
    while let Some((pointer, level, lower)) = pending.pop() {
        if !visited.insert(pointer) || visited.len() > 1_000_000 {
            return Err(EIO);
        }
        let offset = sb.extent_offset(pointer, 1)?;
        reader.seek(SeekFrom::Start(offset)).map_err(|_| EIO)?;
        let mut raw = vec![0; sb.sb_blocksize as usize];
        reader.read_exact(&mut raw).map_err(|_| EIO)?;
        let header_size = match u32be(&raw, 0)? {
            0x424d4150 if sb.version() < 5 => 24,
            0x424d4133 if sb.version() == 5 => {
                crc(&raw, 64)?;
                if bytes(&raw, 40, 16)? != sb.metadata_uuid.as_bytes()
                    || u64be(&raw, 24)? != offset / 512
                    || u64be(&raw, 56)? != owner
                {
                    return Err(EIO);
                }
                72
            }
            _ => return Err(EIO),
        };
        if u16be(&raw, 4)? != level {
            return Err(EIO);
        }
        let count = u16be(&raw, 6)? as usize;
        let capacity = (raw.len() - header_size) / 16;
        if count == 0 || count > capacity {
            return Err(EIO);
        }
        if level == 0 {
            for index in 0..count {
                records.push(
                    decode::<BmbtRec>(bytes(&raw, header_size + index * 16, 16)?)
                        .map_err(|_| EIO)?
                        .0,
                );
            }
            if records.get(records.len() - count).ok_or(EIO)?.br_startoff != lower {
                return Err(EIO);
            }
            if records.len() > 4_000_000 {
                return Err(EIO);
            }
        } else {
            let mut previous = None;
            let mut children = Vec::with_capacity(count);
            for index in 0..count {
                let key = u64be(&raw, header_size + index * 8)?;
                if previous.is_some_and(|v| v >= key) || (index == 0 && key != lower) {
                    return Err(EIO);
                }
                previous = Some(key);
                children.push((
                    u64be(&raw, header_size + capacity * 8 + index * 8)?,
                    level - 1,
                    key,
                ));
            }
            pending.extend(children.into_iter().rev());
        }
    }
    Ok(records)
}

/// Reads exact logical bytes. Only holes and unwritten extents synthesize zeroes.
pub(crate) fn read<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    records: &[BmbtRec],
    file_size: i64,
    offset: i64,
    size: u32,
) -> Result<Vec<u8>, i32> {
    if file_size < 0 || offset < 0 {
        return Err(EINVAL);
    }
    let offset = offset as u64;
    let length = (file_size as u64)
        .saturating_sub(offset)
        .min(u64::from(size)) as usize;
    let end = offset.checked_add(length as u64).ok_or(EIO)?;
    let mut output = vec![0; length];
    let block_size = u64::from(sb.sb_blocksize);
    let first = records
        .partition_point(|record| record.br_startoff + record.br_blockcount <= offset / block_size);
    for record in &records[first..] {
        let start = record.br_startoff.checked_mul(block_size).ok_or(EIO)?;
        let extent_length = record.br_blockcount.checked_mul(block_size).ok_or(EIO)?;
        let extent_end = start.checked_add(extent_length).ok_or(EIO)?;
        if start >= end {
            break;
        }
        let from = start.max(offset);
        let to = extent_end.min(end);
        if from >= to || record.br_flag {
            continue;
        }
        let physical = sb
            .extent_offset(record.br_startblock, record.br_blockcount)?
            .checked_add(from - start)
            .ok_or(EIO)?;
        reader.seek(SeekFrom::Start(physical)).map_err(|_| EIO)?;
        reader
            .read_exact(&mut output[(from - offset) as usize..(to - offset) as usize])
            .map_err(|_| EIO)?;
    }
    Ok(output)
}
