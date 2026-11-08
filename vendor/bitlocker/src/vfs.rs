//! `forensic-vfs` [`EncryptionLayer`] adapter for BitLocker, behind the `vfs` feature.
//!
//! Wraps an encrypted BitLocker volume (a parent [`ImageSource`]) and, given a
//! credential, presents the **decrypted** volume as a [`DynSource`] a normal
//! filesystem mounts unchanged. The decryption is bitlocker-core's own (audited
//! RustCrypto ciphers + the `elephant-diffuser` crate); this module only wires
//! the contract: pull a credential from the [`CredentialSource`], call the
//! matching `unlock_*`, and expose the result as a positioned-read source.

use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, PoisonError};

use forensic_vfs::adapters::SourceCursor;
use forensic_vfs::{
    Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme, ImageSource,
    VfsError, VfsResult,
};

use crate::{BitLockerVolume, DecryptedVolume};

/// A BitLocker-encrypted volume presented as a [`EncryptionLayer`].
pub struct BitlockerLayer {
    encrypted: DynSource,
    len: u64,
}

impl BitlockerLayer {
    /// Wrap an encrypted BitLocker volume (the ciphertext byte source).
    pub fn new(encrypted: DynSource) -> Self {
        let len = encrypted.len();
        Self { encrypted, len }
    }
}

/// Translate a bitlocker-core error into the VFS error type: a bad key / bad
/// header is a loud [`VfsError::Decode`].
fn map_bde_err(e: &crate::BdeError) -> VfsError {
    VfsError::Decode {
        layer: "bitlocker",
        offset: 0,
        detail: e.to_string(),
        bytes: forensic_vfs::SmallHex::new(&[]),
    }
}

impl EncryptionLayer for BitlockerLayer {
    fn scheme(&self) -> EncryptionScheme {
        EncryptionScheme::Bitlocker
    }

    fn open(&self, creds: &dyn CredentialSource) -> VfsResult<DynSource> {
        let cands = creds.credentials_for(EncryptionScheme::Bitlocker, "");
        if cands.is_empty() {
            return Err(VfsError::NeedCredentials {
                scheme: "bitlocker",
                target: String::new(),
            });
        }
        // Try each offered credential; a fresh Read+Seek view of the ciphertext
        // per attempt (unlock consumes the reader).
        let mut last_err = None;
        for cred in &cands {
            let cursor = SourceCursor::new(Arc::clone(&self.encrypted), 0, self.len);
            let attempt = match cred {
                Credential::Password(p) => BitLockerVolume::unlock_with_password(cursor, p),
                Credential::RecoveryKey(rk) => {
                    BitLockerVolume::unlock_with_recovery_password(cursor, rk)
                }
                // KeyBytes / KeyFile are not a BitLocker protector this layer wires.
                _ => continue,
            };
            match attempt {
                Ok(vol) => return Ok(Arc::new(BitlockerSource::new(vol))),
                Err(e) => last_err = Some(e),
            }
        }
        // Credentials were offered but none unlocked → a loud bad-key Decode,
        // never a silent empty or a guess.
        Err(last_err.as_ref().map_or(
            VfsError::NeedCredentials {
                scheme: "bitlocker",
                target: String::new(),
            },
            map_bde_err,
        ))
    }
}

/// A decrypted BitLocker volume presented as a read-only [`ImageSource`]. Reads
/// serialize through a poison-recovering `Mutex` (bitlocker-core's `read_at`
/// advances an internal cursor).
struct BitlockerSource<R: Read + Seek> {
    inner: Mutex<DecryptedVolume<R>>,
    len: u64,
}

impl<R: Read + Seek> BitlockerSource<R> {
    fn new(vol: DecryptedVolume<R>) -> Self {
        let len = vol.volume_size();
        Self {
            inner: Mutex::new(vol),
            len,
        }
    }
}

impl<R: Read + Seek + Send> ImageSource for BitlockerSource<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let avail = self.len.saturating_sub(offset);
        if avail == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(avail) as usize;
        let Some(dst) = buf.get_mut(..want) else {
            return Ok(0); // cov:unreachable: want <= buf.len() by the min above
        };
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard.read_at(offset, dst).map_err(|e| map_bde_err(&e))?;
        Ok(want)
    }
}

