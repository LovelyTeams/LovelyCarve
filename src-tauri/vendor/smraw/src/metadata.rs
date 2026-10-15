//! Caller-supplied acquisition metadata. RAW bytes contain none of these
//! values; this module does not infer, authenticate or persist them.
use crate::invalid;
use std::io;

/// Media/acquisition declarations supplied by the caller, not the RAW image.
/// `None` preserves unknown geometry/type/flags instead of asserting a default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaMetadata {
    pub bytes_per_sector: Option<u32>,
    pub media_type: Option<u8>,
    pub media_flags: Option<u8>,
    pub information: MetadataValues,
    /// Declared hashes only. No verification is implied by storing a value.
    pub integrity_hashes: MetadataValues,
}

/// Insertion-ordered UTF-8 values with bounded allocation and stable indices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataValues(Vec<(String, String)>);

impl MetadataValues {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(identifier, value)| (identifier.as_str(), value.as_str()))
    }
    pub fn get(&self, identifier: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == identifier)
            .map(|(_, value)| value.as_str())
    }
    pub fn get_index(&self, index: usize) -> Option<(&str, &str)> {
        self.0
            .get(index)
            .map(|(identifier, value)| (identifier.as_str(), value.as_str()))
    }
    pub fn set(&mut self, identifier: &str, value: &str) -> io::Result<()> {
        if identifier.is_empty()
            || identifier.len() > 128
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || value.len() > 65_536
            || value.contains('\0')
        {
            return Err(invalid("invalid RAW metadata identifier or value"));
        }
        let existing = self.0.iter().position(|(key, _)| key == identifier);
        let old_length = existing.map(|index| self.0[index].1.len()).unwrap_or(0);
        let length: usize = self
            .0
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        let added_identifier = if existing.is_some() {
            0
        } else {
            identifier.len()
        };
        if length - old_length + added_identifier + value.len() > 1024 * 1024
            || (existing.is_none() && self.0.len() >= 4096)
        {
            return Err(invalid("RAW metadata size limit exceeded"));
        }
        if let Some(index) = existing {
            self.0[index].1 = value.to_owned();
        } else {
            self.0.push((identifier.to_owned(), value.to_owned()));
        }
        Ok(())
    }
    pub fn set_utf16(&mut self, identifier: &str, value: &[u16]) -> io::Result<()> {
        if value.len() > 65_536 {
            return Err(invalid("RAW UTF-16 metadata size limit exceeded"));
        }
        let value =
            String::from_utf16(value).map_err(|_| invalid("invalid UTF-16 RAW metadata"))?;
        self.set(identifier, &value)
    }
    pub fn get_utf16(&self, identifier: &str) -> Option<Vec<u16>> {
        self.get(identifier)
            .map(|value| value.encode_utf16().collect())
    }
}
