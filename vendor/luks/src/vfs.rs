//! `forensic-vfs` [`EncryptionLayer`] adapter for LUKS1 / LUKS2, behind the `vfs`
//! feature.
//!
//! Wraps an encrypted LUKS volume (a parent [`ImageSource`]) and, given a
//! passphrase, presents the **decrypted** payload as a [`DynSource`] a normal
//! filesystem mounts unchanged. The decryption is luks-core's own (audited
//! RustCrypto AES-XTS + PBKDF2/Argon2 key derivation); this module only wires the
//! contract.

use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, PoisonError};

use forensic_vfs::adapters::SourceCursor;
use forensic_vfs::{
    Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme, ImageSource,
    VfsError, VfsResult,
};

use crate::{DecryptedPayload, LuksError, LuksVolume};

/// LUKS header magic (`"LUKS"` + `0xBABE`).
const LUKS_MAGIC: &[u8; 6] = b"LUKS\xba\xbe";

/// A LUKS-encrypted volume presented as a [`EncryptionLayer`].
pub struct LuksLayer {
    encrypted: DynSource,
    len: u64,
    scheme: EncryptionScheme,
}

impl LuksLayer {
    /// Wrap an encrypted LUKS volume (the ciphertext byte source), peeking the
    /// on-disk version to report `Luks1` vs `Luks2`.
    pub fn new(encrypted: DynSource) -> Self {
        let len = encrypted.len();
        // Version = big-endian u16 at offset 6, after the 6-byte magic.
        let mut hdr = [0u8; 8];
        let scheme = match encrypted.read_at(0, &mut hdr) {
            Ok(n)
                if n >= 8
                    && hdr.starts_with(LUKS_MAGIC)
                    && u16::from_be_bytes([hdr[6], hdr[7]]) == 1 =>
            {
                EncryptionScheme::Luks1
            }
            _ => EncryptionScheme::Luks2,
        };
        Self {
            encrypted,
            len,
            scheme,
        }
    }
}

impl EncryptionLayer for LuksLayer {
    fn scheme(&self) -> EncryptionScheme {
        self.scheme
    }

    fn open(&self, creds: &dyn CredentialSource) -> VfsResult<DynSource> {
        let cands = creds.credentials_for(self.scheme, "");
        if cands.is_empty() {
            return Err(VfsError::NeedCredentials {
                scheme: "luks",
                target: String::new(),
            });
        }
        // A LUKS passphrase may arrive as a password or a keyfile's contents
        // (RecoveryKey); try each over a fresh Read+Seek view of the ciphertext.
        let mut last_err = None;
        for cred in &cands {
            let passphrase: &[u8] = match cred {
                Credential::Password(p) | Credential::RecoveryKey(p) => p.as_bytes(),
                Credential::KeyBytes(b) => b,
                _ => continue, // KeyFile / future variants: not wired here
            };
            let cursor = SourceCursor::new(Arc::clone(&self.encrypted), 0, self.len);
            match LuksVolume::unlock_with_passphrase(cursor, passphrase) {
                Ok(payload) => return Ok(Arc::new(LuksSource::new(payload))),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.as_ref().map_or(
            VfsError::NeedCredentials {
                scheme: "luks",
                target: String::new(),
            },
            map_luks_err,
        ))
    }
}

/// Translate a luks-core error into the VFS error type (a wrong passphrase / bad
/// header is a loud [`VfsError::Decode`]).
fn map_luks_err(e: &LuksError) -> VfsError {
    VfsError::Decode {
        layer: "luks",
        offset: 0,
        detail: e.to_string(),
        bytes: forensic_vfs::SmallHex::new(&[]),
    }
}

/// A decrypted LUKS payload presented as a read-only [`ImageSource`]. Reads
/// serialize through a poison-recovering `Mutex` (the reader advances a cursor).
struct LuksSource<R: Read + Seek> {
    inner: Mutex<DecryptedPayload<R>>,
    len: u64,
}

impl<R: Read + Seek> LuksSource<R> {
    fn new(payload: DecryptedPayload<R>) -> Self {
        let len = payload.payload_size();
        Self {
            inner: Mutex::new(payload),
            len,
        }
    }
}

impl<R: Read + Seek + Send> ImageSource for LuksSource<R> {
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
        guard.read_at(offset, dst).map_err(|e| map_luks_err(&e))?;
        Ok(want)
    }
}

