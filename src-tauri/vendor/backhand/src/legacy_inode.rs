//! Legacy bit fields converted to the shared backhand node and data plan.
use super::wire::{Header, Metadata, require};
use super::{Block, Plan, Result, block, invalid};
use crate::traits::filesystem::{
    BackhandDataSize, BackhandInnerNode as Inner, BackhandNodeHeader,
    BackhandSquashfsFileReader as File,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(super) struct Directory {
    start: u32,
    offset: u32,
    size: u32,
}
pub(super) struct Entry {
    pub name: String,
    pub reference: u64,
    pub kind: u8,
}
pub(super) struct Parsed {
    pub start: usize,
    pub end: usize,
    pub kind: u8,
    pub header: BackhandNodeHeader,
    pub inner: Inner,
    pub directory: Option<Directory>,
    pub plan: Option<Plan>,
}

pub(super) fn parse(
    reference: u64,
    table: &Metadata,
    directories: &Metadata,
    header: &Header,
    fragments: &[Block],
) -> Result<Parsed> {
    let start = table.offset(reference)?;
    let bytes = table
        .bytes
        .get(start..)
        .ok_or_else(|| invalid("inode outside table"))?;
    let order = header.order;
    let raw_kind = order.bits(bytes, 0, 4)? as u8;
    let base = if header.version == 1 { 3 } else { 4 };
    let uid_low = order.bits(bytes, 16, if header.version == 1 { 4 } else { 8 })? as usize;
    let gid_index = order.bits(
        bytes,
        if header.version == 1 { 20 } else { 24 },
        if header.version == 1 { 4 } else { 8 },
    )? as usize;
    let (kind, uid_index) = if header.version == 1 {
        if raw_kind == 0 {
            let kind = order.bits(bytes, 24, 4)? as u8;
            require(matches!(kind, 6 | 7), "invalid IPC type")?;
            (kind, uid_low + 16 * order.bits(bytes, 28, 4)? as usize)
        } else {
            (
                (raw_kind - 1) % 5 + 1,
                uid_low + 16 * ((raw_kind - 1) / 5) as usize,
            )
        }
    } else {
        require((1..=8).contains(&raw_kind), "inode type")?;
        (if raw_kind == 8 { 1 } else { raw_kind }, uid_low)
    };
    let uid = *header
        .uids
        .get(uid_index)
        .ok_or_else(|| invalid("UID index"))?;
    let gid = if gid_index == if header.version == 1 { 15 } else { 255 } {
        uid
    } else {
        *header
            .gids
            .get(gid_index)
            .ok_or_else(|| invalid("GID index"))?
    };
    let mut node_header = BackhandNodeHeader {
        permissions: order.bits(bytes, 4, 12)? as u16,
        uid,
        gid,
        mtime: 0,
        mtime_known: false,
    };
    let mut directory = None;
    let mut plan = None;
    let (inner, length) = match kind {
        1 => {
            let long = raw_kind == 8 && header.version == 2;
            require(!long || header.minor == 1, "long directory requires v2.1")?;
            let size_bits = if long { 27 } else { 19 };
            let body = base * 8;
            let size = order.bits(bytes, body, size_bits)? as u32;
            let offset = order.bits(bytes, body + size_bits, 13)? as u32;
            let time_byte = base + if long { 5 } else { 4 };
            node_header.mtime = i64::from(order.u32(bytes, time_byte)? as i32);
            node_header.mtime_known = true;
            let block = order.bits(bytes, (time_byte + 4) * 8, 24)? as u32;
            let mut length = time_byte + 7;
            if long {
                let count = order.u16(bytes, length)? as usize;
                length += 2;
                let mut previous = None;
                for _ in 0..count {
                    let index = order.bits(bytes, length * 8, 27)? as u32;
                    let pointer = order.bits(bytes, length * 8 + 27, 29)?;
                    let name_size = order.bits(bytes, length * 8 + 56, 8)? as usize + 1;
                    require(
                        index < size
                            && previous.is_none_or(|old| old < index)
                            && directories.blocks.contains_key(&pointer),
                        "directory index extent/order",
                    )?;
                    previous = Some(index);
                    let name = bytes
                        .get(length + 8..length + 8 + name_size)
                        .ok_or_else(|| invalid("directory index name extent"))?;
                    valid_name(name)?;
                    length += 8 + name_size;
                }
            }
            directory = Some(Directory {
                start: block,
                offset,
                size,
            });
            (Inner::Dir, length)
        }
        2 => {
            let mtime = order.u32(bytes, base)?;
            node_header.mtime = i64::from(mtime as i32);
            node_header.mtime_known = true;
            let blocks_start = u64::from(order.u32(bytes, base + 4)?);
            let (fragment, within, size, length) = if header.version == 1 {
                (u32::MAX, 0, order.u32(bytes, base + 8)?, base + 12)
            } else {
                (
                    order.u32(bytes, base + 8)?,
                    order.u32(bytes, base + 12)?,
                    order.u32(bytes, base + 16)?,
                    base + 20,
                )
            };
            let count = if fragment == u32::MAX {
                (size as usize).div_ceil(header.size)
            } else {
                size as usize / header.size
            };
            require(count <= 1_048_576, "file block count")?;
            let entry_size = if header.version == 1 { 2 } else { 4 };
            let length_end = length
                .checked_add(count * entry_size)
                .ok_or_else(|| invalid("block list overflow"))?;
            require(length_end <= bytes.len(), "truncated block list")?;
            let mut sizes = Vec::with_capacity(count);
            let mut blocks = Vec::with_capacity(count);
            let mut position = blocks_start;
            for index in 0..count {
                let raw = if header.version == 1 {
                    let raw = order.u16(bytes, length + index * 2)?;
                    let size = if raw & 0x7fff == 0 {
                        32768
                    } else {
                        u32::from(raw & 0x7fff)
                    };
                    size | if raw & 0x8000 != 0 { 0x0100_0000 } else { 0 }
                } else {
                    order.u32(bytes, length + index * 4)?
                };
                let logical = (size as usize - index * header.size).min(header.size);
                let data = block(position, raw, header, logical)?;
                require(
                    !data.uncompressed || data.size == logical,
                    "stored block length",
                )?;
                sizes.push(BackhandDataSize {
                    size: data.size as u32,
                    uncompressed: data.uncompressed,
                });
                position += data.size as u64;
                blocks.push(data);
            }
            let tail = if fragment != u32::MAX {
                let data = fragments
                    .get(fragment as usize)
                    .ok_or_else(|| invalid("fragment index"))?
                    .clone();
                let count = size as usize % header.size;
                require(
                    within as usize + count <= header.size,
                    "fragment logical window",
                )?;
                if count == 0 {
                    None
                } else {
                    Some((data, within as usize, count))
                }
            } else {
                require(within == 0, "offset without a fragment")?;
                None
            };
            let descriptor = File::Basic {
                blocks_start,
                frag_index: fragment,
                block_offset: within,
                file_size: size,
                block_sizes: sizes,
            };
            plan = Some(Plan {
                descriptor: descriptor.clone(),
                blocks,
                fragment: tail,
            });
            (Inner::File(descriptor), length_end)
        }
        3 => {
            let size = order.u16(bytes, base)? as usize;
            let link = bytes
                .get(base + 2..base + 2 + size)
                .ok_or_else(|| invalid("truncated symlink"))?;
            require(!link.contains(&0), "NUL in symlink target")?;
            let link =
                std::str::from_utf8(link).map_err(|_| invalid("unsupported non-UTF8 symlink"))?;
            (
                Inner::Symlink {
                    link: PathBuf::from(link),
                },
                base + 2 + size,
            )
        }
        4 => (
            Inner::BlockDevice {
                device_number: u32::from(order.u16(bytes, base)?),
            },
            base + 2,
        ),
        5 => (
            Inner::CharacterDevice {
                device_number: u32::from(order.u16(bytes, base)?),
            },
            base + 2,
        ),
        6 => (Inner::NamedPipe, base + usize::from(header.version == 1)),
        7 => (Inner::Socket, base + usize::from(header.version == 1)),
        _ => return Err(invalid("unknown canonical inode type")),
    };
    require(length <= bytes.len(), "inode record exceeds table")?;
    Ok(Parsed {
        start,
        end: start + length,
        kind,
        header: node_header,
        inner,
        directory,
        plan,
    })
}

pub(super) fn valid_name(bytes: &[u8]) -> Result<&str> {
    let name = std::str::from_utf8(bytes).map_err(|_| invalid("unsupported non-UTF8 name"))?;
    require(
        !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0']),
        "invalid path component",
    )?;
    Ok(name)
}

pub(super) fn directory(
    directory: &Directory,
    table: &Metadata,
    header: &Header,
) -> Result<Vec<Entry>> {
    if directory.size == 0 {
        return Ok(Vec::new());
    }
    let start = table.offset((u64::from(directory.start) << 16) | u64::from(directory.offset))?;
    let bytes = table
        .bytes
        .get(start..start + directory.size as usize)
        .ok_or_else(|| invalid("directory exceeds table"))?;
    let mut offset = 0usize;
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    let mut previous = None;
    while offset < bytes.len() {
        let count = header.order.bits(bytes, offset * 8, 8)? as usize + 1;
        let block = header.order.bits(bytes, offset * 8 + 8, 24)?;
        offset += 4;
        for _ in 0..count {
            let within = header.order.bits(bytes, offset * 8, 13)?;
            let kind = header.order.bits(bytes, offset * 8 + 13, 3)? as u8;
            let size = header.order.bits(bytes, offset * 8 + 16, 8)? as usize + 1;
            require((1..=7).contains(&kind), "directory entry type")?;
            offset += 3;
            let name = valid_name(
                bytes
                    .get(offset..offset + size)
                    .ok_or_else(|| invalid("truncated directory name"))?,
            )?
            .to_owned();
            offset += size;
            if header.version == 2 && header.minor == 1 {
                require(
                    previous.as_ref().is_none_or(|old: &String| old < &name),
                    "unsorted directory",
                )?;
            }
            previous = Some(name.clone());
            require(names.insert(name.clone()), "duplicate directory name")?;
            entries.push(Entry {
                name,
                reference: (block << 16) | within,
                kind,
            });
        }
    }
    Ok(entries)
}
