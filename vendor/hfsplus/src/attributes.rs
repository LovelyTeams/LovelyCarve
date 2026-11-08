//! Attributes B-tree: extended attributes.
//!
//! Keyed by `(fileID, attrName, startBlock)` and compared in that order, with
//! the name as a 16-bit binary comparison — `hfs_attrkeycompare`,
//! `core/hfs_xattr.c`: *"The name portion of the key is compared using a
//! 16-bit binary comparison."*
//!
//! Values take one of three record types (`core/hfs_format.h`):
//! `kHFSPlusAttrInlineData` holds the value in the node,
//! `kHFSPlusAttrForkData` holds a fork to read it from, and
//! `kHFSPlusAttrExtents` carries overflow extents for a fork of more than
//! eight.

use std::io::{Read, Seek};

use crate::btree::{self, BTreeHeaderRecord};
use crate::error::{HfsPlusError, Result};
use crate::unicode;
use crate::volume::{ExtentDescriptor, ForkData};

/// Value fits in the B-tree node.
pub const RECORD_TYPE_INLINE_DATA: u32 = 0x10;
/// Value lives in allocation blocks described by a fork.
pub const RECORD_TYPE_FORK_DATA: u32 = 0x20;
/// Overflow extents for a fork of more than eight extents.
pub const RECORD_TYPE_EXTENTS: u32 = 0x30;

/// Offset of `attrNameLen` within an attribute key record, past the 2-byte
/// `keyLength`, 2-byte `pad`, 4-byte `fileID` and 4-byte `startBlock`.
const KEY_NAME_LEN_OFFSET: usize = 12;
const KEY_NAME_OFFSET: usize = 14;

/// Where an attribute keeps its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrValue {
    /// The value is in the record.
    Inline(Vec<u8>),
    /// The value is in allocation blocks. Read it with the fork readers, as
    /// for file data.
    Fork(ForkData),
}

/// Fields of an attribute key needed for ordering and for naming a record.
struct AttrKey {
    file_id: u32,
    start_block: u32,
    name: Vec<u16>,
    /// Offset of the record value, past the key and its alignment padding.
    value_offset: usize,
}

impl AttrKey {
    fn parse(record: &[u8]) -> Result<Self> {
        let header = record
            .get(0..KEY_NAME_OFFSET)
            .ok_or_else(|| HfsPlusError::InvalidBTree("attribute key too short".into()))?;

        let key_length = u16::from_be_bytes([header[0], header[1]]) as usize;
        let file_id = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        let start_block = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        let name_len =
            u16::from_be_bytes([header[KEY_NAME_LEN_OFFSET], header[KEY_NAME_LEN_OFFSET + 1]])
                as usize;

        let name_end = name_len
            .checked_mul(2)
            .and_then(|n| KEY_NAME_OFFSET.checked_add(n))
            .ok_or_else(|| HfsPlusError::InvalidBTree("attribute name length overflow".into()))?;
        let name_data = record.get(KEY_NAME_OFFSET..name_end).ok_or_else(|| {
            HfsPlusError::InvalidBTree(format!(
                "attribute name extends beyond record: name_end={name_end}, record_len={}",
                record.len()
            ))
        })?;

        // The value follows the key, which is measured from just after the
        // keyLength field itself and padded to even alignment.
        let value_offset = key_length
            .checked_add(2)
            .map(|o| o.next_multiple_of(2))
            .ok_or_else(|| HfsPlusError::InvalidBTree("attribute key length overflow".into()))?;

        if name_len > 127 || name_end > value_offset || value_offset > record.len() {
            return Err(HfsPlusError::InvalidBTree(
                "invalid attribute key boundaries".into(),
            ));
        }
        Ok(AttrKey {
            file_id,
            start_block,
            name: unicode::utf16be_to_u16(name_data),
            value_offset,
        })
    }

    fn to_name(&self) -> Result<String> {
        String::from_utf16(&self.name).map_err(|e| {
            HfsPlusError::CorruptedData(format!(
                "attribute name on file {} is not valid Unicode: {e}",
                self.file_id
            ))
        })
    }
}

/// Order a record against `(file_id, name, start_block)`.
///
/// An undecodable key fails the operation, matching the other comparators in
/// this crate: any ordering guessed for it would steer the descent past the
/// bad record and report an attribute that exists as absent.
/// PROVISIONAL(anomaly-channel): fail now, degrade to a reported miss once
/// there is somewhere to report to.
fn compare_key(
    record: &[u8],
    file_id: u32,
    name: &[u16],
    start_block: u32,
) -> Result<std::cmp::Ordering> {
    let key = AttrKey::parse(record)?;
    Ok(key
        .file_id
        .cmp(&file_id)
        .then_with(|| unicode::compare_binary(&key.name, name))
        .then_with(|| key.start_block.cmp(&start_block)))
}

/// Look up one extended attribute, or `None` when the file has no attribute of
/// that name.
pub fn lookup<R: Read + Seek>(
    reader: &mut R,
    attributes_btree: &BTreeHeaderRecord,
    file_id: u32,
    name: &str,
) -> Result<Option<AttrValue>> {
    let name: Vec<u16> = name.encode_utf16().collect();
    let comparator = |record: &[u8]| compare_key(record, file_id, &name, 0);

    let Some((node, index)) = btree::search_btree(reader, attributes_btree, &comparator)? else {
        return Ok(None);
    };
    let record = node.record_data(index)?;
    parse_value(record).map(Some)
}

