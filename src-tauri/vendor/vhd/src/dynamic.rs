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

    /// Return `(payload_file_offset, bitmap_byte_file_offset, bitmap_bit_mask, sector_in_block, sectors_per_block)`
    /// for an allocated BAT block, or `None` if `bat_entry == BAT_ENTRY_UNUSED`.
    pub fn differencing_sector_location(
        &self,
        virtual_byte: u64,
    ) -> Result<Option<(u64, u64, u8, u64, u64)>> {
        let block_size = u64::from(self.block_size);
        let block_index = (virtual_byte / block_size) as usize;
        if block_index >= self.entries.len() {
            return Err(VhdError::BlockOutOfBounds);
        }
        let bat_entry = self.entries[block_index];
        if bat_entry == BAT_ENTRY_UNUSED {
            return Ok(None);
        }
        let bitmap_start = u64::from(bat_entry) * 512;
        let offset_in_block = virtual_byte % block_size;
        let sector_in_block = offset_in_block / 512;
        let sectors_per_block = block_size / 512;
        let bitmap_byte_file_offset = bitmap_start + (sector_in_block / 8);
        let bit_mask = 0x80u8 >> (sector_in_block % 8);
        let payload_file_offset =
            bitmap_start + bitmap_sectors(self.block_size) * 512 + offset_in_block;
        Ok(Some((
            payload_file_offset,
            bitmap_byte_file_offset,
            bit_mask,
            sector_in_block,
            sectors_per_block,
        )))
    }
}

/// Extract ordered candidate parent path strings from a 1024-byte VHD dynamic header
/// (MS-VHD §2.3 Parent Locator Entries + Parent Unicode Name).
pub fn parse_parent_locator_candidates<R: std::io::Read + std::io::Seek>(
    header_bytes: &[u8],
    backing: &mut R,
    footer_offset: u64,
) -> Vec<String> {
    let mut out = Vec::new();
    if header_bytes.len() < DYNAMIC_HEADER_SIZE {
        return out;
    }

    // First prefer W2ru (Windows relative UTF-16LE), then Wi2r / W2ku / Wi2k / Mac.
    let mut locators = Vec::new();
    for i in 0..8 {
        let base = 576 + i * 24;
        let code = [
            header_bytes[base],
            header_bytes[base + 1],
            header_bytes[base + 2],
            header_bytes[base + 3],
        ];
        let data_len = be_u32(header_bytes, base + 8) as usize;
        let data_off = be_u64(header_bytes, base + 16);
        if code == [0; 4] || data_len == 0 || data_len > 65536 {
            continue;
        }
        if data_off
            .checked_add(data_len as u64)
            .is_none_or(|end| end > footer_offset)
        {
            continue;
        }
        let prio: u8 = match &code {
            b"W2ru" => 0,
            b"Wi2r" => 1,
            b"Mac " => 2,
            b"W2ku" => 3,
            b"Wi2k" => 4,
            b"MacX" => 5,
            _ => 6,
        };
        locators.push((prio, code, data_len, data_off));
    }
    locators.sort_by_key(|(prio, ..)| *prio);

    for (_, code, data_len, data_off) in locators {
        if backing.seek(std::io::SeekFrom::Start(data_off)).is_err() {
            continue;
        }
        let mut raw = vec![0u8; data_len];
        if backing.read_exact(&mut raw).is_err() {
            continue;
        }
        let decoded = match &code {
            b"W2ru" | b"W2ku" | b"Wi2r" | b"Wi2k" => decode_utf16le(&raw),
            _ => decode_utf8_trimmed(&raw),
        };
        if !decoded.is_empty() && !out.contains(&decoded) {
            out.push(decoded);
        }
    }

    // Fallback: Parent Unicode Name (512 bytes UTF-16BE at offset 64).
    let pname = decode_utf16be(&header_bytes[64..576]);
    if !pname.is_empty() && !out.contains(&pname) {
        out.push(pname);
    }

    out
}

fn decode_utf16le(raw: &[u8]) -> String {
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&w| w != 0)
        .collect();
    String::from_utf16_lossy(&words).trim().to_string()
}

fn decode_utf16be(raw: &[u8]) -> String {
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .take_while(|&w| w != 0)
        .collect();
    String::from_utf16_lossy(&words).trim().to_string()
}

fn decode_utf8_trimmed(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
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
