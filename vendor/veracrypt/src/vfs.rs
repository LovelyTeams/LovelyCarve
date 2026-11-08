//! `forensic-vfs` [`EncryptionLayer`] adapter for VeraCrypt / TrueCrypt, behind the
//! `vfs` feature.
//!
//! Wraps an encrypted VeraCrypt volume (a parent [`ImageSource`]) and, given a
//! password, presents the **decrypted** data area as a [`DynSource`] a normal
//! filesystem mounts unchanged. The decryption is veracrypt-core's own (audited
//! RustCrypto XTS, optional AES/Serpent/Twofish cascade); this module only wires
//! the contract.

use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, PoisonError};

use forensic_vfs::adapters::SourceCursor;
use forensic_vfs::{
    Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme, ImageSource,
    VfsError, VfsResult,
};

use crate::{DecryptedVolume, VeraError, VeraVolume};

/// A VeraCrypt-encrypted volume presented as a [`EncryptionLayer`].
pub struct VeraCryptLayer {
    encrypted: DynSource,
    len: u64,
}

impl VeraCryptLayer {
    /// Wrap an encrypted VeraCrypt/TrueCrypt volume (the ciphertext byte source).
    pub fn new(encrypted: DynSource) -> Self {
        let len = encrypted.len();
        Self { encrypted, len }
    }
}

impl EncryptionLayer for VeraCryptLayer {
    fn scheme(&self) -> EncryptionScheme {
        EncryptionScheme::VeraCrypt
    }

    fn open(&self, creds: &dyn CredentialSource) -> VfsResult<DynSource> {
        let cands = creds.credentials_for(EncryptionScheme::VeraCrypt, "");
        if cands.is_empty() {
            return Err(VfsError::NeedCredentials {
                scheme: "veracrypt",
                target: String::new(),
            });
        }
        // VeraCrypt is unlocked by a volume password; try each offered one over a
        // fresh Read+Seek view of the ciphertext (unlock consumes the reader).
        let mut last_err = None;
        for cred in &cands {
            let Credential::Password(p) = cred else {
                continue; // only a password protector is wired here
            };
            let cursor = SourceCursor::new(Arc::clone(&self.encrypted), 0, self.len);
            match VeraVolume::unlock_with_password(cursor, p.as_bytes()) {
                Ok(vol) => return Ok(Arc::new(VeraCryptSource::new(vol))),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.as_ref().map_or(
            VfsError::NeedCredentials {
                scheme: "veracrypt",
                target: String::new(),
            },
            map_vera_err,
        ))
    }
}

/// Translate a veracrypt-core error into the VFS error type (a wrong password /
/// bad header is a loud [`VfsError::Decode`]).
fn map_vera_err(e: &VeraError) -> VfsError {
    VfsError::Decode {
        layer: "veracrypt",
        offset: 0,
        detail: e.to_string(),
        bytes: forensic_vfs::SmallHex::new(&[]),
    }
}

/// A decrypted VeraCrypt volume presented as a read-only [`ImageSource`]. Reads
/// serialize through a poison-recovering `Mutex` (the reader advances a cursor).
struct VeraCryptSource<R: Read + Seek> {
    inner: Mutex<DecryptedVolume<R>>,
    len: u64,
}

impl<R: Read + Seek> VeraCryptSource<R> {
    fn new(vol: DecryptedVolume<R>) -> Self {
        let len = vol.data_size();
        Self {
            inner: Mutex::new(vol),
            len,
        }
    }
}

impl<R: Read + Seek + Send> ImageSource for VeraCryptSource<R> {
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
        guard.read_at(offset, dst).map_err(|e| map_vera_err(&e))?;
        Ok(want)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::VeraCryptLayer;
    use forensic_vfs::adapters::FileSource;
    use forensic_vfs::{
        Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme,
    };
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    struct FixedCreds(Vec<Credential>);
    impl CredentialSource for FixedCreds {
        fn credentials_for(&self, _scheme: EncryptionScheme, _target: &str) -> Vec<Credential> {
            self.0.clone()
        }
    }

    /// The real AES-Twofish cascade VeraCrypt container `vccasc.vc` (password
    /// `aaaaaaaaaaaa`), staged at /tmp (env `VC_CASCADE_ORACLE`, default path).
    /// Ground truth from the veracrypt binary + `cryptsetup --veracrypt`:
    /// decrypted data sector 0 has the SHA-256 below. Skips if absent.
    fn encrypted() -> Option<DynSource> {
        let path = std::env::var("VC_CASCADE_ORACLE")
            .unwrap_or_else(|_| "/tmp/vc-oracle/vccasc.vc".to_string());
        let src = FileSource::open(&path).ok()?;
        Some(Arc::new(src))
    }

    fn sha256_hex(data: &[u8]) -> String {
        use std::fmt::Write;
        Sha256::digest(data).iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    #[test]
    fn veracrypt_encryptionlayer_decrypts_cascade() {
        let Some(enc) = encrypted() else {
            eprintln!("skip: no VeraCrypt image (set VC_CASCADE_ORACLE)");
            return;
        };
        let layer = VeraCryptLayer::new(enc);
        assert_eq!(layer.scheme(), EncryptionScheme::VeraCrypt);

        let creds = FixedCreds(vec![Credential::Password("aaaaaaaaaaaa".to_string())]);
        let dec: DynSource = layer.open(&creds).expect("unlock vccasc.vc");

        // Decrypted data sector 0 — veracrypt/cryptsetup oracle SHA-256.
        let mut sector = [0u8; 512];
        assert_eq!(dec.read_at(0, &mut sector).expect("read decrypted"), 512);
        assert_eq!(
            sha256_hex(&sector),
            "da09622b78baeeb1fa8e6532f1eb23afc733a8449097d3a08d612286d4161492"
        );

        // No credentials offered → NeedCredentials, never a guess or panic.
        assert!(layer.open(&FixedCreds(vec![])).is_err());
    }
}
