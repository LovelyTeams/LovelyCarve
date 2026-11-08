//! Checked ext extended attributes and inline payloads.

use crate::Ext4;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::features::IncompatibleFeatures;
use crate::inode::{Inode, InodeFlags, InodeIndex};
use crate::util::{read_u16le, read_u32le};
use alloc::{format, string::String, vec, vec::Vec};

const MAGIC: u32 = 0xea02_0000;
const MAX_VALUE: usize = 65_536;

/// One extended attribute, with its expanded namespace and binary value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtendedAttribute {
    /// Expanded attribute name. Invalid UTF-8 is replaced for display.
    pub name: String,
    /// Exact attribute bytes.
    pub value: Vec<u8>,
}

struct Entry {
    name: String,
    is_system_data: bool,
    value_inode: u32,
    size: usize,
    offset: usize,
}

fn entries(
    storage: &[u8],
    first: usize,
    base: usize,
    inode: InodeIndex,
) -> Result<Vec<Entry>, Ext4Error> {
    let invalid = || CorruptKind::Xattr(inode);
    let mut cursor = first;
    let mut result = Vec::new();
    loop {
        let end_marker = storage.get(cursor..cursor + 4).ok_or_else(invalid)?;
        if end_marker == [0; 4] {
            cursor += 4;
            break;
        }
        let header = storage.get(cursor..cursor + 16).ok_or_else(invalid)?;
        let name_length = usize::from(header[0]);
        let name_bytes = storage
            .get(cursor + 16..cursor + 16 + name_length)
            .ok_or_else(invalid)?;
        if name_bytes.contains(&0) {
            return Err(invalid().into());
        }
        let suffix = String::from_utf8_lossy(name_bytes);
        let name = match header[1] {
            0 => suffix.into_owned(),
            1 => format!("user.{suffix}"),
            2 => String::from("system.posix_acl_access"),
            3 => String::from("system.posix_acl_default"),
            4 => format!("trusted.{suffix}"),
            6 => format!("security.{suffix}"),
            7 => format!("system.{suffix}"),
            8 => String::from("system.richacl"),
            9 => format!("encryption.{suffix}"),
            index => format!("index{index}.{suffix}"),
        };
        let size = usize::try_from(read_u32le(header, 8)).map_err(|_| invalid())?;
        if size > MAX_VALUE {
            return Err(invalid().into());
        }
        result.push(Entry {
            is_system_data: header[1] == 7 && name_bytes == b"data",
            name,
            value_inode: read_u32le(header, 4),
            size,
            offset: base + usize::from(read_u16le(header, 2)),
        });
        cursor += (16 + name_length + 3) & !3;
    }
    // Values may share storage, but never overlap the table or exceed the buffer.
    for entry in &result {
        if entry.value_inode == 0
            && entry.size != 0
            && (entry.offset < cursor
                || entry.offset % 4 != 0
                || storage
                    .get(entry.offset..entry.offset + entry.size)
                    .is_none())
        {
            return Err(invalid().into());
        }
    }
    Ok(result)
}

fn inode_entries(bytes: &[u8], index: InodeIndex) -> Result<Vec<Entry>, Ext4Error> {
    if bytes.len() <= 128 {
        return Ok(Vec::new());
    }
    let extra = usize::from(read_u16le(bytes, 128));
    if extra == 0 {
        return Ok(Vec::new());
    }
    let header = 128 + extra;
    if header > bytes.len() {
        return Err(CorruptKind::Xattr(index).into());
    }
    if header + 4 > bytes.len() || read_u32le(bytes, header) != MAGIC {
        return Ok(Vec::new());
    }
    entries(bytes, header + 4, header + 4, index)
}

fn resolve(
    fs: &Ext4,
    owner: InodeIndex,
    storage: &[u8],
    entries: Vec<Entry>,
) -> Result<Vec<ExtendedAttribute>, Ext4Error> {
    let mut result = Vec::new();
    for entry in entries {
        let value = if entry.value_inode == 0 {
            if entry.size == 0 {
                Vec::new()
            } else {
                storage[entry.offset..entry.offset + entry.size].to_vec()
            }
        } else {
            if !fs
                .0
                .superblock
                .incompatible_features
                .contains(IncompatibleFeatures::LARGE_EXTENDED_ATTRIBUTES_IN_INODES)
                || entry.value_inode == owner.get()
            {
                return Err(CorruptKind::Xattr(owner).into());
            }
            let index = InodeIndex::new(entry.value_inode).ok_or(CorruptKind::Xattr(owner))?;
            let inode = Inode::read(fs, index)?;
            if !inode.metadata.file_type.is_regular_file()
                || !inode.flags.contains(InodeFlags::EXTENDED_ATTRIBUTES)
                || inode.flags.contains(InodeFlags::INLINE_DATA)
                || inode.metadata.len() != entry.size as u64
            {
                return Err(CorruptKind::Xattr(owner).into());
            }
            fs.read_inode_file(&inode)?
        };
        result.push(ExtendedAttribute {
            name: entry.name,
            value,
        });
    }
    Ok(result)
}

pub(crate) fn read(fs: &Ext4, inode: &Inode) -> Result<Vec<ExtendedAttribute>, Ext4Error> {
    let raw = Inode::read_raw_bytes(fs, inode.index)?;
    let mut result = resolve(fs, inode.index, &raw, inode_entries(&raw, inode.index)?)?;
    let high = if fs
        .0
        .superblock
        .incompatible_features
        .contains(IncompatibleFeatures::IS_64BIT)
    {
        u64::from(read_u16le(&raw, 118)) << 32
    } else {
        0
    };
    let block_number = high | u64::from(read_u32le(&raw, 104));
    if block_number != 0 {
        let mut block = vec![0; fs.0.superblock.block_size.to_usize()];
        fs.read_from_block(block_number, 0, &mut block)?;
        if read_u32le(&block, 0) != MAGIC || read_u32le(&block, 8) != 1 {
            return Err(CorruptKind::Xattr(inode.index).into());
        }
        if fs.has_metadata_checksums() {
            let mut checksum = Checksum::with_seed(fs.0.superblock.checksum_seed);
            checksum.update(&block_number.to_le_bytes());
            checksum.update(&block[..16]);
            checksum.update_u32_le(0);
            checksum.update(&block[20..]);
            if checksum.finalize() != read_u32le(&block, 16) {
                return Err(CorruptKind::Xattr(inode.index).into());
            }
        }
        result.extend(resolve(
            fs,
            inode.index,
            &block,
            entries(&block, 32, 0, inode.index)?,
        )?);
    }
    Ok(result)
}

pub(crate) fn inline_payload(fs: &Ext4, inode: &Inode) -> Result<Vec<u8>, Ext4Error> {
    let raw = Inode::read_raw_bytes(fs, inode.index)?;
    let attributes = inode_entries(&raw, inode.index)?;
    let mut system_data = attributes.iter().filter(|entry| entry.is_system_data);
    let entry = system_data
        .next()
        .ok_or(CorruptKind::InlineData(inode.index))?;
    if entry.value_inode != 0 || system_data.next().is_some() {
        return Err(CorruptKind::InlineData(inode.index).into());
    }
    let mut data = inode.inline_data.to_vec();
    if entry.size != 0 {
        data.extend_from_slice(&raw[entry.offset..entry.offset + entry.size]);
    }
    if inode.metadata.len() > data.len() as u64 {
        return Err(CorruptKind::InlineData(inode.index).into());
    }
    if !inode.metadata.is_dir() {
        data.truncate(usize::try_from(inode.metadata.len()).map_err(|_| Ext4Error::FileTooLarge)?);
    }
    Ok(data)
}
