// Ported from refsprogs parse_generic_block / parse_generic_entry.
use crate::bytes::{slice, u16_at, u32_at};
use crate::metadata::validate_header;
use crate::{corrupt, BootSector, NodeReference, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub inode: Option<u64>,
}

pub(crate) struct TablePage {
    pub object_id: u64,
    pub index: bool,
    pub rows: Vec<Row>,
}

#[derive(Clone, Copy)]
pub(crate) enum LeafLayout {
    KeyValue,
    Extent,
}

impl TablePage {
    pub(crate) fn parse(bytes: &[u8], block: u64, boot: &BootSector) -> Result<Self> {
        Self::parse_with_layout(bytes, block, boot, LeafLayout::KeyValue)
    }

    pub(crate) fn parse_with_layout(
        bytes: &[u8],
        block: u64,
        boot: &BootSector,
        layout: LeafLayout,
    ) -> Result<Self> {
        let object_id = validate_header(bytes, block, boot, b"MSB+")?;
        let mut page =
            Self::parse_body_with_layout(bytes, boot.header_size(), object_id, boot, layout)?;
        for row in &mut page.rows {
            row.inode = row.inode.map(|offset| block.wrapping_shl(16) | offset);
        }
        Ok(page)
    }

    fn parse_body_with_layout(
        bytes: &[u8],
        header: usize,
        object_id: u64,
        boot: &BootSector,
        layout: LeafLayout,
    ) -> Result<Self> {
        let header_size = u32_at(bytes, header)? as usize;
        if header_size < 8 {
            return Err(corrupt("invalid table header size"));
        }
        slice(bytes, header, header_size)?;
        let allocation = header
            .checked_add(header_size)
            .ok_or_else(|| corrupt("table header overflow"))?;
        Self::parse_allocation_with_layout(bytes, allocation, object_id, boot, layout)
    }

    pub(crate) fn parse_allocation(
        bytes: &[u8],
        allocation: usize,
        object_id: u64,
        boot: &BootSector,
    ) -> Result<Self> {
        Self::parse_allocation_with_layout(bytes, allocation, object_id, boot, LeafLayout::KeyValue)
    }

    fn parse_allocation_with_layout(
        bytes: &[u8],
        allocation: usize,
        object_id: u64,
        boot: &BootSector,
        layout: LeafLayout,
    ) -> Result<Self> {
        let allocation_size = u32_at(bytes, allocation)? as usize;
        let allocation_bytes = slice(bytes, allocation, allocation_size)?;
        let index = u32_at(allocation_bytes, 0xC)? & 0x100 != 0;
        let start = u32_at(allocation_bytes, 0x10)? as usize;
        let count = u32_at(allocation_bytes, 0x14)? as usize;
        let end = u32_at(allocation_bytes, if boot.major >= 3 { 0x20 } else { 0x18 })? as usize;
        if start < allocation_size || end < start || count > (end - start) / 4 {
            return Err(corrupt("invalid table offset array"));
        }
        let offsets_start = allocation
            .checked_add(start)
            .ok_or_else(|| corrupt("row offsets overflow"))?;
        let offsets = slice(
            bytes,
            offsets_start,
            count
                .checked_mul(4)
                .ok_or_else(|| corrupt("row count overflow"))?,
        )?;
        slice(bytes, allocation, end)?;
        let mut rows = Vec::with_capacity(count);
        for pair in offsets.as_chunks::<4>().0 {
            let relative = u16_at(pair, 0)? as usize;
            if relative < allocation_size {
                return Err(corrupt("row overlaps allocation header"));
            }
            let offset = allocation
                .checked_add(relative)
                .ok_or_else(|| corrupt("row offset overflow"))?;
            let fixed_extent = !index && matches!(layout, LeafLayout::Extent);
            let length = if fixed_extent {
                24
            } else {
                u32_at(bytes, offset)? as usize
            };
            if length < 16
                || offset
                    .checked_add(length)
                    .is_none_or(|value| value > offsets_start)
            {
                return Err(corrupt("row crosses table offset array"));
            }
            let row = slice(bytes, offset, length)?;
            if fixed_extent {
                rows.push(Row {
                    key: Vec::new(),
                    value: row.to_vec(),
                    inode: Some(offset as u64),
                });
                continue;
            }
            let mut row = Self::parse_row(row, object_id)?;
            row.inode = Some(offset as u64);
            rows.push(row);
        }
        Ok(Self {
            object_id,
            index,
            rows,
        })
    }

    pub(crate) fn parse_row(row: &[u8], object_id: u64) -> Result<Row> {
        let length = u32_at(row, 0)? as usize;
        let row = slice(row, 0, length)?;
        let key_offset = u16_at(row, 4)? as usize;
        let key_length = u16_at(row, 6)? as usize;
        let value_offset = u16_at(row, 10)? as usize;
        let value_length = u16_at(row, 12)? as usize;
        if value_offset < 16 || (key_length != 0 && key_offset < 16) {
            return Err(corrupt(format!("invalid row key / value bounds in object {object_id:#x}: size={length}, key={key_offset}+{key_length}, value={value_offset}+{value_length}")));
        }
        Ok(Row {
            // Mapping leaves can advertise a key inside their value. The
            // C reader treats that as a value-only row; keep that behavior.
            key: if object_id != 0xB
                || key_offset
                    .checked_add(key_length)
                    .is_some_and(|end| end <= value_offset)
            {
                slice(row, key_offset, key_length)?.to_vec()
            } else {
                slice(row, key_offset, key_length)?;
                Vec::new()
            },
            value: slice(row, value_offset, value_length)?.to_vec(),
            inode: None,
        })
    }

    pub(crate) fn children(&self, boot: &BootSector) -> Result<Vec<NodeReference>> {
        self.rows
            .iter()
            .map(|row| NodeReference::parse(&row.value, 0, boot.modern()))
            .collect()
    }
}
