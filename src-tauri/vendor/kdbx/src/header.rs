//! The outer header: signature, version and the fields before the payload.
//! Everything here is readable without a key.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::bytes::{fixed, hex, Cursor};
use crate::error::{Error, Result};
use crate::kdf::Kdf;
use crate::variant::VariantDictionary;

pub const SIGNATURE_1: u32 = 0x9aa2_d903;
/// KeePass 2.x and KDBX 3 / 4.
pub const SIGNATURE_2: u32 = 0xb54b_fb67;
/// KeePass 2.x pre-release builds.
pub const SIGNATURE_2_PRE_RELEASE: u32 = 0xb54b_fb66;
/// KeePass 1.x (KDB).
pub const SIGNATURE_2_KEEPASS1: u32 = 0xb54b_fb65;

/// The oldest and newest major versions KeePass reads with these layouts:
/// below 4 the KDBX 3 layout, 4 the KDBX 4 layout.
const MIN_MAJOR: u16 = 2;
const MAX_MAJOR: u16 = 4;

/// The file version, `major.minor` (3.1, 4.0, 4.1 …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    /// Whether the file uses the KDBX 4 layout.
    pub fn is_kdbx4(self) -> bool {
        self.major >= 4
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

/// The payload cipher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    Aes256,
    Twofish,
    ChaCha20,
    /// A cipher UUID this reader does not implement (a KeePass plugin cipher).
    Unknown([u8; 16]),
}

impl Cipher {
    pub const AES256_UUID: [u8; 16] = [
        0x31, 0xc1, 0xf2, 0xe6, 0xbf, 0x71, 0x43, 0x50, 0xbe, 0x58, 0x05, 0x21, 0x6a, 0xfc, 0x5a,
        0xff,
    ];
    pub const TWOFISH_UUID: [u8; 16] = [
        0xad, 0x68, 0xf2, 0x9f, 0x57, 0x6f, 0x4b, 0xb9, 0xa3, 0x6a, 0xd4, 0x7a, 0xf9, 0x65, 0x34,
        0x6c,
    ];
    pub const CHACHA20_UUID: [u8; 16] = [
        0xd6, 0x03, 0x8a, 0x2b, 0x8b, 0x6f, 0x4c, 0xb5, 0xa5, 0x24, 0x33, 0x9a, 0x31, 0xdb, 0xb5,
        0x9a,
    ];

    pub fn from_uuid(uuid: [u8; 16]) -> Self {
        match uuid {
            Self::AES256_UUID => Self::Aes256,
            Self::TWOFISH_UUID => Self::Twofish,
            Self::CHACHA20_UUID => Self::ChaCha20,
            uuid => Self::Unknown(uuid),
        }
    }

