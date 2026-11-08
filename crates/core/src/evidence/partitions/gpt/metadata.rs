//! Raw GUIDs are canonical UUIDs; names also retain their original UTF-16 units.

use std::io::Cursor;

use anyhow::{ensure, Context};
use binrw::BinRead;
use uuid::Uuid;

use super::{guid_to_kind, Partition, GPT_HEADER_BYTES, GPT_SIGNATURE};

#[derive(Debug, Clone, BinRead)]
#[br(little, magic = b"EFI PART")]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub header_crc32: u32,
    pub reserved: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    #[br(map = Uuid::from_bytes_le)]
    pub disk_guid: Uuid,
    pub entries_lba: u64,
    pub entry_count: u32,
    pub entry_size: u32,
    pub entries_crc32: u32,
    #[br(ignore)]
    pub read_lba: u64,
    #[br(ignore)]
    pub header_crc_valid: bool,
    #[br(ignore)]
    pub entries_crc_valid: bool,
    /// Header geometry, revision, reserved field and both checksums validated.
    #[br(ignore)]
    pub verified: bool,
}

impl GptHeader {
    /// Decode fixed fields even when checksums/geometry are damaged. Validation
    /// and extra header bytes are handled by `GptTable`, not this decoder.
    pub fn decode(bytes: &[u8], read_lba: u64) -> Option<Self> {
        if bytes.len() < GPT_HEADER_BYTES || !has_gpt_signature(bytes) {
            return None;
        }
        let mut header = Self::read(&mut Cursor::new(bytes)).ok()?;
        header.read_lba = read_lba;
        Some(header)
    }
}

/// Signature alone does not establish a valid partition table.
pub fn has_gpt_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(GPT_SIGNATURE)
}

/// Nonempty raw entry, including entries rejected from the filesystem projection.
#[derive(Debug, Clone, PartialEq, Eq, BinRead)]
#[br(little)]
pub struct GptEntry {
    #[br(map = Uuid::from_bytes_le)]
    pub type_guid: Uuid,
    #[br(map = Uuid::from_bytes_le)]
    pub unique_guid: Uuid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name_utf16: [u16; 36],
    #[br(ignore)]
    pub entry_index: u32,
    #[br(ignore)]
    pub entry_offset: u64,
    /// Full range validated against this table's source and usable area.
    #[br(ignore)]
    pub valid: bool,
}

impl GptEntry {
    pub fn decode(bytes: &[u8], entry_index: u32, entry_offset: u64) -> Option<Self> {
        let mut entry = Self::read(&mut Cursor::new(bytes)).ok()?;
        entry.entry_index = entry_index;
        entry.entry_offset = entry_offset;
        Some(entry)
    }

    /// Invalid UTF-16 is reported to the caller; original units stay available.
    pub fn name(&self) -> Result<String, std::string::FromUtf16Error> {
        let end = self
            .name_utf16
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(36);
        String::from_utf16(&self.name_utf16[..end])
    }

    pub(super) fn project(&self, id: u8) -> anyhow::Result<Partition> {
        ensure!(!self.type_guid.is_nil(), "unused GPT entry");
        let length_lba = self
            .last_lba
            .checked_sub(self.first_lba)
            .and_then(|length| length.checked_add(1))
            .context("GPT partition length overflow")?;
        let label = self.name().ok().filter(|name| !name.is_empty());
        Ok(Partition {
            id,
            kind: guid_to_kind(self.type_guid),
            start_lba: self.first_lba,
            length_lba,
            label,
            unique_guid: (!self.unique_guid.is_nil()).then(|| *self.unique_guid.as_bytes()),
            amiga_geometry: None,
        })
    }
}