#[cfg(test)]
mod tests {
    //! Always-on hermetic tests: build a valid LUKS1 aes-xts-plain64 container in
    //! memory (with luks-core's own audited primitives), wrap it in an in-memory
    //! [`ImageSource`], and drive every [`LuksLayer`] path — scheme detection,
    //! successful `open`, the credential-variant loop, and the error arms. These
    //! are Tier-3 self-consistency scaffolding; the `cryptsetup` proof is the
    //! Tier-1 oracle in `core/tests/oracle_vfs.rs` (env-gated on `LUKS_XTS_ORACLE`).

    use super::{LuksLayer, LuksSource};
    use crate::af;
    use crate::header::{
        KEYSLOT_LEN, KEY_DISABLED, KEY_ENABLED, LUKS1_PHDR_LEN, LUKS_MAGIC, LUKS_NUM_KEYS,
    };
    use crate::LuksVolume;
    use aes::cipher::KeyInit;
    use aes::Aes256;
    use forensic_vfs::{
        Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme, ImageSource,
        VfsError,
    };
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::Arc;
    use xts_mode::Xts128;

    const PASS: &[u8] = b"luks-TEST";

    struct FixedCreds(Vec<Credential>);
    impl CredentialSource for FixedCreds {
        fn credentials_for(&self, _scheme: EncryptionScheme, _target: &str) -> Vec<Credential> {
            self.0.clone()
        }
    }

    /// An in-memory [`ImageSource`] over a `Vec<u8>` — a byte-exact, panic-free
    /// stand-in for a file so the adapter can be exercised without touching disk.
    struct MemSource(Vec<u8>);
    impl ImageSource for MemSource {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> forensic_vfs::VfsResult<usize> {
            let start = (offset as usize).min(self.0.len());
            let end = start.saturating_add(buf.len()).min(self.0.len());
            let n = end - start;
            buf[..n].copy_from_slice(&self.0[start..end]);
            Ok(n)
        }
    }

    fn mem(bytes: Vec<u8>) -> DynSource {
        Arc::new(MemSource(bytes))
    }

    // ---- synthetic LUKS1 container (mirrors the volume.rs hermetic builder) ---

    const L1_KM_OFF: u32 = 2; // sector 2 = byte 1024 (past the 592-byte phdr)
    const L1_PAYLOAD_OFF: u32 = 4; // sector 4 = byte 2048
    const L1_STRIPES: u32 = 8;
    const L1_KEY_BYTES: u32 = 64;
    const L1_ITERS: u32 = 5;
    const L1_MK_ITER: u32 = 7;

