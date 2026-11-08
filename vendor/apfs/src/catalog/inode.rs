//! Strict inode xfield parsing, including dstream length and device identity.
use crate::error::{ApfsError, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

// Extended field types (INO_EXT_TYPE_*)
const INO_EXT_TYPE_DSTREAM: u8 = 8;

/// Parsed inode value from a catalog record.
#[derive(Debug, Clone)]
pub struct InodeVal {
    pub parent_id: u64,
    pub private_id: u64,
    pub create_time: i64,
    pub modify_time: i64,
    pub change_time: i64,
    pub access_time: i64,
    pub internal_flags: u64,
    pub nchildren_or_nlink: i32,
    pub default_protection_class: u32,
    pub write_generation_counter: u32,
    pub bsd_flags: u32,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub pad1: u16,
    pub uncompressed_size: u64,
    /// Logical file size from the dstream xfield (if present).
    pub dstream_size: Option<u64>,
    pub device_number: Option<u32>,
}

impl InodeVal {
    /// Fixed size of j_inode_val_t before xfields
    const FIXED_SIZE: usize = 92;

    /// Parse from raw catalog value bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::FIXED_SIZE {
            return Err(ApfsError::CorruptedData(format!(
                "inode value too short: {} bytes",
                data.len()
            )));
        }
        let mut cursor = Cursor::new(data);
        let parent_id = cursor.read_u64::<LittleEndian>()?;
        let private_id = cursor.read_u64::<LittleEndian>()?;
        let create_time = cursor.read_i64::<LittleEndian>()?;
        let modify_time = cursor.read_i64::<LittleEndian>()?;
        let change_time = cursor.read_i64::<LittleEndian>()?;
        let access_time = cursor.read_i64::<LittleEndian>()?;
        let internal_flags = cursor.read_u64::<LittleEndian>()?;
        let nchildren_or_nlink = cursor.read_i32::<LittleEndian>()?;
        if nchildren_or_nlink < 0 {
            return Err(ApfsError::CorruptedData(
                "negative inode children/link count".into(),
            ));
        }
        let default_protection_class = cursor.read_u32::<LittleEndian>()?;
        let write_generation_counter = cursor.read_u32::<LittleEndian>()?;
        let bsd_flags = cursor.read_u32::<LittleEndian>()?;
        let uid = cursor.read_u32::<LittleEndian>()?;
        let gid = cursor.read_u32::<LittleEndian>()?;
        let mode = cursor.read_u16::<LittleEndian>()?;
        let pad1 = cursor.read_u16::<LittleEndian>()?;
        let uncompressed_size = cursor.read_u64::<LittleEndian>()?;

        // Parse xfields for dstream size
        let (dstream_size, device_number) = Self::parse_xfields(&data[Self::FIXED_SIZE..])?;

        Ok(InodeVal {
            parent_id,
            private_id,
            create_time,
            modify_time,
            change_time,
            access_time,
            internal_flags,
            nchildren_or_nlink,
            default_protection_class,
            write_generation_counter,
            bsd_flags,
            uid,
            gid,
            mode,
            pad1,
            uncompressed_size,
            dstream_size,
            device_number,
        })
    }

    /// Parse xfields to extract dstream size.
    /// Layout: xf_blob_t { xf_num_exts: u16, xf_used_data: u16 }
    /// followed by x_field_t[xf_num_exts] { x_type: u8, x_flags: u8, x_size: u16 }
    /// followed by the actual field data values (each padded to 8-byte alignment).
    fn parse_xfields(data: &[u8]) -> Result<(Option<u64>, Option<u32>)> {
        if data.is_empty() {
            return Ok((None, None));
        }
        let mut cursor = Cursor::new(data);
        let count = usize::from(cursor.read_u16::<LittleEndian>()?);
        let used = usize::from(cursor.read_u16::<LittleEndian>()?);
        let entries_end = 4 + count * 4;
        let end = entries_end
            .checked_add(used)
            .ok_or_else(|| ApfsError::CorruptedData("xfield length overflow".into()))?;
        if end > data.len() {
            return Err(ApfsError::CorruptedData("truncated inode xfields".into()));
        }
        let mut offset = entries_end;
        let mut size = None;
        let mut device = None;
        for index in 0..count {
            let entry = &data[4 + index * 4..8 + index * 4];
            let length = usize::from(u16::from_le_bytes([entry[2], entry[3]]));
            let value = data
                .get(offset..offset + length)
                .filter(|_| offset + length <= end)
                .ok_or_else(|| {
                    ApfsError::CorruptedData("inode xfield outside declared data".into())
                })?;
            match entry[0] {
                INO_EXT_TYPE_DSTREAM => {
                    if value.len() != 40 || size.is_some() {
                        return Err(ApfsError::CorruptedData(
                            "invalid or duplicate dstream xfield".into(),
                        ));
                    }
                    let mut cursor = Cursor::new(value);
                    size = Some(cursor.read_u64::<LittleEndian>()?);
                }
                14 => {
                    if value.len() != 4 || device.is_some() {
                        return Err(ApfsError::CorruptedData(
                            "invalid or duplicate device xfield".into(),
                        ));
                    }
                    let mut cursor = Cursor::new(value);
                    device = Some(cursor.read_u32::<LittleEndian>()?);
                }
                _ => {}
            }
            offset += (length + 7) & !7;
            if offset > end {
                return Err(ApfsError::CorruptedData(
                    "xfield padding exceeds declared data".into(),
                ));
            }
        }
        Ok((size, device))
    }

    /// Get the file type from the mode field
    pub fn kind(&self) -> u16 {
        self.mode & 0o170000
    }

    /// Get the logical file size.
    /// Prefers dstream size from xfields; falls back to uncompressed_size.
    pub fn size(&self) -> u64 {
        self.dstream_size.unwrap_or(self.uncompressed_size)
    }

    pub fn nlink(&self) -> u32 {
        self.nchildren_or_nlink as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dstream_device_and_declared_xfield_boundaries_are_preserved() {
        let mut bytes = vec![0u8; 92];
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&48u16.to_le_bytes());
        bytes.extend_from_slice(&[8, 0, 40, 0, 14, 0, 4, 0]);
        bytes.extend_from_slice(&12345u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 32]);
        bytes.extend_from_slice(&0x02000007u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        let inode = InodeVal::parse(&bytes).unwrap();
        assert_eq!(inode.dstream_size, Some(12345));
        assert_eq!(inode.device_number, Some(0x02000007));
        bytes[94..96].copy_from_slice(&8u16.to_le_bytes());
        assert!(InodeVal::parse(&bytes).is_err());
        assert!(InodeVal::parse(&bytes[..100]).is_err());
    }
}
