use crate::bytes::{le_arr16, le_u32, le_u64};
use crate::error::{Result, VhdxError};

/// Offsets per MS-VHDX spec §2.1 — each block occupies a 64 KB slot within
/// the first 1 MB of the file (the "header section").
pub const HEADER1_OFFSET: u64 = 0x0001_0000; // 64 KB
pub const HEADER2_OFFSET: u64 = 0x0002_0000; // 128 KB
pub const REGION_TABLE1_OFFSET: u64 = 0x0003_0000; // 192 KB
pub const REGION_TABLE2_OFFSET: u64 = 0x0004_0000; // 256 KB

pub const HEADER_SIGNATURE: &[u8; 4] = b"head";
pub const HEADER_SIZE: usize = 4096;

/// Parsed VHDX header (the active copy with highest valid sequence number).
#[derive(Debug, Clone)]
pub struct VhdxHeader {
    pub sequence_number: u64,
    pub log_guid: [u8; 16],
    pub log_offset: u64,
    pub log_length: u32,
}

/// Select the active header from a raw byte slice covering both header copies.
/// Returns the header with the highest sequence number whose CRC32C is valid.
pub fn parse_active_header(data: &[u8]) -> Result<VhdxHeader> {
    let h1 = parse_one_header(data, HEADER1_OFFSET as usize);
    let h2 = parse_one_header(data, HEADER2_OFFSET as usize);
    match (h1, h2) {
        (Ok(a), Ok(b)) => {
            if a.sequence_number >= b.sequence_number {
                Ok(a)
            } else {
                Ok(b)
            }
        }
        (Ok(a), Err(_)) => Ok(a),
        (Err(_), Ok(b)) => Ok(b),
        (Err(_), Err(_)) => Err(VhdxError::NoValidHeader),
    }
}

fn parse_one_header(data: &[u8], offset: usize) -> Result<VhdxHeader> {
    let end = offset
        .checked_add(HEADER_SIZE)
        .ok_or(VhdxError::NoValidHeader)?;
    if data.len() < end {
        return Err(VhdxError::NoValidHeader);
    }
    let slice = &data[offset..end];
    if &slice[0..4] != HEADER_SIGNATURE {
        return Err(VhdxError::NoValidHeader);
    }
    if !verify_crc32c(slice) {
        return Err(VhdxError::NoValidHeader);
    }
    let sequence_number = le_u64(slice, 8);
    let log_guid: [u8; 16] = le_arr16(slice, 48);
    let log_length = le_u32(slice, 68);
    let log_offset = le_u64(slice, 72);
    Ok(VhdxHeader {
        sequence_number,
        log_guid,
        log_offset,
        log_length,
    })
}

fn verify_crc32c(block: &[u8]) -> bool {
    let stored = le_u32(block, 4);
    let mut buf = block.to_vec();
    buf[4..8].fill(0);
    crc32c(&buf) == stored
}

pub fn crc32c(data: &[u8]) -> u32 {
    const POLY: u32 = 0x82F6_3B78;
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}