    pub fn uuid(self) -> [u8; 16] {
        match self {
            Self::Aes256 => Self::AES256_UUID,
            Self::Twofish => Self::TWOFISH_UUID,
            Self::ChaCha20 => Self::CHACHA20_UUID,
            Self::Unknown(uuid) => uuid,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Aes256 => "AES-256",
            Self::Twofish => "Twofish",
            Self::ChaCha20 => "ChaCha20",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Payload compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Unknown(u32),
}

impl Compression {
    fn from_flags(flags: u32) -> Self {
        match flags {
            0 => Self::None,
            1 => Self::Gzip,
            other => Self::Unknown(other),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "GZip",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// The stream cipher that protects `Protected="True"` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InnerStreamAlgorithm {
    None,
    /// KeePass 2.x betas; not implemented by KeePass's successors either.
    ArcFourVariant,
    Salsa20,
    ChaCha20,
    Unknown(u32),
}

impl InnerStreamAlgorithm {
    pub fn from_id(id: u32) -> Self {
        match id {
            0 => Self::None,
            1 => Self::ArcFourVariant,
            2 => Self::Salsa20,
            3 => Self::ChaCha20,
            other => Self::Unknown(other),
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Self::None => 0,
            Self::ArcFourVariant => 1,
            Self::Salsa20 => 2,
            Self::ChaCha20 => 3,
            Self::Unknown(id) => id,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ArcFourVariant => "ArcFour variant",
            Self::Salsa20 => "Salsa20",
            Self::ChaCha20 => "ChaCha20",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// One header field as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderField {
    pub id: u8,
    /// Offset of the field's id byte in the file.
    pub offset: usize,
    pub data: Vec<u8>,
}

impl HeaderField {
    /// KeePass's name for the field id in a file of `version`.
    pub fn name(&self, version: Version) -> Option<&'static str> {
        Some(match (self.id, version.is_kdbx4()) {
            (0, _) => "EndOfHeader",
            (1, _) => "Comment",
            (2, _) => "CipherID",
            (3, _) => "CompressionFlags",
            (4, _) => "MasterSeed",
            (5, false) => "TransformSeed",
            (6, false) => "TransformRounds",
            (7, _) => "EncryptionIV",
            (8, false) => "InnerRandomStreamKey",
            (9, false) => "StreamStartBytes",
            (10, false) => "InnerRandomStreamID",
            (11, true) => "KdfParameters",
            (12, true) => "PublicCustomData",
            _ => return None,
        })
    }
}

/// The outer header of a KDBX file.
#[derive(Clone, Debug)]
pub struct Header {
    /// Written by a KeePass 2.x pre-release (second signature `0xB54BFB66`).
    pub pre_release: bool,
    pub version: Version,
    /// Every field in file order, the end-of-header field last.
    pub fields: Vec<HeaderField>,
    pub cipher: Cipher,
    pub compression: Compression,
    pub master_seed: Vec<u8>,
    pub encryption_iv: Vec<u8>,
    pub kdf: Kdf,
    /// KDBX 4: the KDF parameters as stored.
    pub kdf_parameters: Option<VariantDictionary>,
    /// KDBX 4: plugin data stored in the clear.
    pub public_custom_data: Option<VariantDictionary>,
    pub comment: Option<Vec<u8>>,
    /// KDBX 3: the inner stream key.
    pub protected_stream_key: Option<Vec<u8>>,
    /// KDBX 3: the 32 bytes the decrypted payload must start with.
    pub stream_start_bytes: Option<Vec<u8>>,
    /// KDBX 3: the inner stream algorithm (KDBX 4 keeps it in the inner header).
    pub inner_stream: Option<InnerStreamAlgorithm>,
    /// Bytes from the signature through the end-of-header field.
    pub length: usize,
    /// SHA-256 of those bytes.
    pub sha256: [u8; 32],
    /// KDBX 4: the SHA-256 stored after the header.
    pub stored_sha256: Option<[u8; 32]>,
    /// KDBX 4: the HMAC-SHA-256 stored after the header; it checks the key.
    pub stored_hmac: Option<[u8; 32]>,
    /// Fields that were ignored or defaulted.
    pub warnings: Vec<String>,
}

fn sized<const N: usize>(field: &HeaderField, name: &str) -> Result<[u8; N]> {
    fixed(&field.data).ok_or_else(|| {
        Error::Malformed(format!(
            "header field {name}: {} bytes where {N} are required",
            field.data.len()
        ))
    })
}

/// Whether `data` starts with the first KeePass signature, so a short file is
/// a truncated database rather than another format.
fn starts_like_keepass(data: &[u8]) -> bool {
    let signature = SIGNATURE_1.to_le_bytes();
    data.len() >= 4 && data[..4] == signature
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(if starts_like_keepass(data) {
                Error::Truncated("file signature")
            } else {
                Error::NotKeePass
            });
        }
        let mut cursor = Cursor::new(data);
        let signature1 = cursor.u32("file signature")?;
        let signature2 = cursor.u32("file signature")?;
        if signature1 != SIGNATURE_1 {
            return Err(Error::NotKeePass);
        }
        let pre_release = match signature2 {
            SIGNATURE_2 => false,
            SIGNATURE_2_PRE_RELEASE => true,
            SIGNATURE_2_KEEPASS1 => return Err(Error::KeePass1),
            _ => return Err(Error::NotKeePass),
        };
        let minor = cursor.u16("file version")?;
        let major = cursor.u16("file version")?;
        let version = Version { major, minor };
        if !(MIN_MAJOR..=MAX_MAJOR).contains(&major) {
            return Err(Error::UnsupportedVersion { major, minor });
        }
        let kdbx4 = version.is_kdbx4();

        let mut fields = Vec::new();
        loop {
            let offset = cursor.position();
            let id = cursor.u8("header field")?;
            let size = if kdbx4 {
                cursor.length_i32("header field")?
            } else {
                usize::from(cursor.u16("header field")?)
            };
            let field = cursor.take(size, "header field")?;
            fields.push(HeaderField {
                id,
                offset,
                data: field.to_vec(),
            });
            if id == 0 {
                break;
            }
        }
        let length = cursor.position();
        let sha256: [u8; 32] = Sha256::digest(&data[..length]).into();
        let (stored_sha256, stored_hmac) = if kdbx4 {
            (
                Some(cursor.bytes32("header SHA-256")?),
                Some(cursor.bytes32("header HMAC")?),
            )
        } else {
            (None, None)
        };

        let mut warnings = Vec::new();
        let mut cipher = None;
        let mut compression = None;
        let mut master_seed = None;
        let mut encryption_iv = None;
        let mut transform_seed = None;
        let mut transform_rounds = None;
        let mut kdf_parameters = None;
        let mut public_custom_data = None;
        let mut comment = None;
        let mut protected_stream_key = None;
        let mut stream_start_bytes = None;
        let mut inner_stream = None;
        for field in &fields {
            match (field.id, kdbx4) {
                (0, _) => {}
                (1, _) => comment = Some(field.data.clone()),
                (2, _) => cipher = Some(Cipher::from_uuid(sized(field, "CipherID")?)),
                (3, _) => {
                    let flags = u32::from_le_bytes(sized(field, "CompressionFlags")?);
                    compression = Some(Compression::from_flags(flags));
                }
                (4, _) => master_seed = Some(field.data.clone()),
                (7, _) => encryption_iv = Some(field.data.clone()),
                (5, false) => transform_seed = Some(field.data.clone()),
                (6, false) => {
                    transform_rounds = Some(u64::from_le_bytes(sized(field, "TransformRounds")?));
                }
                (8, false) => protected_stream_key = Some(field.data.clone()),
                (9, false) => stream_start_bytes = Some(field.data.clone()),
                (10, false) => {
                    let id = u32::from_le_bytes(sized(field, "InnerRandomStreamID")?);
                    inner_stream = Some(InnerStreamAlgorithm::from_id(id));
                }
                (11, true) => kdf_parameters = Some(VariantDictionary::parse(&field.data)?),
                (12, true) => public_custom_data = Some(VariantDictionary::parse(&field.data)?),
                (id, _) => warnings.push(format!(
                    "header field {id} ({} bytes) at {:#x} ignored",
                    field.data.len(),
                    field.offset
                )),
            }
        }

        let missing = |name: &str| Error::Malformed(format!("header without {name}"));
        let cipher = cipher.ok_or_else(|| missing("CipherID"))?;
        let master_seed = master_seed.ok_or_else(|| missing("MasterSeed"))?;
        if master_seed.len() != 32 {
            warnings.push(format!(
                "MasterSeed is {} bytes instead of 32",
                master_seed.len()
            ));
        }
        let encryption_iv = encryption_iv.ok_or_else(|| missing("EncryptionIV"))?;
        let compression = compression.unwrap_or_else(|| {
            // KeePass and KeePassXC assume GZip when the field is absent.
            warnings.push("no CompressionFlags; GZip assumed".into());
            Compression::Gzip
        });
        let kdf = if kdbx4 {
            Kdf::from_dictionary(
                kdf_parameters
                    .as_ref()
                    .ok_or_else(|| missing("KdfParameters"))?,
            )?
        } else {
            Kdf::AesKdf {
                rounds: transform_rounds.ok_or_else(|| missing("TransformRounds"))?,
                seed: transform_seed.ok_or_else(|| missing("TransformSeed"))?,
            }
        };
        if !kdbx4 {
            let start = stream_start_bytes
                .as_ref()
                .ok_or_else(|| missing("StreamStartBytes"))?;
            if start.len() != 32 {
                return Err(Error::Malformed(format!(
                    "StreamStartBytes of {} bytes",
                    start.len()
                )));
            }
            protected_stream_key
                .as_ref()
                .ok_or_else(|| missing("InnerRandomStreamKey"))?;
            inner_stream.ok_or_else(|| missing("InnerRandomStreamID"))?;
        }

        Ok(Self {
            pre_release,
            version,
            fields,
            cipher,
            compression,
            master_seed,
            encryption_iv,
            kdf,
            kdf_parameters,
            public_custom_data,
            comment,
            protected_stream_key,
            stream_start_bytes,
            inner_stream,
            length,
            sha256,
            stored_sha256,
            stored_hmac,
            warnings,
        })
    }

    /// Offset of the encrypted payload in the file.
    pub fn payload_offset(&self) -> usize {
        if self.version.is_kdbx4() {
            self.length + 64
        } else {
            self.length
        }
    }

    /// KDBX 4: whether the SHA-256 stored after the header matches it.
    pub fn sha256_matches(&self) -> Option<bool> {
        self.stored_sha256.map(|stored| stored == self.sha256)
    }

    /// The master seed and cipher, in hex, for logs and reports.
    pub fn describe(&self) -> String {
        format!(
            "KDBX {} {} {} seed {}",
            self.version,
            self.cipher.name(),
            self.kdf.name(),
            hex(&self.master_seed)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field3(id: u8, data: &[u8]) -> Vec<u8> {
        let mut out = vec![id];
        out.extend((data.len() as u16).to_le_bytes());
        out.extend(data);
        out
    }

    fn kdbx3_header() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(SIGNATURE_1.to_le_bytes());
        data.extend(SIGNATURE_2.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(3u16.to_le_bytes());
        data.extend(field3(2, &Cipher::AES256_UUID));
        data.extend(field3(3, &1u32.to_le_bytes()));
        data.extend(field3(4, &[4; 32]));
        data.extend(field3(5, &[5; 32]));
        data.extend(field3(6, &6000u64.to_le_bytes()));
        data.extend(field3(7, &[7; 16]));
        data.extend(field3(8, &[8; 32]));
        data.extend(field3(9, &[9; 32]));
        data.extend(field3(10, &2u32.to_le_bytes()));
        data.extend(field3(42, b"x"));
        data.extend(field3(0, b"\r\n\r\n"));
        data
    }

    #[test]
    fn reads_a_kdbx3_header() {
        let data = kdbx3_header();
        let header = Header::parse(&data).unwrap();
        assert_eq!(header.version, Version { major: 3, minor: 1 });
        assert_eq!(header.cipher, Cipher::Aes256);
        assert_eq!(header.compression, Compression::Gzip);
        assert_eq!(
            header.kdf,
            Kdf::AesKdf {
                rounds: 6000,
                seed: vec![5; 32]
            }
        );
        assert_eq!(header.inner_stream, Some(InnerStreamAlgorithm::Salsa20));
        assert_eq!(header.length, data.len());
        assert_eq!(header.payload_offset(), data.len());
        assert_eq!(header.fields.len(), 11);
        assert_eq!(header.warnings.len(), 1);
        assert_eq!(header.sha256_matches(), None);
    }

    #[test]
    fn tells_other_formats_and_damage_apart() {
        assert!(matches!(
            Header::parse(b"PK\x03\x04"),
            Err(Error::NotKeePass)
        ));
        let mut kdb = kdbx3_header();
        kdb[4..8].copy_from_slice(&SIGNATURE_2_KEEPASS1.to_le_bytes());
        assert!(matches!(Header::parse(&kdb), Err(Error::KeePass1)));
        let mut future = kdbx3_header();
        future[10..12].copy_from_slice(&5u16.to_le_bytes());
        assert!(matches!(
            Header::parse(&future),
            Err(Error::UnsupportedVersion { major: 5, minor: 1 })
        ));
        let data = kdbx3_header();
        assert!(matches!(
            Header::parse(&data[..data.len() - 2]),
            Err(Error::Truncated(_))
        ));
        assert!(matches!(
            Header::parse(&data[..6]),
            Err(Error::Truncated(_))
        ));
    }
}
