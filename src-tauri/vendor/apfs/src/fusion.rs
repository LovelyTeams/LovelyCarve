//! Addressing and committed write-back mappings for paired APFS devices.

use crate::backing::ContainerReader;
use crate::btree::{self, BTreeNode};
use crate::error::{ApfsError, Result};
use crate::object::{self, ObjectHeader};
use crate::superblock::NxSuperblock;
use byteorder::{ByteOrder, LittleEndian};
use std::io::{Read, Seek};

pub const TIER2_OFFSET: u64 = 1 << 62;
const MIDDLE_TREE: u32 = 0x15;
const WBC_STATE: u32 = 0x16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionTier {
    Main,
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub tier: FusionTier,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug)]
pub(crate) struct CacheRange {
    pub start: u64,
    pub end: u64,
    pub main: u64,
}

pub fn validate_fusion_pair(
    main: &NxSuperblock,
    main_len: u64,
    tier2: &NxSuperblock,
    tier2_len: u64,
) -> Result<()> {
    let (Some((main_id, FusionTier::Main)), Some((other_id, FusionTier::Secondary))) =
        (main.fusion_member(), tier2.fusion_member())
    else {
        return Err(corrupt("Fusion requires one main and one tier-2 member"));
    };
    let block = u64::from(main.block_size);
    if main_id != other_id
        || main.uuid != tier2.uuid
        || main.block_size != tier2.block_size
        || main.block_count != tier2.block_count
        || main.incompatible_features != tier2.incompatible_features
        || main.header.xid != tier2.header.xid
    {
        return Err(corrupt(
            "Fusion superblocks disagree on identity, transaction or geometry",
        ));
    }
    if block == 0
        || !(block..TIER2_OFFSET).contains(&main_len)
        || !(block..TIER2_OFFSET).contains(&tier2_len)
        || main_len / block + tier2_len / block < main.block_count
    {
        return Err(corrupt("Fusion geometry exceeds its members"));
    }
    Ok(())
}

fn corrupt(message: &'static str) -> ApfsError {
    ApfsError::CorruptedData(message.into())
}

pub(crate) fn check_checkpoint_region(nx: &NxSuperblock, main_len: u64) -> Result<()> {
    let end = nx.xp_desc_base.checked_add(u64::from(nx.xp_desc_blocks));
    if nx.xp_desc_base == 0 || end.is_none_or(|end| end > main_len / u64::from(nx.block_size)) {
        return Err(corrupt(
            "Fusion checkpoint descriptors exceed the main device",
        ));
    }
    Ok(())
}

fn ephemeral_locations<R: Read + Seek>(
    reader: &mut ContainerReader<R>,
    nx: &NxSuperblock,
) -> Result<[u64; 2]> {
    let count = u64::from(nx.xp_desc_blocks);
    let used = u64::from(nx.xp_desc_len);
    let index = u64::from(nx.xp_desc_index);
    if used < 2 || used > count || index >= count || nx.fusion_wbc_oid == 0 || nx.spaceman_oid == 0
    {
        return Err(corrupt("invalid Fusion checkpoint descriptors"));
    }
    let mut located = [None, None];
    for i in 0..used - 1 {
        let block = nx.xp_desc_base + (index + i) % count;
        let (header, bytes) = object::read_object(reader, block, nx.block_size)?;
        if header.object_type() != object::OBJECT_TYPE_CHECKPOINT_MAP
            || !header.is_physical()
            || header.oid != block
            || header.xid != nx.header.xid
        {
            return Err(corrupt(
                "Fusion checkpoint map does not match selected transaction",
            ));
        }
        let flag = LittleEndian::read_u32(&bytes[32..]);
        let entries = LittleEndian::read_u32(&bytes[36..]) as usize;
        if flag != u32::from(i + 2 == used) || entries > (bytes.len() - 40) / 40 {
            return Err(corrupt("malformed Fusion checkpoint map"));
        }
        for entry in bytes[40..40 + entries * 40].as_chunks::<40>().0 {
            let oid = LittleEndian::read_u64(&entry[24..]);
            let (slot, kind) = if oid == nx.spaceman_oid {
                (0, object::OBJECT_TYPE_SPACEMAN)
            } else if oid == nx.fusion_wbc_oid {
                (1, WBC_STATE)
            } else {
                continue;
            };
            if LittleEndian::read_u32(entry) != object::OBJ_EPHEMERAL | kind
                || LittleEndian::read_u32(&entry[8..]) != nx.block_size
                || LittleEndian::read_u64(&entry[16..]) != 0
                || located[slot]
                    .replace(LittleEndian::read_u64(&entry[32..]))
                    .is_some()
            {
                return Err(corrupt("malformed or duplicate Fusion checkpoint mapping"));
            }
        }
    }
    match located {
        [Some(spaceman), Some(wbc)] => Ok([spaceman, wbc]),
        _ => Err(corrupt(
            "Fusion checkpoint is missing spaceman or write-back state",
        )),
    }
}

