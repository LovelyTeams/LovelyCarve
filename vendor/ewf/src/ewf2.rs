//! EWF2 (Ex01/Lx01) format types and parsing.
//!
//! EWF2 is the Expert Witness Compression Format version 2, introduced in `EnCase` 7.

use crate::error::{EwfError, Result};
use safe_read::{le_u32, le_u64};

// ---------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------

pub const EVF2_SIGNATURE: [u8; 8] = [0x45, 0x56, 0x46, 0x32, 0x0d, 0x0a, 0x81, 0x00];
pub const LEF2_SIGNATURE: [u8; 8] = [0x4c, 0x45, 0x46, 0x32, 0x0d, 0x0a, 0x81, 0x00];

pub const FILE_HEADER_SIZE: usize = 32;
pub const SECTION_DESCRIPTOR_SIZE: usize = 64;
pub const TABLE_ENTRY_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Compression method
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMethod {
    None,
    Zlib,
    Bzip2,
}

impl CompressionMethod {
    pub fn from_u16(val: u16) -> Result<Self> {
        match val {
            0 => Ok(Self::None),
            1 => Ok(Self::Zlib),
            2 => Ok(Self::Bzip2),
            _ => Err(EwfError::Parse(format!(
                "unknown EWF2 compression method: {val}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Section types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ewf2SectionType {
    DeviceInfo,
    CaseData,
    SectorData,
    SectorTable,
    ErrorTable,
    SessionTable,
    IncrementData,
    Md5Hash,
    Sha1Hash,
    RestartData,
    EncryptionKeys,
    MemoryExtents,
    Next,
    FinalInfo,
    Done,
    AnalyticalData,
    SingleFilesData,
    Unknown(u32),
}

impl Ewf2SectionType {
    pub fn from_u32(val: u32) -> Self {
        match val {
            0x01 => Self::DeviceInfo,
            0x02 => Self::CaseData,
            0x03 => Self::SectorData,
            0x04 => Self::SectorTable,
            0x05 => Self::ErrorTable,
            0x06 => Self::SessionTable,
            0x07 => Self::IncrementData,
            0x08 => Self::Md5Hash,
            0x09 => Self::Sha1Hash,
            0x0A => Self::RestartData,
            0x0B => Self::EncryptionKeys,
            0x0C => Self::MemoryExtents,
            0x0D => Self::Next,
            0x0E => Self::FinalInfo,
            0x0F => Self::Done,
            0x10 => Self::AnalyticalData,
            0x20 => Self::SingleFilesData,
            other => Self::Unknown(other),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        match self {
            Self::DeviceInfo => "device_info",
            Self::CaseData => "case_data",
            Self::SectorData => "sector_data",
            Self::SectorTable => "sector_table",
            Self::ErrorTable => "error_table",
            Self::SessionTable => "session_table",
            Self::IncrementData => "increment_data",
            Self::Md5Hash => "md5_hash",
            Self::Sha1Hash => "sha1_hash",
            Self::RestartData => "restart_data",
            Self::EncryptionKeys => "encryption_keys",
            Self::MemoryExtents => "memory_extents",
            Self::Next => "next",
            Self::FinalInfo => "final_info",
            Self::Done => "done",
            Self::AnalyticalData => "analytical_data",
            Self::SingleFilesData => "single_files_data",
            Self::Unknown(_) => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// File Header (32 bytes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ewf2FileHeader {
    pub is_physical: bool,
    pub major_version: u8,
    pub minor_version: u8,
    pub compression_method: CompressionMethod,
    pub segment_number: u32,
    pub set_identifier: [u8; 16],
}

impl Ewf2FileHeader {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < FILE_HEADER_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: FILE_HEADER_SIZE,
                got: buf.len(),
            });
        }

        let is_physical = if buf[0..8] == EVF2_SIGNATURE {
            true
        } else if buf[0..8] == LEF2_SIGNATURE {
            false
        } else {
            let mut found = [0u8; 8];
            found.copy_from_slice(&buf[0..8]);
            return Err(EwfError::InvalidSignature { found });
        };

        let major_version = buf[8];
        let minor_version = buf[9];
        let compression_method =
            CompressionMethod::from_u16(u16::from_le_bytes([buf[10], buf[11]]))?;
        let segment_number = le_u32(buf, 12);
        let mut set_identifier = [0u8; 16];
        set_identifier.copy_from_slice(&buf[16..32]);

        Ok(Self {
            is_physical,
            major_version,
            minor_version,
            compression_method,
            segment_number,
            set_identifier,
        })
    }
}

// ---------------------------------------------------------------------------
// Section Descriptor (64 bytes)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub const DATA_FLAG_MD5HASHED: u32 = 0x0000_0001;
pub const DATA_FLAG_ENCRYPTED: u32 = 0x0000_0002;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ewf2SectionDescriptor {
    pub section_type: Ewf2SectionType,
    pub data_flags: u32,
    pub previous_offset: u64,
    pub data_size: u64,
    pub descriptor_size: u32,
    pub padding_size: u32,
    pub data_integrity_hash: [u8; 16],
    pub offset: u64,
}

impl Ewf2SectionDescriptor {
    pub fn parse(buf: &[u8], offset: u64) -> Result<Self> {
        if buf.len() < SECTION_DESCRIPTOR_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: SECTION_DESCRIPTOR_SIZE,
                got: buf.len(),
            });
        }

        let section_type = Ewf2SectionType::from_u32(le_u32(buf, 0));
        let data_flags = le_u32(buf, 4);
        let previous_offset = le_u64(buf, 8);
        let data_size = le_u64(buf, 16);
        let descriptor_size = le_u32(buf, 24);
        let padding_size = le_u32(buf, 28);
        let mut data_integrity_hash = [0u8; 16];
        data_integrity_hash.copy_from_slice(&buf[32..48]);

        Ok(Self {
            section_type,
            data_flags,
            previous_offset,
            data_size,
            descriptor_size,
            padding_size,
            data_integrity_hash,
            offset,
        })
    }

    #[allow(dead_code)]
    pub fn is_md5_hashed(&self) -> bool {
        self.data_flags & DATA_FLAG_MD5HASHED != 0
    }

    pub fn is_encrypted(&self) -> bool {
        self.data_flags & DATA_FLAG_ENCRYPTED != 0
    }
}

// ---------------------------------------------------------------------------
// Table Entry (16 bytes)
// ---------------------------------------------------------------------------

pub const CHUNK_FLAG_COMPRESSED: u32 = 0x0000_0001;
#[allow(dead_code)]
pub const CHUNK_FLAG_CHECKSUMED: u32 = 0x0000_0002;
#[allow(dead_code)]
pub const CHUNK_FLAG_PATTERNFILL: u32 = 0x0000_0004;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ewf2TableEntry {
    pub chunk_data_offset: u64,
    pub chunk_data_size: u32,
    pub flags: u32,
}

impl Ewf2TableEntry {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < TABLE_ENTRY_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: TABLE_ENTRY_SIZE,
                got: buf.len(),
            });
        }

        Ok(Self {
            chunk_data_offset: le_u64(buf, 0),
            chunk_data_size: le_u32(buf, 8),
            flags: le_u32(buf, 12),
        })
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & CHUNK_FLAG_COMPRESSED != 0
    }

    #[allow(dead_code)]
    pub fn is_checksumed(&self) -> bool {
        self.flags & CHUNK_FLAG_CHECKSUMED != 0
    }

    #[allow(dead_code)]
    pub fn is_pattern_fill(&self) -> bool {
        self.flags & (CHUNK_FLAG_COMPRESSED | CHUNK_FLAG_PATTERNFILL)
            == (CHUNK_FLAG_COMPRESSED | CHUNK_FLAG_PATTERNFILL)
    }
}

// ---------------------------------------------------------------------------
// Table Header (20 bytes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ewf2TableHeader {
    pub first_chunk: u64,
    pub entry_count: u32,
}

impl Ewf2TableHeader {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        // EWF2 table header is 32 bytes: first_chunk(8) + entry_count(4) + 20 reserved.
        if buf.len() < 32 {
            return Err(EwfError::BufferTooShort {
                expected: 32,
                got: buf.len(),
            });
        }

        Ok(Self {
            first_chunk: le_u64(buf, 0),
            entry_count: le_u32(buf, 8),
        })
    }
}
