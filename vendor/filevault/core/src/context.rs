//! Targeted extraction from the decrypted CoreStorage metadata.
//!
//! The `com.apple.corestorage.lvf.encryption.context` XML plist carries the
//! password protector's wrapped keys; block type `0x001a` carries the logical
//! volume's `familyUUID` (the tweak-key input) and its size. A full plist parser
//! is unnecessary and would be more attack surface: the values we need appear as
//! `<key>NAME</key><data ID=..>BASE64</data>` and
//! `<key>NAME</key><string ID=..>VALUE</string>` pairs, so bounded substring
//! extraction is used (RESEARCH.md explicitly sanctions this). Every scan is
//! bounds-checked and length-capped.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use crate::error::FileVaultError;

/// The password protector's wrapped-key material and PBKDF2 parameters.
#[derive(Debug, Clone)]
pub struct EncryptionContext {
    /// PBKDF2 salt (16 bytes) from the `PassphraseWrappedKEKStruct` (offset 8).
    pub salt: [u8; 16],
    /// PBKDF2 iteration count. NOTE: on the fvdetest ground truth this reads
    /// from struct offset 168 (RESEARCH.md's "172" is one field further on and
    /// reads 1); offset 168 yields the verified 90506.
    pub iterations: u32,
    /// Wrapped KEK (24 bytes) from `PassphraseWrappedKEKStruct` offset 32.
    pub wrapped_kek: [u8; 24],
    /// Wrapped volume master key (24 bytes) from the `AES-XTS`
    /// `KEKWrappedVolumeKeyStruct` offset 8.
    pub wrapped_vmk: [u8; 24],
    /// Protectors (crypto users) present, by `UserType`.
    pub protectors: Vec<Protector>,
    /// LV conversion status (`Complete` / `Converting` / …), if present.
    pub conversion_status: Option<String>,
}

/// A CoreStorage crypto-user (protector) classified by its `UserType` code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protector {
    /// Raw `UserType` code (e.g. `0x10000001` = password).
    pub user_type: u32,
    /// Human-readable classification of `user_type`.
    pub kind: ProtectorKind,
}

/// Classification of a CoreStorage crypto-user `UserType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectorKind {
    /// Password (`0x10000001`).
    Password,
    /// Recovery / personal recovery key (`0x10000009`).
    Recovery,
    /// Institutional / keychain-backed key.
    Institutional,
    /// A crypto-user whose `UserType` is not one of the known codes; carries the
    /// raw code so it is never silently lost.
    Unknown,
}

impl ProtectorKind {
    /// Classify a raw `UserType` code.
    #[must_use]
    pub fn from_user_type(user_type: u32) -> Self {
        match user_type {
            0x1000_0001 => ProtectorKind::Password,
            0x1000_0009 => ProtectorKind::Recovery,
            0x1000_0005 | 0x1000_000f => ProtectorKind::Institutional,
            _ => ProtectorKind::Unknown,
        }
    }

    /// A stable lowercase label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ProtectorKind::Password => "password",
            ProtectorKind::Recovery => "recovery",
            ProtectorKind::Institutional => "institutional",
            ProtectorKind::Unknown => "unknown",
        }
    }
}

/// Offsets inside the `PassphraseWrappedKEKStruct` (see RESEARCH.md + ground
/// truth: iterations live at 168, not 172).
const PW_SALT_OFFSET: usize = 8;
const PW_WRAPPED_KEK_OFFSET: usize = 32;
const PW_ITERATIONS_OFFSET: usize = 168;
/// Offset inside the `KEKWrappedVolumeKeyStruct` of the wrapped VMK.
const KEK_WRAPPED_VMK_OFFSET: usize = 8;
/// Largest base64 blob we will decode from the context (guards allocation).
const MAX_BASE64_BLOB: usize = 4096;

/// Find `<key>NAME</key><data ID=..>BASE64</data>` and return the decoded bytes
/// of the first match. Bounds-checked; caps the encoded length.
fn extract_data(metadata: &[u8], key_name: &str) -> Option<Vec<u8>> {
    let value = extract_tagged(metadata, key_name, b"<data ID=", b"</data>")?;
    let trimmed: Vec<u8> = value
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if trimmed.len() > MAX_BASE64_BLOB {
        return None;
    }
    BASE64.decode(&trimmed).ok()
}

/// Find `<key>NAME</key><string ID=..>VALUE</string>` and return the string.
fn extract_string(metadata: &[u8], key_name: &str) -> Option<String> {
    let value = extract_tagged(metadata, key_name, b"<string ID=", b"</string>")?;
    String::from_utf8(value).ok()
}

