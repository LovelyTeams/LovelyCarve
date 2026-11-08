use crate::bytes::{le_u32, le_u64};
use crate::error::{Result, VhdxError};
use crate::header::crc32c;

pub const REGION_TABLE_SIGNATURE: &[u8; 4] = b"regi";
pub const REGION_ENTRY_SIZE: usize = 32;
/// Byte size of a region table block (CRC covers the full 64 KB).
pub const REGION_TABLE_CRC_COVERAGE: usize = 65536;
/// 1 MiB — the alignment granularity used throughout the VHDX format.
pub const MB: u64 = 0x0010_0000;

/// GUID bytes for the BAT region (MS-VHDX §2.3.4.1).
pub const BAT_GUID: [u8; 16] = [
    0x66, 0x77, 0xC2, 0x2D, 0x23, 0xF6, 0x00, 0x42, 0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08,
];

/// GUID bytes for the Metadata region (MS-VHDX §2.3.4.2).
pub const METADATA_GUID: [u8; 16] = [
    0x06, 0xA2, 0x7C, 0x8B, 0x90, 0x47, 0x9A, 0x4B, 0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E,
];

#[derive(Debug, Clone)]
pub struct RegionEntry {
    #[allow(dead_code)]
    pub guid: [u8; 16],
    pub file_offset: u64,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct RegionTable {
    pub bat: RegionEntry,
    pub metadata: RegionEntry,
}

/// Maximum number of region table entries we will process.
///
/// The spec-defined region table size is 64 KB; with 32-byte entries that
/// gives at most (65536 - 16) / 32 = 2047 entries. Cap at 2048 to prevent
/// a crafted `entry_count = u32::MAX` from iterating over a large file.
const REGION_ENTRY_COUNT_MAX: usize = 2048;

/// Parse a region table from the slice `data` at byte `offset`.
///
/// `container_len` is the size of the WHOLE container (not the slice), used to
/// range-check each region's `file_offset + length`. The bounded reader parses
/// the region table from a small 1 MB prefix while the regions themselves live
/// further into a multi-gigabyte image, so the bound must be the container size,
/// never `data.len()`.
pub fn parse_region_table(data: &[u8], offset: usize, container_len: u64) -> Result<RegionTable> {
    if data.len() < offset + 16 {
        return Err(VhdxError::InvalidRegionTable);
    }
    let slice = &data[offset..];
    if &slice[0..4] != REGION_TABLE_SIGNATURE {
        return Err(VhdxError::InvalidRegionTable);
    }
    let stored_crc = le_u32(slice, 4);
    let mut buf = slice[..65536.min(slice.len())].to_vec();
    buf.resize(65536, 0);
    buf[4..8].fill(0);
    if crc32c(&buf) != stored_crc {
        return Err(VhdxError::InvalidRegionTable);
    }
    let entry_count = (le_u32(slice, 8) as usize).min(REGION_ENTRY_COUNT_MAX);
    let mut bat: Option<RegionEntry> = None;
    let mut metadata: Option<RegionEntry> = None;
    for i in 0..entry_count {
        let base = 16 + i * REGION_ENTRY_SIZE;
        if base + REGION_ENTRY_SIZE > slice.len() {
            break;
        }
        let mut guid = [0u8; 16];
        guid.copy_from_slice(&slice[base..base + 16]);
        let file_offset = le_u64(slice, base + 16);
        let length = le_u32(slice, base + 24);
        let region_end = file_offset
            .checked_add(u64::from(length))
            .ok_or(VhdxError::OffsetOutOfBounds)?;
        if region_end > container_len {
            return Err(VhdxError::OffsetOutOfBounds);
        }
        let entry = RegionEntry {
            guid,
            file_offset,
            length,
        };
        if guid == BAT_GUID {
            bat = Some(entry);
        } else if guid == METADATA_GUID {
            metadata = Some(entry);
        }
    }
    Ok(RegionTable {
        bat: bat.ok_or(VhdxError::BatRegionMissing)?,
        metadata: metadata.ok_or(VhdxError::MetadataRegionMissing)?,
    })
}