    fn pbkdf2_sha256(pw: &[u8], salt: &[u8], iters: u32, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw, salt, iters, &mut out);
        out
    }

    fn xts_encrypt(key: &[u8], buf: &mut [u8], unit_size: usize, base: u128) {
        let step = (unit_size / 512).max(1) as u128;
        let (k1, k2) = key.split_at(32);
        let xts = Xts128::<Aes256>::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
        for (u, chunk) in buf.chunks_mut(unit_size).enumerate() {
            let tweak = (base + u as u128 * step).to_le_bytes();
            xts.encrypt_sector(chunk, tweak);
        }
    }

    /// A valid LUKS1 `aes`/`xts-plain64` (AES-256-XTS) image unlocking to `master`,
    /// carrying a 3-sector plaintext payload (data sector N = byte-pattern N).
    fn build_luks1(master: &[u8]) -> Vec<u8> {
        let salt = [0x11u8; 32];
        let mk_salt = [0x22u8; 32];
        let slot_key = pbkdf2_sha256(PASS, &salt, L1_ITERS, L1_KEY_BYTES as usize);

        let mut material =
            af::af_split::<sha2::Sha256>(master, L1_KEY_BYTES as usize, L1_STRIPES as usize, 0x5a);
        xts_encrypt(&slot_key, &mut material, 512, 0);

        let mk_digest = pbkdf2_sha256(master, &mk_salt, L1_MK_ITER, 20);

        let mut payload: Vec<u8> = (0..3u64)
            .flat_map(|s| std::iter::repeat_n(s as u8, 512))
            .collect();
        xts_encrypt(master, &mut payload, 512, 0);

        let payload_byte = L1_PAYLOAD_OFF as usize * 512;
        let mut img = vec![0u8; payload_byte + payload.len()];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&1u16.to_be_bytes());
        img[8..11].copy_from_slice(b"aes");
        img[40..51].copy_from_slice(b"xts-plain64");
        img[72..78].copy_from_slice(b"sha256");
        img[104..108].copy_from_slice(&L1_PAYLOAD_OFF.to_be_bytes());
        img[108..112].copy_from_slice(&L1_KEY_BYTES.to_be_bytes());
        img[112..132].copy_from_slice(&mk_digest);
        img[132..164].copy_from_slice(&mk_salt);
        img[164..168].copy_from_slice(&L1_MK_ITER.to_be_bytes());
        img[168..204].copy_from_slice(b"b22690e1-a392-4ecc-83b1-c1cf21200116");

        for i in 0..LUKS_NUM_KEYS {
            let base = 208 + i * KEYSLOT_LEN;
            if i == 0 {
                img[base..base + 4].copy_from_slice(&KEY_ENABLED.to_be_bytes());
                img[base + 4..base + 8].copy_from_slice(&L1_ITERS.to_be_bytes());
                img[base + 8..base + 40].copy_from_slice(&salt);
                img[base + 40..base + 44].copy_from_slice(&L1_KM_OFF.to_be_bytes());
                img[base + 44..base + 48].copy_from_slice(&L1_STRIPES.to_be_bytes());
            } else {
                img[base..base + 4].copy_from_slice(&KEY_DISABLED.to_be_bytes());
            }
        }

        let km_byte = L1_KM_OFF as usize * 512;
        img[km_byte..km_byte + material.len()].copy_from_slice(&material);
        img[payload_byte..payload_byte + payload.len()].copy_from_slice(&payload);
        assert!(LUKS1_PHDR_LEN <= km_byte);
        img
    }

    fn master() -> Vec<u8> {
        (0..64u8)
            .map(|x| x.wrapping_mul(7).wrapping_add(3))
            .collect()
    }

    // ---- scheme detection --------------------------------------------------

    #[test]
    fn detects_luks1_scheme() {
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        assert_eq!(layer.scheme(), EncryptionScheme::Luks1);
    }

    #[test]
    fn non_luks1_header_reports_luks2() {
        // Anything that is not a well-formed LUKS1 magic+version-1 header is
        // reported as Luks2 (the on-disk version byte drives the branch at line 46).
        let layer = LuksLayer::new(mem(vec![0u8; 8]));
        assert_eq!(layer.scheme(), EncryptionScheme::Luks2);

        // A LUKS magic with version 2 is Luks2 too.
        let mut hdr = vec![0u8; 4096];
        hdr[0..6].copy_from_slice(&LUKS_MAGIC);
        hdr[6..8].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(LuksLayer::new(mem(hdr)).scheme(), EncryptionScheme::Luks2);
    }

    // ---- open(): success over each wired credential variant ----------------

    fn assert_decrypts(layer: &LuksLayer, creds: &FixedCreds) {
        let dec: DynSource = layer.open(creds).expect("unlock");
        // Payload sector N is byte-pattern N; matching several proves the XTS
        // per-sector tweak and payload-offset alignment.
        for sec in 0u64..3 {
            let mut buf = [0u8; 512];
            assert_eq!(dec.read_at(sec * 512, &mut buf).expect("read"), 512);
            assert!(buf.iter().all(|&b| b == sec as u8), "sector {sec}");
        }
    }

    #[test]
    fn open_succeeds_with_password() {
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        assert_decrypts(
            &layer,
            &FixedCreds(vec![Credential::Password("luks-TEST".to_string())]),
        );
    }

    #[test]
    fn open_succeeds_with_recovery_key_and_key_bytes() {
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        // RecoveryKey carries the passphrase as text.
        assert_decrypts(
            &layer,
            &FixedCreds(vec![Credential::RecoveryKey("luks-TEST".to_string())]),
        );
        // KeyBytes carries it as raw bytes.
        assert_decrypts(
            &layer,
            &FixedCreds(vec![Credential::KeyBytes(PASS.to_vec())]),
        );
    }

    #[test]
    fn open_skips_unwired_variant_then_succeeds() {
        // A KeyFile credential is not wired here — it is skipped, and a following
        // Password still unlocks (the `_ => continue` arm plus a later success).
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        assert_decrypts(
            &layer,
            &FixedCreds(vec![
                Credential::KeyFile(PathBuf::from("/nonexistent.key")),
                Credential::Password("luks-TEST".to_string()),
            ]),
        );
    }

    // ---- open(): error arms ------------------------------------------------

    /// The `Ok` arm of `open` (a `DynSource`) is not `Debug`, so match rather than
    /// `unwrap_err()`; return the error for the caller to assert on.
    fn open_err(layer: &LuksLayer, creds: &FixedCreds) -> VfsError {
        match layer.open(creds) {
            Ok(_) => panic!("expected open to fail"), // cov:unreachable: callers pass only failing creds
            Err(e) => e,
        }
    }

    #[test]
    fn open_with_no_credentials_needs_credentials() {
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        let err = open_err(&layer, &FixedCreds(vec![]));
        assert!(matches!(
            err,
            VfsError::NeedCredentials { scheme: "luks", .. }
        ));
    }

    #[test]
    fn open_only_unwired_credentials_needs_credentials() {
        // Every candidate is an unwired variant → the loop makes no attempt, so
        // `last_err` is None and the fallback is NeedCredentials, not Decode.
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        let err = open_err(
            &layer,
            &FixedCreds(vec![Credential::KeyFile(PathBuf::from("x"))]),
        );
        assert!(matches!(
            err,
            VfsError::NeedCredentials { scheme: "luks", .. }
        ));
    }

    #[test]
    fn open_wrong_passphrase_is_loud_decode() {
        // A wired-but-wrong passphrase sets `last_err`, surfaced as a loud Decode
        // via `map_luks_err` (never a silent guess or panic).
        let layer = LuksLayer::new(mem(build_luks1(&master())));
        let err = open_err(
            &layer,
            &FixedCreds(vec![Credential::Password("wrong".to_string())]),
        );
        match err {
            VfsError::Decode { layer, .. } => assert_eq!(layer, "luks"),
            other => panic!("expected Decode, got {other:?}"), // cov:unreachable: a wrong passphrase always maps to Decode
        }
    }

    // ---- LuksSource: len / read_at boundaries ------------------------------

    #[test]
    fn luks_source_len_and_read_bounds() {
        let payload =
            LuksVolume::unlock1_with_passphrase(Cursor::new(build_luks1(&master())), PASS)
                .expect("unlock");
        let expected_len = payload.payload_size();
        let src: DynSource = Arc::new(LuksSource::new(payload));
        assert_eq!(src.len(), expected_len);
        assert!(!src.is_empty());

        // A read that straddles EOF returns only the available prefix.
        let mut buf = vec![0u8; 512];
        let got = src
            .read_at(expected_len - 256, &mut buf)
            .expect("read tail");
        assert_eq!(got as u64, 256);

        // A read starting at/after EOF returns 0.
        let mut z = [0u8; 16];
        assert_eq!(src.read_at(expected_len, &mut z).expect("eof read"), 0);
    }
}