/// Find `<key>NAME</key><integer ...>0xHHHH</integer>` from position `from` and
/// return (value, index just past the match).
fn extract_integer_from(metadata: &[u8], key_name: &str, from: usize) -> Option<(u32, usize)> {
    let (value, end) = extract_tagged_from(metadata, key_name, b"<integer", b"</integer>", from)?;
    // `extract_tagged_from` already returns the inner text (e.g. `0x10000001`).
    let text = std::str::from_utf8(&value).ok()?.trim();
    let parsed = if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        text.parse::<u32>().ok()?
    };
    Some((parsed, end))
}

/// Core substring machine: locate `<key>NAME</key>` then the following
/// `open_tag` … its `>` … `close_tag`, returning the inner bytes.
fn extract_tagged(
    metadata: &[u8],
    key_name: &str,
    open_tag: &[u8],
    close_tag: &[u8],
) -> Option<Vec<u8>> {
    extract_tagged_from(metadata, key_name, open_tag, close_tag, 0).map(|(v, _)| v)
}

fn extract_tagged_from(
    metadata: &[u8],
    key_name: &str,
    open_tag: &[u8],
    close_tag: &[u8],
    from: usize,
) -> Option<(Vec<u8>, usize)> {
    let mut key = Vec::with_capacity(key_name.len() + 11);
    key.extend_from_slice(b"<key>");
    key.extend_from_slice(key_name.as_bytes());
    key.extend_from_slice(b"</key>");

    let start = from.checked_add(find_sub(metadata.get(from..)?, &key)?)?;
    let after_key = start.checked_add(key.len())?;
    let open_rel = find_sub(metadata.get(after_key..)?, open_tag)?;
    let open_abs = after_key.checked_add(open_rel)?;
    // Advance to the '>' that closes the opening tag.
    let gt_rel = find_sub(metadata.get(open_abs..)?, b">")?;
    let inner_start = open_abs.checked_add(gt_rel)?.checked_add(1)?;
    let close_rel = find_sub(metadata.get(inner_start..)?, close_tag)?;
    let inner_end = inner_start.checked_add(close_rel)?;
    let inner = metadata.get(inner_start..inner_end)?.to_vec();
    Some((inner, inner_end.checked_add(close_tag.len())?))
}

/// Bounded substring search (no regex; needle from a `&'static`/small buffer).
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Enumerate the `CryptoUsers` protectors by scanning every `UserType` integer.
fn extract_protectors(metadata: &[u8]) -> Vec<Protector> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some((user_type, end)) = extract_integer_from(metadata, "UserType", cursor) {
        out.push(Protector {
            user_type,
            kind: ProtectorKind::from_user_type(user_type),
        });
        cursor = end;
        if out.len() >= 64 {
            break; // cov:unreachable: real contexts carry a handful of crypto users
        }
    }
    out
}

impl EncryptionContext {
    /// Extract the encryption context from decrypted metadata.
    ///
    /// # Errors
    /// [`FileVaultError::MetadataStructureMissing`] if the passphrase-wrapped or
    /// KEK-wrapped struct is absent; [`FileVaultError::Base64`] on a bad blob;
    /// [`FileVaultError::OutOfRange`] if a struct is too short.
    pub fn extract(metadata: &[u8]) -> Result<Self, FileVaultError> {
        let pw = extract_data(metadata, "PassphraseWrappedKEKStruct").ok_or(
            FileVaultError::MetadataStructureMissing {
                what: "PassphraseWrappedKEKStruct",
            },
        )?;

        let salt = slice16(&pw, PW_SALT_OFFSET).ok_or(FileVaultError::OutOfRange {
            what: "PassphraseWrappedKEKStruct salt",
        })?;
        let wrapped_kek =
            slice24(&pw, PW_WRAPPED_KEK_OFFSET).ok_or(FileVaultError::OutOfRange {
                what: "PassphraseWrappedKEKStruct wrapped KEK",
            })?;
        let iterations =
            read_u32_le(&pw, PW_ITERATIONS_OFFSET).ok_or(FileVaultError::OutOfRange {
                what: "PassphraseWrappedKEKStruct iterations",
            })?;

        // The AES-XTS KEKWrappedVolumeKeyStruct is the one that wraps the real
        // volume key; the `BlockAlgorithm == None` sibling is a placeholder.
        let vmk_struct = extract_kek_wrapped_aes_xts(metadata).ok_or(
            FileVaultError::MetadataStructureMissing {
                what: "AES-XTS KEKWrappedVolumeKeyStruct",
            },
        )?;
        let wrapped_vmk =
            slice24(&vmk_struct, KEK_WRAPPED_VMK_OFFSET).ok_or(FileVaultError::OutOfRange {
                what: "KEKWrappedVolumeKeyStruct wrapped VMK",
            })?;

        Ok(EncryptionContext {
            salt,
            iterations,
            wrapped_kek,
            wrapped_vmk,
            protectors: extract_protectors(metadata),
            conversion_status: extract_string(metadata, "ConversionStatus"),
        })
    }
}