pub(crate) fn load_fusion_state<R: Read + Seek>(
    reader: &mut ContainerReader<R>,
    nx: &NxSuperblock,
) -> Result<()> {
    let [spaceman, wbc] = ephemeral_locations(reader, nx)?;
    let read_ephemeral = |reader: &mut ContainerReader<R>, block, oid, kind| -> Result<Vec<u8>> {
        let (header, bytes) = object::read_object(reader, block, nx.block_size)?;
        if header.type_and_flags != object::OBJ_EPHEMERAL | kind
            || header.oid != oid
            || header.xid != nx.header.xid
        {
            return Err(corrupt("Fusion checkpoint object identity mismatch"));
        }
        Ok(bytes)
    };
    let space = read_ephemeral(
        reader,
        spaceman,
        nx.spaceman_oid,
        object::OBJECT_TYPE_SPACEMAN,
    )?;
    let main_blocks = LittleEndian::read_u64(&space[48..]);
    let tier2_blocks = LittleEndian::read_u64(&space[96..]);
    if LittleEndian::read_u32(&space[32..]) != nx.block_size
        || main_blocks.checked_add(tier2_blocks) != Some(nx.block_count)
    {
        return Err(corrupt("Fusion spaceman disagrees with container geometry"));
    }
    reader.limit_fusion(main_blocks, tier2_blocks, nx.block_size)?;
    check_checkpoint_region(nx, reader.range_limits().0)?;

    let state = read_ephemeral(reader, wbc, nx.fusion_wbc_oid, WBC_STATE)?;
    if state[40..76]
        .iter()
        .chain(&state[80..104])
        .any(|&byte| byte != 0)
    {
        return Err(ApfsError::Unsupported(
            "APFS Fusion write-back queue requires replay".into(),
        ));
    }
    let root = nx.fusion_tree;
    let (header, bytes) = object::read_object(reader, root, nx.block_size)?;
    check_tree_node(&header, root, nx.header.xid, true)?;
    let root_node = BTreeNode::parse(&bytes)?;
    let info = root_node
        .info
        .as_ref()
        .ok_or_else(|| corrupt("missing Fusion middle-tree root footer"))?;
    if info.bt_fixed.bt_flags & btree::BTREE_PHYSICAL == 0
        || info.bt_fixed.bt_node_size != nx.block_size
    {
        return Err(corrupt("Fusion middle tree has invalid geometry"));
    }

    let mut bad_record = None;
    let mut ranges = Vec::new();
    let mut previous_end = TIER2_OFFSET;
    let block = u64::from(nx.block_size);
    let tier2_end = TIER2_OFFSET + tier2_blocks * block;
    let main_end = main_blocks * block;
    let mut visit = |key: &[u8], value: &[u8]| match read_cache_range(
        key,
        value,
        block,
        previous_end,
        main_end,
        tier2_end,
    ) {
        Ok((end, range)) => {
            previous_end = end;
            ranges.extend(range);
            true
        }
        Err(error) => {
            bad_record = Some(error);
            false
        }
    };
    btree::walk_checked_physical(
        reader,
        root,
        nx.block_size,
        &mut visit,
        |header, address, is_root| check_tree_node(header, address, nx.header.xid, is_root),
    )?;
    if let Some(error) = bad_record {
        return Err(error);
    }
    reader.install_fusion_cache(ranges);
    Ok(())
}

fn check_tree_node(header: &ObjectHeader, block: u64, xid: u64, root: bool) -> Result<()> {
    let kind = if root {
        object::OBJECT_TYPE_BTREE
    } else {
        object::OBJECT_TYPE_BTREE_NODE
    };
    if header.type_and_flags != object::OBJ_PHYSICAL | kind
        || header.subtype != MIDDLE_TREE
        || header.oid != block
        || header.xid > xid
    {
        return Err(corrupt("invalid Fusion middle-tree object"));
    }
    Ok(())
}

fn read_cache_range(
    key: &[u8],
    value: &[u8],
    block: u64,
    previous_end: u64,
    main_end: u64,
    tier2_end: u64,
) -> Result<(u64, Option<CacheRange>)> {
    if key.len() != 8 || value.len() != 16 {
        return Err(corrupt("malformed Fusion middle-tree record"));
    }
    let start = LittleEndian::read_u64(key)
        .checked_mul(block)
        .ok_or_else(|| corrupt("Fusion tier-2 address overflow"))?;
    let main = LittleEndian::read_u64(value)
        .checked_mul(block)
        .ok_or_else(|| corrupt("Fusion main address overflow"))?;
    let length = u64::from(LittleEndian::read_u32(&value[8..])) * block;
    let flags = LittleEndian::read_u32(&value[12..]);
    let end = start
        .checked_add(length)
        .filter(|&end| end <= tier2_end)
        .ok_or_else(|| corrupt("Fusion cache record exceeds tier 2"))?;
    if start < previous_end
        || length == 0
        || main < block
        || main.checked_add(length).is_none_or(|end| end > main_end)
    {
        return Err(corrupt("invalid Fusion cache range"));
    }
    match flags {
        0 => Ok((end, None)),
        1 => Ok((end, Some(CacheRange { start, end, main }))),
        _ => Err(ApfsError::Unsupported(
            "Fusion middle-tree record flags".into(),
        )),
    }
}
