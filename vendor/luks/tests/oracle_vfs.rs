//! Tier-1 oracle: drive the `forensic-vfs` [`LuksLayer`] `EncryptionLayer` over a
//! real `cryptsetup`-minted LUKS1 aes-xts-plain64 container and confirm the
//! decrypted sectors match `cryptsetup` byte-for-byte (SHA-256).
//!
//! The image is not committed (env `LUKS_XTS_ORACLE`, default
//! `/tmp/luks1_xts_oracle.img`); the test skips cleanly when it is absent, so the
//! committed always-on hermetic tests in `core/src/vfs.rs` remain the coverage
//! gate. Provenance + ground truth: `tests/data/README.md`.
//!
//! ```bash
//! LUKS_XTS_ORACLE=/tmp/luks1_xts_oracle.img \
//!   cargo test -p luks-core --features vfs --test oracle_vfs -- --nocapture
//! ```
#![cfg(feature = "vfs")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use forensic_vfs::adapters::FileSource;
use forensic_vfs::{Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme};
use luks::vfs::LuksLayer;
use sha2::{Digest, Sha256};

struct FixedCreds(Vec<Credential>);
impl CredentialSource for FixedCreds {
    fn credentials_for(&self, _scheme: EncryptionScheme, _target: &str) -> Vec<Credential> {
        self.0.clone()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn encrypted() -> Option<DynSource> {
    let path = std::env::var("LUKS_XTS_ORACLE")
        .unwrap_or_else(|_| "/tmp/luks1_xts_oracle.img".to_string());
    let src = FileSource::open(&path).ok()?;
    Some(Arc::new(src))
}

#[test]
fn luks_cryptolayer_decrypts_aes_xts() {
    let Some(enc) = encrypted() else {
        eprintln!("skip: no LUKS image (set LUKS_XTS_ORACLE)");
        return;
    };
    let layer = LuksLayer::new(enc);
    assert_eq!(layer.scheme(), EncryptionScheme::Luks1);

    let creds = FixedCreds(vec![Credential::Password("luks-TEST".to_string())]);
    let dec: DynSource = layer.open(&creds).expect("unlock luks aes-xts");

    // cryptsetup oracle: distinct per-sector plaintext (data sector N = byte N).
    // Matching sectors 0/1/8/15 proves correct XTS per-sector tweak AND the
    // payload-offset alignment (identical plaintext would not).
    for (sec, expected) in [
        (
            0u64,
            "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
        ),
        (
            1,
            "6caf38d537984e261527b8caef5f990fb91415a1db917198821a79ed28997973",
        ),
        (
            8,
            "7debd4d73a98c0df9eb7b083fd21033d7bd0907b3947f22338d8c82154face23",
        ),
        (
            15,
            "941657fde04ff270f8ae019ede5287c71d887758641536ab0eb87a0d434526bd",
        ),
    ] {
        let mut buf = [0u8; 512];
        assert_eq!(
            dec.read_at(sec * 512, &mut buf).expect("read decrypted"),
            512
        );
        assert_eq!(
            sha256_hex(&buf),
            expected,
            "sector {sec} vs cryptsetup oracle"
        );
    }

    // No credentials offered → NeedCredentials, never a guess or panic.
    assert!(layer.open(&FixedCreds(vec![])).is_err());
}
