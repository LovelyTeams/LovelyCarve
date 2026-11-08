//! Alternate LUKS credentials: verified volume keys and UTF-16 passwords.

use crate::crypto::{derive_key_within, DERIVATION_BUDGET, UNLOCK_BUDGET};
use crate::header::{LUKS1_PHDR_LEN, MK_DIGEST_LEN};
use crate::volume::{payload1, payload2, read_header2, UnlockDeadline};
use crate::{validate_cipher, DecryptedPayload, Luks1Header, LuksError, LuksVolume, Result};
use std::io::{Read, Seek, SeekFrom};
use zeroize::Zeroizing;

impl LuksVolume {
    /// Unlock LUKS1 or a single-segment LUKS2 container with a recovered master
    /// key. A matching digest bound to the selected LUKS2 segment is required.
    pub fn unlock_with_master_key<R: Read + Seek>(
        mut reader: R,
        key: &[u8],
    ) -> Result<DecryptedPayload<R>> {
        let mut prefix = [0; 8];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut prefix)?;
        if !prefix.starts_with(&crate::header::LUKS_MAGIC) {
            return Err(LuksError::NotLuks {
                found: crate::bytes::bytes_n(&prefix, 0),
            });
        }
        match crate::bytes::be_u16(&prefix, 6) {
            1 => Self::unlock1_with_master_key(reader, key),
            2 => {
                let (header, uuid) = read_header2(&mut reader)?;
                if header.segments.len() != 1 {
                    return Err(LuksError::Unsupported {
                        what: "LUKS2 segment layout",
                        value: "expected one crypt segment".into(),
                    });
                }
                let segment = header.crypt_segment().ok_or(LuksError::NoActiveKeyslot)?;
                let (name, mode) = crate::volume::split_encryption(&segment.encryption);
                validate_cipher(&name, &mode, key.len())?;
                let deadline = UnlockDeadline::new(UNLOCK_BUDGET);
                for (tried, digest) in header.digests.iter().enumerate() {
                    if digest.kind != "pbkdf2" || !digest.segments.contains(&segment.id) {
                        continue;
                    }
                    if digest.digest.is_empty() {
                        return Err(LuksError::AuthenticationFailed);
                    }
                    let actual = derive_key_within(
                        &digest.hash,
                        key,
                        &digest.salt,
                        digest.iterations,
                        digest.digest.len(),
                        deadline.remaining(tried)?,
                    )?;
                    if actual == digest.digest {
                        return payload2(reader, segment, uuid, key.to_vec());
                    }
                }
                Err(LuksError::AuthenticationFailed)
            }
            version => Err(LuksError::UnsupportedVersion { version }),
        }
    }

    /// Unlock LUKS1 with a recovered volume (master) key, bypassing keyslots.
    /// Verifies the supplied key against the header digest before exposing data;
    /// works even when all password keyslots are disabled.
    pub fn unlock1_with_master_key<R: Read + Seek>(
        mut reader: R,
        key: &[u8],
    ) -> Result<DecryptedPayload<R>> {
        let mut bytes = [0; LUKS1_PHDR_LEN];
        reader.seek(SeekFrom::Start(0))?;
        reader.read_exact(&mut bytes)?;
        let header = Luks1Header::parse(&bytes)?;
        validate_cipher(
            &header.cipher_name,
            &header.cipher_mode,
            header.key_bytes as usize,
        )?;
        if key.len() != header.key_bytes as usize {
            return Err(LuksError::AuthenticationFailed);
        }
        let digest = derive_key_within(
            &header.hash_spec,
            key,
            &header.mk_digest_salt,
            header.mk_digest_iter,
            MK_DIGEST_LEN,
            DERIVATION_BUDGET,
        )?;
        if digest != header.mk_digest {
            return Err(LuksError::AuthenticationFailed);
        }
        payload1(reader, &header, key.to_vec())
    }

    /// Convert a UTF-16 password to UTF-8 without normalization or terminator
    /// stripping, matching the bytes used by cryptsetup. Invalid UTF-16 fails.
    pub fn unlock_with_utf16_password<R: Read + Seek>(
        reader: R,
        password: &[u16],
    ) -> Result<DecryptedPayload<R>> {
        let password =
            Zeroizing::new(
                String::from_utf16(password).map_err(|_| LuksError::Unsupported {
                    what: "password encoding",
                    value: "invalid UTF-16".into(),
                })?,
            );
        Self::unlock_with_passphrase(reader, password.as_bytes())
    }
}
