// Ported from refsprogs parse_extent / parse_extent_tree.
use std::collections::HashSet;

use crate::bytes::{slice, u16_at, u32_at, u64_at};
use crate::metadata::{physical_block, read_node, BlockRange};
use crate::table::{LeafLayout, TablePage};
use crate::{corrupt, BootSector, NodeReference, ReadAt, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    pub logical: u64,
    /// None means an explicitly unallocated or unwritten zero-filled span.
    pub physical: Option<u64>,
    pub length: u64,
}

pub(crate) fn parse_list(
    list: &[u8],
    boot: &BootSector,
    mappings: &[BlockRange],
    source: &dyn ReadAt,
) -> Result<Vec<Extent>> {
    let indexed = u32_at(list, 12)? & 1 != 0;
    let modern_layout = (boot.major, boot.minor) >= (3, 5);
    let count = u32_at(list, if modern_layout { 20 } else { 16 })? as usize;
    let mut offsets = Vec::with_capacity(count.min(list.len() / 4));
    if modern_layout {
        let index = u32_at(list, 16)? as usize;
        let array = slice(
            list,
            index,
            count
                .checked_mul(4)
                .ok_or_else(|| corrupt("extent index overflow"))?,
        )?;
        for pair in array.as_chunks::<4>().0 {
            offsets.push(u16_at(pair, 0)? as usize);
        }
    } else {
        let mut offset = u32_at(list, 0)? as usize;
        if count > list.len() / 16 {
            return Err(corrupt("extent count exceeds list"));
        }
        for _ in 0..count {
            offsets.push(offset);
            let size = if indexed {
                u32_at(list, offset)? as usize
            } else {
                48
            };
            if size < 16 {
                return Err(corrupt("invalid extent entry size"));
            }
            offset = offset
                .checked_add(size)
                .ok_or_else(|| corrupt("extent offset overflow"))?;
        }
    }
    let mut extents = Vec::new();
    let mut stack = Vec::new();
    for offset in offsets {
        if indexed {
            let row = TablePage::parse_row(
                slice(
                    list,
                    offset,
                    list.len()
                        .checked_sub(offset)
                        .ok_or_else(|| corrupt("invalid extent offset"))?,
                )?,
                0,
            )?;
            stack.push(NodeReference::parse(&row.value, 0, boot.modern())?);
        } else {
            let size = if boot.major >= 3 { 24 } else { 48 };
            append_extent(slice(list, offset, size)?, boot, mappings, &mut extents)?;
        }
    }
    let mut visited = HashSet::new();
    let mut object_id = None;
    while let Some(reference) = stack.pop() {
        if !visited.insert(reference.blocks[0]) {
            return Err(corrupt("cycle or duplicate node in extent tree"));
        }
        let bytes = read_node(source, boot, mappings, reference)?;
        let page =
            TablePage::parse_with_layout(&bytes, reference.blocks[0], boot, LeafLayout::Extent)?;
        if object_id.is_some_and(|expected| page.object_id != expected) {
            return Err(corrupt("extent tree object identity mismatch"));
        }
        object_id = Some(page.object_id);
        if page.index {
            stack.extend(page.children(boot)?);
        } else {
            for row in page.rows {
                append_extent(&row.value, boot, mappings, &mut extents)?;
            }
        }
    }
    extents.sort_by_key(|extent| extent.logical);
    let mut previous_end = 0;
    for extent in &extents {
        if extent.logical < previous_end {
            return Err(corrupt("overlapping file extents"));
        }
        previous_end = extent
            .logical
            .checked_add(extent.length)
            .ok_or_else(|| corrupt("extent logical range overflow"))?;
    }
    Ok(extents)
}

pub(crate) fn append_extent(
    bytes: &[u8],
    boot: &BootSector,
    mappings: &[BlockRange],
    extents: &mut Vec<Extent>,
) -> Result<()> {
    let (mut logical, mut virtual_block, mut count) = if boot.major >= 3 {
        (
            u64_at(bytes, 12)?,
            u64_at(bytes, 0)?,
            u64::from(u32_at(bytes, 20)?),
        )
    } else {
        (u64_at(bytes, 16)?, u64_at(bytes, 32)?, u64_at(bytes, 24)?)
    };
    if count == 0 {
        return Ok(());
    }
    let flags = u32_at(bytes, if boot.major >= 3 { 8 } else { 40 })?;
    // Extent flags encode allocation separately from written data: bit 0x20
    // marks a hole and bit 0x10 marks written content. A known unwritten run
    // must read as zeros even if its allocated clusters contain old bytes.
    let zero_filled = virtual_block == 0 || flags & 0x20 != 0 || (flags != 0 && flags & 0x10 == 0);
    let unit = boot.block_unit();
    let chunk = if boot.cluster_size == 4096 {
        16384
    } else {
        1024
    };
    while count != 0 {
        // Virtual chunks can map to nonadjacent physical regions. Never assume
        // an extent crossing a chunk boundary is physically contiguous.
        let piece = count.min(chunk - virtual_block % chunk);
        let physical = if zero_filled {
            None
        } else {
            Some(
                physical_block(boot, mappings, virtual_block)?
                    .checked_mul(unit)
                    .ok_or_else(|| corrupt("extent physical offset overflow"))?,
            )
        };
        let length = piece
            .checked_mul(unit)
            .ok_or_else(|| corrupt("extent size overflow"))?;
        let logical_bytes = logical
            .checked_mul(unit)
            .ok_or_else(|| corrupt("extent logical offset overflow"))?;
        if physical.is_some_and(|physical| {
            physical
                .checked_add(length)
                .is_none_or(|end| end > boot.volume_size)
        }) {
            return Err(corrupt("file extent exceeds volume"));
        }
        extents.push(Extent {
            logical: logical_bytes,
            physical,
            length,
        });
        logical = logical
            .checked_add(piece)
            .ok_or_else(|| corrupt("extent logical block overflow"))?;
        virtual_block = virtual_block
            .checked_add(piece)
            .ok_or_else(|| corrupt("extent virtual block overflow"))?;
        count -= piece;
    }
    Ok(())
}
