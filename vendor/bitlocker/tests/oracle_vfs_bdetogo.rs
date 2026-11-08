//! Tier-1 oracle for the `forensic-vfs` [`BitlockerLayer`] adapter: wrap the
//! real dfvfs `bdetogo.raw` image as a `DynSource`, unlock it through the
//! `EncryptionLayer` contract with the published password, and confirm the decrypted
//! boot sector is the original FAT (OEM name `mkdosfs`), matching `pybde`.
//!
//! Env-gated on `BDE_ORACLE_IMAGE` — skips cleanly when the image is absent (it
//! is 64 MiB and not committed). The hermetic in-crate tests in
//! `core/src/vfs.rs` are the committed-fixture coverage gate; this is the Tier-1
//! confirmation against the real image. Provenance: `tests/data/README.md`.
//!
//! ```bash
//! BDE_ORACLE_IMAGE=/path/to/bdetogo.raw \
//!   cargo test -p bitlocker-core --features vfs --test oracle_vfs_bdetogo -- --nocapture
//! ```
#![cfg(feature = "vfs")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use bitlocker::vfs::BitlockerLayer;
use forensic_vfs::adapters::FileSource;
use forensic_vfs::{Credential, CredentialSource, DynSource, EncryptionLayer, EncryptionScheme};

struct FixedCreds(Vec<Credential>);
impl CredentialSource for FixedCreds {
    fn credentials_for(&self, _scheme: EncryptionScheme, _target: &str) -> Vec<Credential> {
        self.0.clone()
    }
}

#[test]
fn tier1_bitlocker_cryptolayer_decrypts_bdetogo() {
    let Ok(path) = std::env::var("BDE_ORACLE_IMAGE") else {
        eprintln!("BDE_ORACLE_IMAGE unset — skipping Tier-1 vfs oracle (see tests/data/README.md)");
        return;
    };

    let src: DynSource = Arc::new(FileSource::open(&path).expect("open BDE_ORACLE_IMAGE"));
    let layer = BitlockerLayer::new(src);
    assert_eq!(layer.scheme(), EncryptionScheme::Bitlocker);

    let creds = FixedCreds(vec![Credential::Password("bde-TEST".to_string())]);
    let dec: DynSource = layer.open(&creds).expect("unlock bdetogo.raw");

    let mut boot = [0u8; 512];
    assert_eq!(dec.read_at(0, &mut boot).expect("read decrypted boot"), 512);
    // The decrypted first sector is the ORIGINAL FAT boot sector (OEM name
    // "mkdosfs"), proving the volume-header relocation was followed and the
    // sector decrypted, not read raw.
    assert_eq!(&boot[3..11], b"mkdosfs\0", "decrypted volume is a FAT");
    assert_eq!(&boot[510..512], &[0x55, 0xAA]);

    // No credentials offered → a loud error, never a guess or panic.
    let empty = FixedCreds(vec![]);
    assert!(layer.open(&empty).is_err());
}