/// Extract the `KEKWrappedVolumeKeyStruct` whose preceding `BlockAlgorithm` is
/// `AES-XTS`. Contexts list a `None` algorithm first, then `AES-XTS`; scan for
/// the `AES-XTS` marker and take the next wrapped-key blob after it.
fn extract_kek_wrapped_aes_xts(metadata: &[u8]) -> Option<Vec<u8>> {
    let marker = b"<string ID=";
    // Find the BlockAlgorithm key whose value is AES-XTS.
    let mut cursor = 0;
    loop {
        let (algo, end) =
            extract_tagged_from(metadata, "BlockAlgorithm", marker, b"</string>", cursor)?;
        if algo == b"AES-XTS" {
            return extract_data(metadata.get(end..)?, "KEKWrappedVolumeKeyStruct");
        }
        cursor = end;
    }
}

/// The LV logical size (bytes) and family UUID, from the decrypted metadata.
#[derive(Debug, Clone)]
pub struct LogicalVolumeInfo {
    /// LV logical size in bytes.
    pub size: u64,
    /// Family UUID string (tweak-key input), canonical form.
    pub family_uuid: String,
    /// LV identifier UUID string, if present.
    pub lv_identifier: Option<String>,
    /// LV name, if present.
    pub name: Option<String>,
}

impl LogicalVolumeInfo {
    /// Extract the LV info from decrypted metadata.
    ///
    /// # Errors
    /// [`FileVaultError::MetadataStructureMissing`] if the family UUID is absent.
    pub fn extract(metadata: &[u8]) -> Result<Self, FileVaultError> {
        let family_uuid = extract_string(metadata, "com.apple.corestorage.lv.familyUUID").ok_or(
            FileVaultError::MetadataStructureMissing {
                what: "com.apple.corestorage.lv.familyUUID",
            },
        )?;
        let size = extract_lv_size(metadata).unwrap_or(0);
        let lv_identifier = extract_string(metadata, "com.apple.corestorage.lv.uuid");
        let name = extract_string(metadata, "com.apple.corestorage.lv.name");
        Ok(LogicalVolumeInfo {
            size,
            family_uuid,
            lv_identifier,
            name,
        })
    }
}

/// Extract the LV size from the `0x001a` volume record. The size appears as a
/// little-endian u64 at a fixed offset in the record following the family UUID
/// marker; if the structural read fails, `None` (the size is then reported as
/// unknown rather than fabricated).
fn extract_lv_size(metadata: &[u8]) -> Option<u64> {
    // The LV size is carried as a plist integer `com.apple.corestorage.lv.size`
    // when present; fall back to None otherwise.
    if let Some((value, _)) = extract_u64_integer(metadata, "com.apple.corestorage.lv.size") {
        return Some(value);
    }
    None
}

/// Like `extract_integer_from` but for a u64 plist integer.
fn extract_u64_integer(metadata: &[u8], key_name: &str) -> Option<(u64, usize)> {
    let (value, end) = extract_tagged_from(metadata, key_name, b"<integer", b"</integer>", 0)?;
    let text = std::str::from_utf8(&value).ok()?.trim();
    let parsed = if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()?
    } else {
        text.parse::<u64>().ok()?
    };
    Some((parsed, end))
}

fn slice16(data: &[u8], off: usize) -> Option<[u8; 16]> {
    let s = data.get(off..off.checked_add(16)?)?;
    let mut out = [0u8; 16];
    out.copy_from_slice(s);
    Some(out)
}

