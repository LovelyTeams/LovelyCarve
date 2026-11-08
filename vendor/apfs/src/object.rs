use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::error::{ApfsError, Result};
use crate::fletcher;

// Object type constants (lower 16 bits of type_and_flags)
pub const OBJECT_TYPE_NX_SUPERBLOCK: u32 = 0x01;
pub const OBJECT_TYPE_BTREE: u32 = 0x02;
pub const OBJECT_TYPE_BTREE_NODE: u32 = 0x03;
pub const OBJECT_TYPE_SPACEMAN: u32 = 0x05;
pub const OBJECT_TYPE_OMAP: u32 = 0x0B;
pub const OBJECT_TYPE_CHECKPOINT_MAP: u32 = 0x0C;
pub const OBJECT_TYPE_FS: u32 = 0x0D;

// Object flag masks (upper 16 bits of type_and_flags)
pub const OBJ_PHYSICAL: u32 = 0x40000000;
pub const OBJ_VIRTUAL: u32 = 0x00000000;
pub const OBJ_EPHEMERAL: u32 = 0x80000000;
pub const OBJ_STORAGE_TYPE_MASK: u32 = 0xC0000000;
pub const OBJECT_TYPE_MASK: u32 = 0x0000FFFF;

/// 32-byte header present on every APFS on-disk object. All fields are little-endian.
#[derive(Debug, Clone)]
pub struct ObjectHeader {
    pub checksum: u64,       // 0x00
    pub oid: u64,            // 0x08
    pub xid: u64,            // 0x10
    pub type_and_flags: u32, // 0x18
    pub subtype: u32,        // 0x1C
}

impl ObjectHeader {
    /// Size of the on-disk header in bytes
    pub const SIZE: usize = 32;

    /// Parse an object header from the first 32 bytes of a block
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(ApfsError::CorruptedData(format!(
                "object header too short: {} bytes",
                data.len()
            )));
        }

        let mut cursor = Cursor::new(data);
        Ok(ObjectHeader {
            checksum: cursor.read_u64::<LittleEndian>()?,
            oid: cursor.read_u64::<LittleEndian>()?,
            xid: cursor.read_u64::<LittleEndian>()?,
            type_and_flags: cursor.read_u32::<LittleEndian>()?,
            subtype: cursor.read_u32::<LittleEndian>()?,
        })
    }

    /// Get the object type (lower 16 bits, no flags)
    pub fn object_type(&self) -> u32 {
        self.type_and_flags & OBJECT_TYPE_MASK
    }

    /// Get the storage type flags (upper 2 bits)
    pub fn storage_type(&self) -> u32 {
        self.type_and_flags & OBJ_STORAGE_TYPE_MASK
    }

    /// Whether this is a physical object (address = block number)
    pub fn is_physical(&self) -> bool {
        self.storage_type() == OBJ_PHYSICAL
    }
}

/// Read a full block at the given block number, verify its checksum, and parse the header.
pub fn read_object<R: Read + Seek>(
    reader: &mut R,
    block_number: u64,
    block_size: u32,
) -> Result<(ObjectHeader, Vec<u8>)> {
    let offset = block_number
        .checked_mul(u64::from(block_size))
        .ok_or_else(|| ApfsError::CorruptedData("object offset overflow".into()))?;
    if !(4096..=65536).contains(&block_size) || !block_size.is_power_of_two() {
        return Err(ApfsError::CorruptedData("invalid object block size".into()));
    }
    reader.seek(SeekFrom::Start(offset))?;

    let mut block = vec![0u8; block_size as usize];
    reader.read_exact(&mut block)?;

    if !fletcher::verify_object(&block) {
        return Err(ApfsError::InvalidChecksum);
    }

    let header = ObjectHeader::parse(&block)?;
    Ok((header, block))
}

/// Read a block at the given block number without checksum verification.
pub fn read_block<R: Read + Seek>(
    reader: &mut R,
    block_number: u64,
    block_size: u32,
) -> Result<Vec<u8>> {
    let offset = block_number
        .checked_mul(u64::from(block_size))
        .ok_or_else(|| ApfsError::CorruptedData("block offset overflow".into()))?;
    if !(4096..=65536).contains(&block_size) || !block_size.is_power_of_two() {
        return Err(ApfsError::CorruptedData("invalid block size".into()));
    }
    reader.seek(SeekFrom::Start(offset))?;

    let mut block = vec![0u8; block_size as usize];
    reader.read_exact(&mut block)?;
    Ok(block)
}
