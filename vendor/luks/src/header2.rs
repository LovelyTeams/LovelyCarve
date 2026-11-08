//! LUKS2 header parsing: the 4096-byte binary header plus the JSON metadata area.
//!
//! LUKS2 replaces the fixed LUKS1 `phdr` with a small binary header (magic,
//! version, `hdr_size`) followed by a JSON document describing keyslots (Argon2
//! or PBKDF2 KDF + anti-forensic area), data segments (cipher, sector size, IV
//! tweak), and digests (master-key verification). See the *LUKS2 On-Disk Format
//! Specification*.

use base64::Engine;
use serde_json::Value;

use crate::bytes::{be_u16, be_u64, bytes_n};
use crate::error::{LuksError, Result};
use crate::header::LUKS_MAGIC;

/// Size of the LUKS2 binary header preceding the JSON area.
pub const LUKS2_BIN_HDR_LEN: usize = 4096;

/// A LUKS2 keyslot key-derivation function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Luks2Kdf {
    /// Argon2i / Argon2id (the LUKS2 default).
    Argon2 {
        /// `argon2i` or `argon2id`.
        kind: String,
        /// Time cost.
        time: u32,
        /// Memory cost (KiB).
        memory: u32,
        /// Parallelism.
        cpus: u32,
        /// Salt bytes.
        salt: Vec<u8>,
    },
    /// PBKDF2 (legacy / converted volumes).
    Pbkdf2 {
        /// Hash spec.
        hash: String,
        /// Iteration count.
        iterations: u32,
        /// Salt bytes.
        salt: Vec<u8>,
    },
}

/// A LUKS2 keyslot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Luks2Keyslot {
    /// Keyslot id (JSON object key).
    pub id: String,
    /// Master-key size in bytes.
    pub key_size: usize,
    /// Anti-forensic stripe count.
    pub af_stripes: u32,
    /// Anti-forensic hash.
    pub af_hash: String,
    /// Byte offset of the AF key-material area.
    pub area_offset: u64,
    /// Byte length of the AF key-material area.
    pub area_size: u64,
    /// AF-area cipher (e.g. `aes-xts-plain64`).
    pub area_encryption: String,
    /// Key length used to decrypt the keyslot area; independent of the master key.
    pub area_key_size: usize,
    /// The KDF.
    pub kdf: Luks2Kdf,
}

/// A LUKS2 data segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Luks2Segment {
    /// Segment id.
    pub id: String,
    /// Byte offset of the encrypted data.
    pub offset: u64,
    /// Cipher (e.g. `aes-xts-plain64`).
    pub encryption: String,
    /// Encryption sector size (512 or 4096).
    pub sector_size: usize,
    /// IV tweak base added to the plain64 sector number.
    pub iv_tweak: u128,
}

/// A LUKS2 digest (master-key verifier).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Luks2Digest {
    /// Digest type (`pbkdf2`).
    pub kind: String,
    /// Hash spec.
    pub hash: String,
    /// PBKDF2 iteration count.
    pub iterations: u32,
    /// Salt bytes.
    pub salt: Vec<u8>,
    /// Expected digest bytes.
    pub digest: Vec<u8>,
    /// Keyslot ids this digest verifies.
    pub keyslots: Vec<String>,
    /// Data segments verified by this digest (empty means an unbound key).
    pub segments: Vec<String>,
}

/// A parsed LUKS2 header.
#[derive(Debug, Clone)]
pub struct Luks2Header {
    /// Version (2).
    pub version: u16,
    /// Total header size (binary + JSON) in bytes.
    pub hdr_size: u64,
    /// Keyslots.
    pub keyslots: Vec<Luks2Keyslot>,
    /// Data segments.
    pub segments: Vec<Luks2Segment>,
    /// Digests.
    pub digests: Vec<Luks2Digest>,
}