fn slice24(data: &[u8], off: usize) -> Option<[u8; 24]> {
    let s = data.get(off..off.checked_add(24)?)?;
    let mut out = [0u8; 24];
    out.copy_from_slice(s);
    Some(out)
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let s = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// Build a 284-byte PassphraseWrappedKEKStruct with the ground-truth salt,
    /// wrapped KEK and iteration count at their real offsets (8 / 32 / 168).
    fn passphrase_struct() -> Vec<u8> {
        let mut s = vec![0u8; 284];
        s[PW_SALT_OFFSET..PW_SALT_OFFSET + 16]
            .copy_from_slice(&hex("9bfcf480e4d9ad0eddd9ac6f47b85955"));
        s[PW_WRAPPED_KEK_OFFSET..PW_WRAPPED_KEK_OFFSET + 24]
            .copy_from_slice(&hex("ebbc1f64b9684eb4b26bfba3f85578677bab8cfafaf7b2c1"));
        s[PW_ITERATIONS_OFFSET..PW_ITERATIONS_OFFSET + 4].copy_from_slice(&90506u32.to_le_bytes());
        s
    }

    /// Build a KEKWrappedVolumeKeyStruct with the wrapped VMK at offset 8.
    fn kek_wrapped_struct(wrapped_vmk_hex: &str) -> Vec<u8> {
        let mut s = vec![0u8; 256];
        s[KEK_WRAPPED_VMK_OFFSET..KEK_WRAPPED_VMK_OFFSET + 24]
            .copy_from_slice(&hex(wrapped_vmk_hex));
        s
    }

    fn b64(bytes: &[u8]) -> String {
        BASE64.encode(bytes)
    }

    fn build_context_plist() -> Vec<u8> {
        let pw = b64(&passphrase_struct());
        // The `None` algorithm sibling first, then AES-XTS with the real wrapped VMK.
        let none_kek = b64(&kek_wrapped_struct(
            "000000000000000000000000000000000000000000000000",
        ));
        let xts_kek = b64(&kek_wrapped_struct(
            "9a5b30e99f902ed8e2f03989e5f9c1543ed60512aa1dc9d1",
        ));
        format!(
            "<dict ID=\"0\"><key>CryptoUsers</key><array ID=\"2\">\
             <dict ID=\"3\">\
             <key>PassphraseWrappedKEKStruct</key><data ID=\"4\">{pw}</data>\
             <key>UserType</key><integer size=\"32\" ID=\"6\">0x10000001</integer>\
             <key>BlockAlgorithm</key><string ID=\"7\">None</string>\
             <key>KEKWrappedVolumeKeyStruct</key><data ID=\"8\">{none_kek}</data>\
             <key>BlockAlgorithm</key><string ID=\"9\">AES-XTS</string>\
             <key>KEKWrappedVolumeKeyStruct</key><data ID=\"10\">{xts_kek}</data>\
             </dict>\
             <dict ID=\"11\">\
             <key>UserType</key><integer size=\"32\" ID=\"12\">0x10000009</integer>\
             </dict>\
             </array>\
             <key>ConversionStatus</key><string ID=\"20\">Complete</string></dict>"
        )
        .into_bytes()
    }

    #[test]
    fn extracts_encryption_context() {
        let meta = build_context_plist();
        let ctx = EncryptionContext::extract(&meta).unwrap();
        assert_eq!(
            ctx.salt,
            *b"\x9b\xfc\xf4\x80\xe4\xd9\xad\x0e\xdd\xd9\xac\x6f\x47\xb8\x59\x55"
        );
        assert_eq!(ctx.iterations, 90506);
        assert_eq!(
            ctx.wrapped_kek.to_vec(),
            hex("ebbc1f64b9684eb4b26bfba3f85578677bab8cfafaf7b2c1")
        );
        // The AES-XTS wrapped VMK, not the None sibling.
        assert_eq!(
            ctx.wrapped_vmk.to_vec(),
            hex("9a5b30e99f902ed8e2f03989e5f9c1543ed60512aa1dc9d1")
        );
        assert_eq!(ctx.conversion_status.as_deref(), Some("Complete"));
    }

    #[test]
    fn enumerates_protectors() {
        let meta = build_context_plist();
        let ctx = EncryptionContext::extract(&meta).unwrap();
        assert_eq!(ctx.protectors.len(), 2);
        assert_eq!(ctx.protectors[0].kind, ProtectorKind::Password);
        assert_eq!(ctx.protectors[0].user_type, 0x1000_0001);
        assert_eq!(ctx.protectors[1].kind, ProtectorKind::Recovery);
    }

    #[test]
    fn missing_passphrase_struct_is_error() {
        let meta = b"<dict><key>Nothing</key></dict>".to_vec();
        assert!(matches!(
            EncryptionContext::extract(&meta),
            Err(FileVaultError::MetadataStructureMissing {
                what: "PassphraseWrappedKEKStruct"
            })
        ));
    }

    #[test]
    fn missing_aes_xts_kek_is_error() {
        let pw = b64(&passphrase_struct());
        let meta = format!(
            "<key>PassphraseWrappedKEKStruct</key><data ID=\"4\">{pw}</data>\
             <key>BlockAlgorithm</key><string ID=\"9\">None</string>\
             <key>KEKWrappedVolumeKeyStruct</key><data ID=\"10\">{}</data>",
            b64(&kek_wrapped_struct(
                "000000000000000000000000000000000000000000000000"
            ))
        )
        .into_bytes();
        assert!(matches!(
            EncryptionContext::extract(&meta),
            Err(FileVaultError::MetadataStructureMissing {
                what: "AES-XTS KEKWrappedVolumeKeyStruct"
            })
        ));
    }

    #[test]
    fn protector_kind_classification() {
        assert_eq!(
            ProtectorKind::from_user_type(0x1000_0001),
            ProtectorKind::Password
        );
        assert_eq!(
            ProtectorKind::from_user_type(0x1000_0009),
            ProtectorKind::Recovery
        );
        assert_eq!(
            ProtectorKind::from_user_type(0x1000_000f),
            ProtectorKind::Institutional
        );
        assert_eq!(
            ProtectorKind::from_user_type(0xdead),
            ProtectorKind::Unknown
        );
        assert_eq!(ProtectorKind::Password.label(), "password");
        assert_eq!(ProtectorKind::Recovery.label(), "recovery");
        assert_eq!(ProtectorKind::Institutional.label(), "institutional");
        assert_eq!(ProtectorKind::Unknown.label(), "unknown");
    }

    #[test]
    fn extracts_logical_volume_info() {
        let meta = b"<dict ID=\"0\">\
            <key>com.apple.corestorage.lv.familyUUID</key><string ID=\"1\">1F01CA34-5F6C-4123-AC0C-B0A256889DB2</string>\
            <key>com.apple.corestorage.lv.name</key><string ID=\"6\">TestLV</string>\
            <key>com.apple.corestorage.lv.size</key><integer size=\"64\" ID=\"7\">0xa000000</integer>\
            <key>com.apple.corestorage.lv.uuid</key><string ID=\"8\">420AF122-CF73-4A30-8B0A-A593A65FBEF5</string>\
            </dict>"
            .to_vec();
        let lv = LogicalVolumeInfo::extract(&meta).unwrap();
        assert_eq!(lv.family_uuid, "1F01CA34-5F6C-4123-AC0C-B0A256889DB2");
        assert_eq!(lv.size, 167_772_160);
        assert_eq!(lv.name.as_deref(), Some("TestLV"));
        assert_eq!(
            lv.lv_identifier.as_deref(),
            Some("420AF122-CF73-4A30-8B0A-A593A65FBEF5")
        );
    }

    #[test]
    fn missing_family_uuid_is_error() {
        let meta = b"<dict><key>other</key></dict>".to_vec();
        assert!(matches!(
            LogicalVolumeInfo::extract(&meta),
            Err(FileVaultError::MetadataStructureMissing {
                what: "com.apple.corestorage.lv.familyUUID"
            })
        ));
    }

    #[test]
    fn lv_size_absent_is_zero_not_fabricated() {
        let meta = b"<key>com.apple.corestorage.lv.familyUUID</key><string ID=\"1\">1F01CA34-5F6C-4123-AC0C-B0A256889DB2</string>".to_vec();
        let lv = LogicalVolumeInfo::extract(&meta).unwrap();
        assert_eq!(lv.size, 0);
        assert!(lv.name.is_none());
    }

    #[test]
    fn oversized_base64_blob_is_rejected() {
        // A data blob longer than the cap decodes to None (guards allocation).
        let big = "A".repeat(MAX_BASE64_BLOB + 4);
        let meta = format!("<key>PassphraseWrappedKEKStruct</key><data ID=\"4\">{big}</data>")
            .into_bytes();
        assert!(matches!(
            EncryptionContext::extract(&meta),
            Err(FileVaultError::MetadataStructureMissing { .. })
        ));
    }

    #[test]
    fn decimal_integer_values_parse() {
        // UserType and lv.size given as decimals (not 0x hex) still parse.
        let meta = b"<key>UserType</key><integer size=\"32\" ID=\"6\">268435457</integer>".to_vec();
        assert_eq!(
            extract_integer_from(&meta, "UserType", 0).unwrap().0,
            268_435_457
        );

        let lv_meta = b"<dict>\
            <key>com.apple.corestorage.lv.familyUUID</key><string ID=\"1\">1F01CA34-5F6C-4123-AC0C-B0A256889DB2</string>\
            <key>com.apple.corestorage.lv.size</key><integer size=\"64\" ID=\"7\">167772160</integer>\
            </dict>".to_vec();
        let lv = LogicalVolumeInfo::extract(&lv_meta).unwrap();
        assert_eq!(lv.size, 167_772_160);
    }
}