#[cfg(test)]
mod tests {
    use super::BitlockerLayer;
    use crate::crypto::{aes_ccm_wrap, password_hash, stretch_key, SectorCipher};
    use crate::metadata::{
        ENTRY_TYPE_FVEK, ENTRY_TYPE_VMK, ENTRY_TYPE_VOLUME_HEADER, PROTECTION_PASSWORD,
        VALUE_TYPE_AES_CCM, VALUE_TYPE_STRETCH, VALUE_TYPE_VMK,
    };
    use forensic_vfs::adapters::SeekPoolSource;
    use forensic_vfs::{
        Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme, VfsError,
    };
    use std::io::Cursor;
    use std::sync::Arc;

    struct FixedCreds(Vec<Credential>);
    impl CredentialSource for FixedCreds {
        fn credentials_for(&self, _scheme: EncryptionScheme, _target: &str) -> Vec<Credential> {
            self.0.clone()
        }
    }

    const RELOCATED_OFFSET: u64 = 0x4000;
    const META_BLOCK_OFFSET: u64 = 0x1000;
    const IMAGE_SIZE: usize = 0x5000;
    const ENCRYPTED_SIZE: u64 = 0x4800;

    fn entry(entry_type: u16, value_type: u16, data: &[u8]) -> Vec<u8> {
        let size = (8 + data.len()) as u16;
        let mut v = Vec::new();
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&entry_type.to_le_bytes());
        v.extend_from_slice(&value_type.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    /// Build a minimal synthetic BitLocker To Go volume (method 0x8000, password
    /// protector `pw`) as an in-memory byte source, plus the plaintext of its
    /// relocated first sector. Self-consistency (Tier-3) only — the Tier-1 pybde
    /// proof is `bitlocker_cryptolayer_decrypts_bdetogo` on the real image; this
    /// hermetic image drives the adapter's control flow with no external file.
    fn build_encrypted(pw: &str) -> (DynSource, [u8; 512]) {
        let salt = [0x33u8; 16];
        let fvek = [0x11u8; 16];
        let tweak = [0x22u8; 16];
        let vmk = [0x44u8; 32];

        let mut vmk_container = vec![0u8; 44];
        vmk_container[12..44].copy_from_slice(&vmk);
        let stretched = stretch_key(&password_hash(pw), &salt);
        let vmk_ccm = aes_ccm_wrap(&stretched, &[0x55; 12], &vmk_container);

        let mut fvek_container = vec![0u8; 76];
        fvek_container[12..28].copy_from_slice(&fvek);
        fvek_container[44..60].copy_from_slice(&tweak);
        let fvek_ccm = aes_ccm_wrap(&vmk, &[0x66; 12], &fvek_container);

        let mut stretch_data = vec![0u8; 4];
        stretch_data.extend_from_slice(&salt);
        let stretch_entry = entry(0, VALUE_TYPE_STRETCH, &stretch_data);
        let vmk_ccm_entry = entry(0, VALUE_TYPE_AES_CCM, &vmk_ccm);

        let mut vmk_data = vec![0u8; 28];
        vmk_data[26..28].copy_from_slice(&PROTECTION_PASSWORD.to_le_bytes());
        vmk_data.extend_from_slice(&stretch_entry);
        vmk_data.extend_from_slice(&vmk_ccm_entry);
        let vmk_entry = entry(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, &vmk_data);

        let fvek_entry = entry(ENTRY_TYPE_FVEK, VALUE_TYPE_AES_CCM, &fvek_ccm);

        let mut vh_data = Vec::new();
        vh_data.extend_from_slice(&RELOCATED_OFFSET.to_le_bytes());
        vh_data.extend_from_slice(&512u64.to_le_bytes());
        let vh_entry = entry(ENTRY_TYPE_VOLUME_HEADER, ENTRY_TYPE_VOLUME_HEADER, &vh_data);

        let mut entries = Vec::new();
        entries.extend_from_slice(&vh_entry);
        entries.extend_from_slice(&vmk_entry);
        entries.extend_from_slice(&fvek_entry);
        let metadata_size = 48 + entries.len();

        let mut image = vec![0u8; IMAGE_SIZE];
        image[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        image[3..11].copy_from_slice(b"MSWIN4.1");
        image[12] = 0x02;
        image[440..448].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());

        let mb = META_BLOCK_OFFSET as usize;
        image[mb..mb + 8].copy_from_slice(b"-FVE-FS-");
        image[mb + 10..mb + 12].copy_from_slice(&2u16.to_le_bytes());
        image[mb + 16..mb + 24].copy_from_slice(&ENCRYPTED_SIZE.to_le_bytes());
        image[mb + 28..mb + 32].copy_from_slice(&1u32.to_le_bytes());
        image[mb + 32..mb + 40].copy_from_slice(&META_BLOCK_OFFSET.to_le_bytes());
        image[mb + 56..mb + 64].copy_from_slice(&RELOCATED_OFFSET.to_le_bytes());
        image[mb + 64..mb + 68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
        image[mb + 64 + 36..mb + 64 + 38].copy_from_slice(&0x8000u16.to_le_bytes());
        image[mb + 64 + 48..mb + 64 + 48 + entries.len()].copy_from_slice(&entries);

        let mut plain = [0u8; 512];
        for (i, b) in plain.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x5a;
        }
        let cipher = SectorCipher::new(fvek, tweak);
        let ct = cipher.encrypt_sector(&plain, RELOCATED_OFFSET);
        let ro = RELOCATED_OFFSET as usize;
        image[ro..ro + 512].copy_from_slice(&ct);

        let len = image.len() as u64;
        let src: DynSource = Arc::new(SeekPoolSource::single(Cursor::new(image), len));
        (src, plain)
    }

    /// The synthetic image round-trips through the adapter: `open` decrypts with
    /// the password, and the resulting `DynSource` reads the plaintext sector
    /// back — exercising `open`'s happy path, `BitlockerSource::{new,len,read_at}`.
    #[test]
    fn cryptolayer_decrypts_synthetic_and_reads_back() {
        let (enc, plain) = build_encrypted("test-pw");
        let layer = BitlockerLayer::new(enc);
        assert_eq!(layer.scheme(), EncryptionScheme::Bitlocker);

        let creds = FixedCreds(vec![Credential::Password("test-pw".to_string())]);
        let dec: DynSource = layer.open(&creds).expect("unlock synthetic volume");

        // len() reports the decrypted volume size.
        assert_eq!(dec.len(), IMAGE_SIZE as u64);

        let mut boot = [0u8; 512];
        assert_eq!(
            dec.read_at(0, &mut boot).expect("read plaintext sector"),
            512
        );
        assert_eq!(boot, plain);

        // read_at at the exact end returns 0 (avail == 0 early return).
        assert_eq!(dec.read_at(IMAGE_SIZE as u64, &mut boot).unwrap(), 0);

        // A buffer larger than the remaining bytes is clamped to `want`.
        let mut tail = [0u8; 1024];
        let got = dec.read_at(IMAGE_SIZE as u64 - 512, &mut tail).unwrap();
        assert_eq!(got, 512);
    }

    /// No credentials offered → a loud `NeedCredentials`, never a guess or panic.
    #[test]
    fn cryptolayer_no_credentials_needs_credentials() {
        let (enc, _) = build_encrypted("test-pw");
        let layer = BitlockerLayer::new(enc);
        let empty = FixedCreds(vec![]);
        assert!(matches!(
            layer.open(&empty),
            Err(VfsError::NeedCredentials { scheme, .. }) if scheme == "bitlocker"
        ));
    }

    /// A wrong password unlocks nothing → the bad-key path surfaces as a loud
    /// `Decode` (exercises `map_bde_err` and the `last_err` branch).
    #[test]
    fn cryptolayer_wrong_password_is_loud_decode() {
        let (enc, _) = build_encrypted("test-pw");
        let layer = BitlockerLayer::new(enc);
        let creds = FixedCreds(vec![Credential::Password("wrong-pw".to_string())]);
        assert!(matches!(
            layer.open(&creds),
            Err(VfsError::Decode { layer, .. }) if layer == "bitlocker"
        ));
    }

    /// A recovery-key credential drives the `unlock_with_recovery_password` arm;
    /// on this password-only volume it fails, still a loud `Decode` (never a
    /// silent empty). Exercises the `RecoveryKey` branch and its error path.
    #[test]
    fn cryptolayer_recovery_key_branch() {
        let (enc, _) = build_encrypted("test-pw");
        let layer = BitlockerLayer::new(enc);
        let creds = FixedCreds(vec![Credential::RecoveryKey(
            "000000-000000-000000-000000-000000-000000-000000-000000".to_string(),
        )]);
        assert!(matches!(layer.open(&creds), Err(VfsError::Decode { .. })));
    }

    /// A credential this layer does not wire (raw key bytes) hits the `_ => continue`
    /// arm; with no unlocking credential offered the result is `NeedCredentials`
    /// (no attempt was made, so `last_err` is `None`).
    #[test]
    fn cryptolayer_unwired_credential_continues() {
        let (enc, _) = build_encrypted("test-pw");
        let layer = BitlockerLayer::new(enc);
        let creds = FixedCreds(vec![Credential::KeyBytes(vec![0u8; 32])]);
        assert!(matches!(
            layer.open(&creds),
            Err(VfsError::NeedCredentials { .. })
        ));
    }
}