/// Names of every extended attribute on a file, in on-disk order.
///
/// Scans the leaves from the start of the tree rather than descending to the
/// file, since a lookup needs a name to descend with. Stops at the first
/// record past `file_id`.
pub fn list_names<R: Read + Seek>(
    reader: &mut R,
    attributes_btree: &BTreeHeaderRecord,
    file_id: u32,
) -> Result<Vec<String>> {
    let match_fn = |record: &[u8]| -> Result<Option<bool>> {
        let key = AttrKey::parse(record)?;
        Ok(match key.file_id.cmp(&file_id) {
            std::cmp::Ordering::Less => Some(false),
            std::cmp::Ordering::Equal => Some(key.start_block == 0),
            std::cmp::Ordering::Greater => None,
        })
    };
    let parse_fn = |record: &[u8]| AttrKey::parse(record)?.to_name();

    btree::scan_leaves(
        reader,
        attributes_btree,
        attributes_btree.first_leaf_node,
        &match_fn,
        &parse_fn,
    )
}

/// Collect an attribute fork through its own Attributes-tree continuation records.
/// Attribute extents do not belong to the file's data-fork overflow tree.
pub fn fork_extents<R: Read + Seek>(
    reader: &mut R,
    tree: &BTreeHeaderRecord,
    volume: &crate::VolumeHeader,
    file_id: u32,
    name: &str,
    fork: &ForkData,
) -> Result<Vec<ExtentDescriptor>> {
    let name: Vec<u16> = name.encode_utf16().collect();
    let mut result = Vec::new();
    let mut allocated = 0u32;
    crate::extents::append_extents(&mut result, &mut allocated, &fork.extents, volume, fork)?;
    while allocated < fork.total_blocks {
        let comparator = |record: &[u8]| compare_key(record, file_id, &name, allocated);
        let Some((node, index)) = btree::search_btree(reader, tree, &comparator)? else {
            return Err(HfsPlusError::CorruptedData(
                "missing attribute extent continuation".into(),
            ));
        };
        let record = node.record_data(index)?;
        let key = AttrKey::parse(record)?;
        let value = record
            .get(key.value_offset..)
            .ok_or_else(|| HfsPlusError::InvalidBTree("attribute continuation truncated".into()))?;
        if be_u32(value, 0)? != RECORD_TYPE_EXTENTS {
            return Err(HfsPlusError::CorruptedData(
                "invalid attribute continuation type".into(),
            ));
        }
        let mut extents = [ExtentDescriptor::default(); 8];
        for (index, extent) in extents.iter_mut().enumerate() {
            extent.start_block = be_u32(value, 8 + index * 8)?;
            extent.block_count = be_u32(value, 12 + index * 8)?;
        }
        let previous = allocated;
        crate::extents::append_extents(&mut result, &mut allocated, &extents, volume, fork)?;
        if allocated == previous {
            return Err(HfsPlusError::CorruptedData(
                "empty attribute continuation".into(),
            ));
        }
    }
    if fork.logical_size > u64::from(allocated) * u64::from(volume.block_size) {
        return Err(HfsPlusError::CorruptedData(
            "attribute exceeds allocation".into(),
        ));
    }
    Ok(result)
}

/// Parse an attribute record's value.
fn parse_value(record: &[u8]) -> Result<AttrValue> {
    let key = AttrKey::parse(record)?;
    let value = record.get(key.value_offset..).ok_or_else(|| {
        HfsPlusError::InvalidBTree(format!(
            "attribute value starts past the record: offset={}, record_len={}",
            key.value_offset,
            record.len()
        ))
    })?;

    let record_type = be_u32(value, 0)?;
    match record_type {
        // recordType u32, reserved[2] u32, attrSize u32, attrData[]
        RECORD_TYPE_INLINE_DATA => {
            let size = be_u32(value, 12)? as usize;
            let data = value.get(16..).unwrap_or_default();
            if data.len() < size {
                return Err(HfsPlusError::CorruptedData(format!(
                    "inline attribute declares {size} bytes, record holds {}",
                    data.len()
                )));
            }
            Ok(AttrValue::Inline(data[..size].to_vec()))
        }
        // recordType u32, reserved u32, HFSPlusForkData
        RECORD_TYPE_FORK_DATA => Ok(AttrValue::Fork(parse_fork_data(value, 8)?)),
        RECORD_TYPE_EXTENTS => Err(HfsPlusError::CorruptedData(
            "attribute lookup landed on an overflow extent record, which has no value".into(),
        )),
        other => Err(HfsPlusError::CorruptedData(format!(
            "unknown attribute record type {other:#x}"
        ))),
    }
}

/// `HFSPlusForkData`: logical size, clump size, total blocks, then eight
/// extent descriptors.
fn parse_fork_data(value: &[u8], offset: usize) -> Result<ForkData> {
    let logical_size =
        u64::from(be_u32(value, offset)?) << 32 | u64::from(be_u32(value, offset + 4)?);
    let clump_size = be_u32(value, offset + 8)?;
    let total_blocks = be_u32(value, offset + 12)?;

    let mut extents = [ExtentDescriptor {
        start_block: 0,
        block_count: 0,
    }; 8];
    for (i, extent) in extents.iter_mut().enumerate() {
        let at = offset + 16 + i * 8;
        extent.start_block = be_u32(value, at)?;
        extent.block_count = be_u32(value, at + 4)?;
    }

    Ok(ForkData {
        logical_size,
        clump_size,
        total_blocks,
        extents,
    })
}

fn be_u32(buf: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| HfsPlusError::InvalidBTree("attribute record offset overflow".into()))?;
    let bytes = buf.get(offset..end).ok_or_else(|| {
        HfsPlusError::InvalidBTree(format!(
            "attribute record truncated: need {end} bytes, have {}",
            buf.len()
        ))
    })?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests;
