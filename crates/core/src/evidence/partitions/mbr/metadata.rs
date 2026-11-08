//! Lossless MBR/EBR record metadata; CHS is descriptive, LBA drives reads.

use binrw::BinRead;
use std::io::Cursor;

use super::{MBR_ENTRY_COUNT, MBR_PART_ENTRY_SIZE, MBR_PART_TABLE_OFFSET};

/// Packed legacy geometry. Zero and saturated values are preserved, not
/// converted to LBA: that conversion requires geometry absent from the MBR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChsAddress {
    pub cylinder: u16,
    pub head: u8,
    pub sector: u8,
}

impl From<[u8; 3]> for ChsAddress {
    fn from(bytes: [u8; 3]) -> Self {
        Self {
            cylinder: u16::from(bytes[2]) | (u16::from(bytes[1] & 0xc0) << 2),
            head: bytes[0],
            sector: bytes[1] & 0x3f,
        }
    }
}

/// One on-disk slot, including empty and container entries. The type byte
/// is distinct from a filesystem kind; e.g. 0x07 can contain NTFS or exFAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BinRead)]
#[br(little)]
pub struct MbrEntry {
    pub boot_indicator: u8,
    pub first_chs: [u8; 3],
    pub type_byte: u8,
    pub last_chs: [u8; 3],
    pub start_lba: u32,
    pub length_lba: u32,
}

impl MbrEntry {
    pub fn is_extended(&self) -> bool {
        matches!(self.type_byte, 0x05 | 0x0f | 0x85)
    }

    pub fn is_bootable(&self) -> bool {
        self.boot_indicator == 0x80
    }

    pub fn start_chs(&self) -> ChsAddress {
        self.first_chs.into()
    }

    pub fn end_chs(&self) -> ChsAddress {
        self.last_chs.into()
    }

    pub(super) fn is_populated(&self) -> bool {
        self.type_byte != 0 && self.length_lba != 0
    }
}

/// The standard 512-byte portion of a boot record, even on 4 KiB sectors.
/// Vendor boot-code extensions (AAP/NEWLDR/16-slot tables) are not interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbrBootRecord {
    pub record_lba: u64,
    pub disk_signature: u32,
    pub has_signature: bool,
    pub entries: [MbrEntry; MBR_ENTRY_COUNT],
}

impl MbrBootRecord {
    /// Decode without claiming a valid table; detection separately validates
    /// signatures, boot indicators and ranges. Short records return `None`.
    pub fn decode(bytes: &[u8], record_lba: u64) -> Option<Self> {
        let bytes = bytes.get(..512)?;
        let mut cursor = Cursor::new(&bytes[MBR_PART_TABLE_OFFSET..510]);
        let entries = <[MbrEntry; MBR_ENTRY_COUNT]>::read(&mut cursor).ok()?;
        Some(Self {
            record_lba,
            disk_signature: u32::from_le_bytes(bytes[440..444].try_into().ok()?),
            has_signature: has_boot_signature(bytes),
            entries,
        })
    }

    pub(super) fn valid_indicators(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| matches!(entry.boot_indicator, 0 | 0x80))
    }

    /// Byte offset of an entry in the enclosing source (not in its partition).
    pub fn entry_offset(&self, slot: usize, sector_size: usize) -> Option<u64> {
        if slot >= MBR_ENTRY_COUNT {
            return None;
        }
        self.record_lba
            .checked_mul(sector_size as u64)?
            .checked_add((MBR_PART_TABLE_OFFSET + slot * MBR_PART_ENTRY_SIZE) as u64)
    }
}

/// Signature-only check, equivalent to libvsmbr's check-volume-signature API.
/// A volume boot sector can also match, so this is not table detection.
pub fn has_boot_signature(bytes: &[u8]) -> bool {
    bytes.get(510..512) == Some(super::MBR_MAGIC.as_slice())
}
