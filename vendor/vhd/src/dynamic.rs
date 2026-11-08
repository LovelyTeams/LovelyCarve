//! Dynamic VHD Block Allocation Table (BAT) parsing (MS-VHD §2.3).

use crate::error::{Result, VhdError};
use crate::read::be_u32;
use crate::read::be_u64;

pub const DYNAMIC_HEADER_COOKIE: &[u8; 8] = b"cxsparse";
pub const DYNAMIC_HEADER_SIZE: usize = 1024;
pub const BAT_ENTRY_UNUSED: u32 = 0xFFFF_FFFF;
/// Sector bitmap size in sectors (always 1 sector = 512 bytes per block).
/// Compute the bitmap size in sectors for a given block_size (MS-VHD §2.3).
///
/// Formula from the spec and QEMU vpc.c:
///   bitmap_bytes = ((block_size / (8 * 512)) + 511) & !511
///   bitmap_sectors = bitmap_bytes / 512
pub fn bitmap_sectors(block_size: u32) -> u64 {
    let bitmap_bytes = (u64::from(block_size).div_ceil(8 * 512) + 511) & !511;
    bitmap_bytes / 512
}

/// Parsed dynamic header fields.
pub struct DynamicHeader {
    pub bat_offset: u64,
    pub block_size: u32,
    pub max_bat_entries: u32,
}

impl DynamicHeader {
    pub fn parse(data: &[u8], offset: u64) -> Result<Self> {
        let start = usize::try_from(offset).map_err(|_| VhdError::BatOutOfBounds)?;
        let end = start
            .checked_add(DYNAMIC_HEADER_SIZE)
            .ok_or(VhdError::BatOutOfBounds)?;
        if end > data.len() {
            return Err(VhdError::BatOutOfBounds);
        }
        let hdr = &data[start..end];
        if &hdr[0..8] != DYNAMIC_HEADER_COOKIE {
            return Err(VhdError::BadCookie);
        }
        let bat_offset = be_u64(hdr, 16);
        let block_size = be_u32(hdr, 32);
        let max_bat_entries = be_u32(hdr, 28);
        if block_size < 512 || !block_size.is_power_of_two() {
            return Err(VhdError::InvalidBlockSize);
        }
        Ok(DynamicHeader {
            bat_offset,
            block_size,
            max_bat_entries,
        })
    }
}

/// In-memory Block Allocation Table.
pub struct BlockAllocationTable {
    /// File-sector offset for each block (BAT_ENTRY_UNUSED if not present).
    entries: Vec<u32>,
    pub block_size: u32,
}

impl BlockAllocationTable {
    pub fn parse(data: &[u8], hdr: &DynamicHeader) -> Result<Self> {
        let start = usize::try_from(hdr.bat_offset).map_err(|_| VhdError::BatOutOfBounds)?;
        let entry_count = hdr.max_bat_entries as usize;
        let table_bytes = entry_count.checked_mul(4).ok_or(VhdError::BatOutOfBounds)?;
        let end = start
            .checked_add(table_bytes)
            .ok_or(VhdError::BatOutOfBounds)?;
        if end > data.len() {
            return Err(VhdError::BatOutOfBounds);
        }
        let entries = (0..entry_count)
            .map(|i| be_u32(data, start + i * 4))
            .collect();
        Ok(BlockAllocationTable {
            entries,
            block_size: hdr.block_size,
        })
    }

    /// Validate allocated payloads before exposing logical disk bytes.
    pub fn validate_payload_bounds(&self, footer_offset: u64) -> Result<()> {
        let span = bitmap_sectors(self.block_size) * 512 + u64::from(self.block_size);
        for &entry in &self.entries {
            if entry != BAT_ENTRY_UNUSED && u64::from(entry) * 512 + span > footer_offset {
                return Err(VhdError::BlockOutOfBounds);
            }
        }
        Ok(())
    }

    /// Resolve a virtual byte offset to a file byte offset.
    ///
    /// Returns `None` if the block is not present (sparse / unwritten region,
    /// reads should return zeroes).
    pub fn file_offset_for_byte(&self, virtual_byte: u64) -> Result<Option<u64>> {
        let block_size = u64::from(self.block_size);
        let block_index = (virtual_byte / block_size) as usize;
        if block_index >= self.entries.len() {
            return Err(VhdError::BlockOutOfBounds);
        }
        let bat_entry = self.entries[block_index];
        if bat_entry == BAT_ENTRY_UNUSED {
            return Ok(None);
        }
        // BAT entry is in 512-byte sectors; each block is preceded by a sector bitmap
        // whose size depends on block_size (see bitmap_sectors()).
        let block_file_offset = u64::from(bat_entry) * 512 + bitmap_sectors(self.block_size) * 512;
        let offset_in_block = virtual_byte % block_size;
        Ok(Some(block_file_offset + offset_in_block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_rounds_up_for_sub_4096_byte_blocks() {
        assert_eq!(bitmap_sectors(512), 1);
        assert_eq!(bitmap_sectors(2048), 1);
        assert_eq!(bitmap_sectors(4 * 1024 * 1024), 2);
    }

    #[test]
    fn dynamic_header_parse_huge_offset_returns_err_no_panic() {
        // A malicious footer data_offset must not overflow start + header size.
        let r = DynamicHeader::parse(&[0u8; 64], u64::MAX);
        assert!(r.is_err(), "huge offset must be a clean error, not a panic");
    }

    #[test]
    fn bat_parse_overflow_returns_err_no_panic() {
        // bat_offset + max_bat_entries*4 must not overflow past the bounds check
        // (which would let a 16 GiB allocation through).
        let hdr = DynamicHeader {
            bat_offset: u64::MAX,
            block_size: 512,
            max_bat_entries: 0xFFFF_FFFF,
        };
        let r = BlockAllocationTable::parse(&[0u8; 64], &hdr);
        assert!(r.is_err(), "overflowing BAT geometry must error, not panic");
    }

    #[test]
    fn header_end_beyond_data_is_out_of_bounds() {
        // valid offset, but the header extends past the file.
        assert!(matches!(
            DynamicHeader::parse(&[0u8; 1000], 0),
            Err(VhdError::BatOutOfBounds)
        ));
    }

    #[test]
    fn header_bad_cookie_rejected() {
        assert!(matches!(
            DynamicHeader::parse(&[0u8; 1024], 0),
            Err(VhdError::BadCookie)
        ));
    }

    #[test]
    fn block_index_beyond_bat_is_out_of_bounds() {
        let bat = BlockAllocationTable {
            entries: vec![BAT_ENTRY_UNUSED],
            block_size: 512,
        };
        assert!(matches!(
            bat.file_offset_for_byte(512 * 100),
            Err(VhdError::BlockOutOfBounds)
        ));
    }

    #[test]
    fn bat_parse_huge_entry_count_is_not_an_alloc_bomb() {
        // max_bat_entries claims 4 billion entries but the file is tiny → must
        // error out before allocating, not OOM.
        let hdr = DynamicHeader {
            bat_offset: 0,
            block_size: 512,
            max_bat_entries: 0xFFFF_FFFF,
        };
        let r = BlockAllocationTable::parse(&[0u8; 64], &hdr);
        assert!(r.is_err());
    }
}
