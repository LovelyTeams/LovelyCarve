use crate::bytes::{le_u16, le_u32};
use crate::error::{Result, VhdxError};

// B04AEFB7-D19E-4A81-B789-25B8E9445913 (MS-VHDX §2.6.2.6.3)
const LOCATOR_TYPE: [u8; 16] = [
    0xB7, 0xEF, 0x4A, 0xB0, 0x9E, 0xD1, 0x81, 0x4A, 0xB7, 0x89, 0x25, 0xB8, 0xE9, 0x44, 0x59, 0x13,
];
const INVALID_LOCATOR: VhdxError = VhdxError::InvalidMetadata("invalid VHDX parent locator");

#[derive(Debug, Clone)]
pub(crate) struct ParentLocator {
    pub(crate) data_write_guid: [u8; 16],
    pub(crate) relative_path: Option<String>,
}

impl ParentLocator {
    pub(crate) fn parse(item: &[u8]) -> Result<Self> {
        const HEADER_LEN: usize = 20;
        const ENTRY_LEN: usize = 12;
        const MAX_ENTRIES: usize = 64;
        if item.len() < HEADER_LEN || item[..16] != LOCATOR_TYPE || le_u16(item, 16) != 0 {
            return Err(INVALID_LOCATOR);
        }
        let entry_count = usize::from(le_u16(item, 18));
        if entry_count > MAX_ENTRIES {
            return Err(INVALID_LOCATOR);
        }
        let entries_end = HEADER_LEN + entry_count * ENTRY_LEN;
        let entries = item.get(HEADER_LEN..entries_end).ok_or(INVALID_LOCATOR)?;
        let mut data_write_guid = None;
        let mut relative_path = None;
        for entry in entries.chunks_exact(ENTRY_LEN) {
            let key = locator_string(item, entries_end, le_u32(entry, 0), le_u16(entry, 8))?;
            let value = locator_string(item, entries_end, le_u32(entry, 4), le_u16(entry, 10))?;
            match key.as_str() {
                "parent_linkage" if data_write_guid.is_none() => {
                    data_write_guid = Some(
                        parse_guid(&value)
                            .filter(|guid| *guid != [0; 16])
                            .ok_or(INVALID_LOCATOR)?,
                    );
                }
                "relative_path" if relative_path.is_none() => relative_path = Some(value),
                "parent_linkage" | "relative_path" => return Err(INVALID_LOCATOR),
                _ => {}
            }
        }
        Ok(Self {
            data_write_guid: data_write_guid.ok_or(INVALID_LOCATOR)?,
            relative_path,
        })
    }
}

fn locator_string(item: &[u8], entries_end: usize, offset: u32, length: u16) -> Result<String> {
    let start = usize::try_from(offset).map_err(|_| INVALID_LOCATOR)?;
    let length = usize::from(length);
    if start < entries_end || length == 0 || length % 2 != 0 {
        return Err(INVALID_LOCATOR);
    }
    let bytes = item
        .get(start..)
        .and_then(|data| data.get(..length))
        .ok_or(INVALID_LOCATOR)?;
    let words: Vec<_> = bytes.chunks_exact(2).map(|pair| le_u16(pair, 0)).collect();
    String::from_utf16(&words)
        .ok()
        .filter(|value| !value.contains('\0'))
        .ok_or(INVALID_LOCATOR)
}

fn parse_guid(value: &str) -> Option<[u8; 16]> {
    let value = value.strip_prefix('{')?.strip_suffix('}')?;
    if !value.is_ascii() || value.len() != 36 {
        return None;
    }
    let mut guid = [0; 16];
    let mut chars = value.chars();
    for (index, byte) in guid.iter_mut().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) && chars.next()? != '-' {
            return None;
        }
        *byte = (chars.next()?.to_digit(16)? * 16 + chars.next()?.to_digit(16)?) as u8;
    }
    guid[..4].reverse();
    guid[4..6].reverse();
    guid[6..8].reverse();
    Some(guid)
}
