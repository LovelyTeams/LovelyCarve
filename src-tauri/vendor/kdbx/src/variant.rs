//! The KDBX 4 `VariantDictionary`: the KDF parameters and public custom data.

use crate::bytes::Cursor;
use crate::error::{Error, Result};

/// A typed value of a [`VariantDictionary`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Variant {
    UInt32(u32),
    UInt64(u64),
    Bool(bool),
    Int32(i32),
    Int64(i64),
    String(String),
    Bytes(Vec<u8>),
    /// A value type KeePass does not define, kept as stored.
    Unknown {
        kind: u8,
        data: Vec<u8>,
    },
}

impl Variant {
    /// The type byte as stored.
    pub fn kind(&self) -> u8 {
        match self {
            Self::UInt32(_) => 0x04,
            Self::UInt64(_) => 0x05,
            Self::Bool(_) => 0x08,
            Self::Int32(_) => 0x0c,
            Self::Int64(_) => 0x0d,
            Self::String(_) => 0x18,
            Self::Bytes(_) => 0x42,
            Self::Unknown { kind, .. } => *kind,
        }
    }
}

/// An ordered key/value list as stored. Lookups return the last value of a
/// key, as KeePass reads a repeated key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VariantDictionary {
    pub version: u16,
    pub items: Vec<(String, Variant)>,
}

/// The highest dictionary format KeePass writes; a higher major byte is a
/// format this reader does not know.
const VERSION: u16 = 0x0100;
const CRITICAL_MASK: u16 = 0xff00;

impl VariantDictionary {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        let version = cursor.u16("variant dictionary version")?;
        if version & CRITICAL_MASK > VERSION & CRITICAL_MASK {
            return Err(Error::Unsupported(format!(
                "variant dictionary version {version:#06x}"
            )));
        }
        let mut items = Vec::new();
        loop {
            let kind = cursor.u8("variant dictionary item")?;
            if kind == 0 {
                break;
            }
            let key_length = cursor.length_i32("variant dictionary key")?;
            let key = String::from_utf8_lossy(cursor.take(key_length, "variant dictionary key")?)
                .into_owned();
            let value_length = cursor.length_i32("variant dictionary value")?;
            let value = cursor.take(value_length, "variant dictionary value")?;
            items.push((key.clone(), decode(kind, &key, value)?));
        }
        Ok(Self { version, items })
    }

    pub fn get(&self, key: &str) -> Option<&Variant> {
        self.items
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }
}

fn sized<const N: usize>(key: &str, value: &[u8]) -> Result<[u8; N]> {
    crate::bytes::fixed(value).ok_or_else(|| {
        Error::Malformed(format!(
            "variant dictionary value {key}: {} bytes where {N} are required",
            value.len()
        ))
    })
}

fn decode(kind: u8, key: &str, value: &[u8]) -> Result<Variant> {
    Ok(match kind {
        0x04 => Variant::UInt32(u32::from_le_bytes(sized(key, value)?)),
        0x05 => Variant::UInt64(u64::from_le_bytes(sized(key, value)?)),
        0x08 => Variant::Bool(sized::<1>(key, value)?[0] != 0),
        0x0c => Variant::Int32(i32::from_le_bytes(sized(key, value)?)),
        0x0d => Variant::Int64(i64::from_le_bytes(sized(key, value)?)),
        0x18 => Variant::String(String::from_utf8_lossy(value).into_owned()),
        0x42 => Variant::Bytes(value.to_vec()),
        kind => Variant::Unknown {
            kind,
            data: value.to_vec(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: u8, key: &str, value: &[u8]) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend((key.len() as i32).to_le_bytes());
        out.extend(key.as_bytes());
        out.extend((value.len() as i32).to_le_bytes());
        out.extend(value);
        out
    }

    #[test]
    fn reads_every_type_and_keeps_the_last_repeated_key() {
        let mut data = 0x0100u16.to_le_bytes().to_vec();
        data.extend(item(0x04, "P", &2u32.to_le_bytes()));
        data.extend(item(0x05, "I", &7u64.to_le_bytes()));
        data.extend(item(0x08, "B", &[1]));
        data.extend(item(0x0c, "i", &(-3i32).to_le_bytes()));
        data.extend(item(0x0d, "l", &(-4i64).to_le_bytes()));
        data.extend(item(0x18, "s", "密码".as_bytes()));
        data.extend(item(0x42, "S", &[9, 8]));
        data.extend(item(0x77, "x", &[1, 2, 3]));
        data.extend(item(0x04, "P", &4u32.to_le_bytes()));
        data.push(0);

        let dictionary = VariantDictionary::parse(&data).unwrap();
        assert_eq!(dictionary.items.len(), 9);
        assert_eq!(dictionary.get("P"), Some(&Variant::UInt32(4)));
        assert_eq!(dictionary.get("I"), Some(&Variant::UInt64(7)));
        assert_eq!(dictionary.get("B"), Some(&Variant::Bool(true)));
        assert_eq!(dictionary.get("i"), Some(&Variant::Int32(-3)));
        assert_eq!(dictionary.get("l"), Some(&Variant::Int64(-4)));
        assert_eq!(dictionary.get("s"), Some(&Variant::String("密码".into())));
        assert_eq!(dictionary.get("S"), Some(&Variant::Bytes(vec![9, 8])));
        assert_eq!(
            dictionary.get("x"),
            Some(&Variant::Unknown {
                kind: 0x77,
                data: vec![1, 2, 3]
            })
        );
    }

    #[test]
    fn rejects_newer_versions_bad_sizes_and_truncation() {
        let newer = [0x00, 0x02, 0x00];
        assert!(matches!(
            VariantDictionary::parse(&newer),
            Err(Error::Unsupported(_))
        ));

        let mut wrong_size = 0x0100u16.to_le_bytes().to_vec();
        wrong_size.extend(item(0x04, "P", &[1, 2]));
        wrong_size.push(0);
        assert!(matches!(
            VariantDictionary::parse(&wrong_size),
            Err(Error::Malformed(_))
        ));

        let mut truncated = 0x0100u16.to_le_bytes().to_vec();
        truncated.extend(item(0x42, "S", &[1, 2, 3]));
        truncated.truncate(truncated.len() - 1);
        assert!(matches!(
            VariantDictionary::parse(&truncated),
            Err(Error::Truncated(_))
        ));
    }
}