/// Read a JSON value that LUKS2 may encode as a string ("32768") or a number.
fn as_u64(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn b64(v: &Value) -> Vec<u8> {
    v.as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default()
}

impl Luks2Header {
    /// Parse a LUKS2 header from a buffer covering the binary header and JSON area.
    ///
    /// # Errors
    /// [`LuksError::NotLuks`] / [`LuksError::UnsupportedVersion`] on a bad
    /// signature/version, [`LuksError::MalformedHeader`] if the JSON is absent or
    /// invalid.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let magic = bytes_n::<6>(data, 0);
        if magic != LUKS_MAGIC {
            return Err(LuksError::NotLuks { found: magic });
        }
        let version = be_u16(data, 6);
        if version != 2 {
            return Err(LuksError::UnsupportedVersion { version });
        }
        let hdr_size = be_u64(data, 8);

        let end = (hdr_size as usize).min(data.len());
        let json_area = data
            .get(LUKS2_BIN_HDR_LEN..end)
            .ok_or(LuksError::MalformedHeader {
                what: "json area",
                need: LUKS2_BIN_HDR_LEN,
                got: data.len(),
            })?;
        let json_end = json_area
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(json_area.len());
        let root: Value = serde_json::from_slice(&json_area[..json_end]).map_err(|_| {
            LuksError::MalformedHeader {
                what: "json parse",
                need: json_end,
                got: json_area.len(),
            }
        })?;

        let keyslots = root
            .get("keyslots")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(id, v)| parse_keyslot(id, v))
                    .collect()
            })
            .unwrap_or_default();
        let segments = root
            .get("segments")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(id, v)| parse_segment(id, v))
                    .collect()
            })
            .unwrap_or_default();
        let digests = root
            .get("digests")
            .and_then(Value::as_object)
            .map(|m| m.values().filter_map(parse_digest).collect())
            .unwrap_or_default();

        Ok(Luks2Header {
            version,
            hdr_size,
            keyslots,
            segments,
            digests,
        })
    }

    /// The first `crypt` data segment (the one we decrypt).
    #[must_use]
    pub fn crypt_segment(&self) -> Option<&Luks2Segment> {
        self.segments.iter().find(|s| !s.encryption.is_empty())
    }
}

fn parse_keyslot(id: &str, v: &Value) -> Option<Luks2Keyslot> {
    let af = v.get("af")?;
    let area = v.get("area")?;
    let kdf_v = v.get("kdf")?;
    let kdf = match kdf_v.get("type").and_then(Value::as_str)? {
        "pbkdf2" => Luks2Kdf::Pbkdf2 {
            hash: kdf_v
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or("sha256")
                .to_string(),
            iterations: as_u64(kdf_v.get("iterations")?) as u32,
            salt: b64(kdf_v.get("salt")?),
        },
        kind @ ("argon2i" | "argon2id") => Luks2Kdf::Argon2 {
            kind: kind.to_string(),
            time: as_u64(kdf_v.get("time")?) as u32,
            memory: as_u64(kdf_v.get("memory")?) as u32,
            cpus: as_u64(kdf_v.get("cpus")?) as u32,
            salt: b64(kdf_v.get("salt")?),
        },
        _ => return None,
    };
    Some(Luks2Keyslot {
        id: id.to_string(),
        key_size: as_u64(v.get("key_size")?) as usize,
        af_stripes: as_u64(af.get("stripes")?) as u32,
        af_hash: af
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or("sha256")
            .to_string(),
        area_offset: as_u64(area.get("offset")?),
        area_size: as_u64(area.get("size")?),
        area_encryption: area.get("encryption").and_then(Value::as_str)?.to_string(),
        area_key_size: as_u64(area.get("key_size").unwrap_or(v.get("key_size")?)) as usize,
        kdf,
    })
}

fn parse_segment(id: &str, v: &Value) -> Option<Luks2Segment> {
    Some(Luks2Segment {
        id: id.to_string(),
        offset: as_u64(v.get("offset")?),
        encryption: v.get("encryption").and_then(Value::as_str)?.to_string(),
        sector_size: as_u64(v.get("sector_size")?) as usize,
        iv_tweak: u128::from(as_u64(v.get("iv_tweak").unwrap_or(&Value::Null))),
    })
}

fn parse_digest(v: &Value) -> Option<Luks2Digest> {
    Some(Luks2Digest {
        kind: v.get("type").and_then(Value::as_str)?.to_string(),
        hash: v
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or("sha256")
            .to_string(),
        iterations: as_u64(v.get("iterations").unwrap_or(&Value::Null)) as u32,
        salt: b64(v.get("salt")?),
        digest: b64(v.get("digest")?),
        keyslots: v
            .get("keyslots")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        segments: v
            .get("segments")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_luks2(json: &str) -> Vec<u8> {
        let hdr_size = LUKS2_BIN_HDR_LEN + 12288;
        let mut buf = vec![0u8; hdr_size];
        buf[0..6].copy_from_slice(&LUKS_MAGIC);
        buf[6..8].copy_from_slice(&2u16.to_be_bytes());
        buf[8..16].copy_from_slice(&(hdr_size as u64).to_be_bytes());
        buf[LUKS2_BIN_HDR_LEN..LUKS2_BIN_HDR_LEN + json.len()].copy_from_slice(json.as_bytes());
        buf
    }

    const ORACLE_JSON: &str = r#"{
      "keyslots":{"0":{"type":"luks2","key_size":64,
        "af":{"type":"luks1","stripes":4000,"hash":"sha256"},
        "area":{"type":"raw","offset":"32768","size":"258048","encryption":"aes-xts-plain64","key_size":64},
        "kdf":{"type":"argon2id","time":4,"memory":32,"cpus":1,"salt":"pUcL4RKUCh/iGgNoo6PW95eKTZm6RsS4Ds7Kn4gCdVs="}}},
      "segments":{"0":{"type":"crypt","offset":"16777216","size":"dynamic","iv_tweak":"0","encryption":"aes-xts-plain64","sector_size":4096}},
      "digests":{"0":{"type":"pbkdf2","keyslots":["0"],"segments":["0"],"hash":"sha256","iterations":1000,
        "salt":"IUaJeXA4vB2CruGetgh6GOlfgvB2WoD2UzWGb2K6KLc=","digest":"yqVonIxK8BFwHFIsUaacNsXcz65r6EnYBVgF2fDPgps="}}}"#;

    #[test]
    fn parses_luks2_keyslot_segment_digest() {
        let h = Luks2Header::parse(&build_luks2(ORACLE_JSON)).unwrap();
        assert_eq!(h.version, 2);
        assert_eq!(h.keyslots.len(), 1);
        let k = &h.keyslots[0];
        assert_eq!(k.key_size, 64);
        assert_eq!(k.af_stripes, 4000);
        assert_eq!(k.area_offset, 32768);
        assert_eq!(k.area_encryption, "aes-xts-plain64");
        match &k.kdf {
            Luks2Kdf::Argon2 {
                kind,
                time,
                memory,
                cpus,
                salt,
            } => {
                assert_eq!(kind, "argon2id");
                assert_eq!((*time, *memory, *cpus), (4, 32, 1));
                assert_eq!(salt.len(), 32);
            }
            Luks2Kdf::Pbkdf2 { .. } => panic!("expected argon2id"), // cov:unreachable: fixture keyslot 0 is always argon2id
        }
        let seg = h.crypt_segment().unwrap();
        assert_eq!(seg.offset, 16_777_216);
        assert_eq!(seg.sector_size, 4096);
        assert_eq!(seg.iv_tweak, 0);
        assert_eq!(h.digests.len(), 1);
        assert_eq!(h.digests[0].kind, "pbkdf2");
        assert_eq!(h.digests[0].digest.len(), 32);
        assert_eq!(h.digests[0].keyslots, vec!["0".to_string()]);
    }

    #[test]
    fn rejects_non_luks2() {
        assert!(matches!(
            Luks2Header::parse(&[0u8; 5000]).unwrap_err(),
            LuksError::NotLuks { .. }
        ));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut b = build_luks2(ORACLE_JSON);
        b[6..8].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(
            Luks2Header::parse(&b).unwrap_err(),
            LuksError::UnsupportedVersion { version: 1 }
        ));
    }

    #[test]
    fn rejects_bad_json() {
        let b = build_luks2("{not valid json");
        assert!(matches!(
            Luks2Header::parse(&b).unwrap_err(),
            LuksError::MalformedHeader { .. }
        ));
    }

    #[test]
    fn parses_pbkdf2_keyslot_variant() {
        let json = r#"{"keyslots":{"0":{"key_size":32,
          "af":{"stripes":4000,"hash":"sha1"},
          "area":{"offset":"32768","size":"128000","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":50000,"salt":"IUaJeXA4vB2CruGetgh6GOlfgvB2WoD2UzWGb2K6KLc="}}},
          "segments":{},"digests":{}}"#;
        let h = Luks2Header::parse(&build_luks2(json)).unwrap();
        match &h.keyslots[0].kdf {
            Luks2Kdf::Pbkdf2 {
                hash,
                iterations,
                salt,
            } => {
                assert_eq!(hash, "sha256");
                assert_eq!(*iterations, 50000);
                assert_eq!(salt.len(), 32);
            }
            Luks2Kdf::Argon2 { .. } => panic!("expected pbkdf2"), // cov:unreachable: fixture keyslot 0 is always pbkdf2
        }
    }

    #[test]
    fn as_u64_non_numeric_field_defaults_zero() {
        // key_size given as a JSON bool exercises as_u64's `_ => 0` fallback.
        let json = r#"{"keyslots":{"0":{"key_size":true,
          "af":{"stripes":4000,"hash":"sha256"},
          "area":{"offset":"32768","size":"128000","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{},"digests":{}}"#;
        let h = Luks2Header::parse(&build_luks2(json)).unwrap();
        assert_eq!(h.keyslots.len(), 1);
        assert_eq!(h.keyslots[0].key_size, 0);
    }

    #[test]
    fn unknown_kdf_type_drops_keyslot() {
        // An unrecognized kdf type hits parse_keyslot's `_ => return None`, so the
        // keyslot is dropped rather than mis-parsed.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":4000,"hash":"sha256"},
          "area":{"offset":"32768","size":"128000","encryption":"aes-xts-plain64"},
          "kdf":{"type":"scrypt","salt":"AAAA"}}},
          "segments":{},"digests":{}}"#;
        let h = Luks2Header::parse(&build_luks2(json)).unwrap();
        assert!(h.keyslots.is_empty());
    }
}
