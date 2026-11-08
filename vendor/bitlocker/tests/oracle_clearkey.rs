//! Tier-2 oracle: unlock a **clear-key** BitLocker volume with **no credential**
//! and confirm the decrypted sectors match `pybde` byte-for-byte (SHA-256).
//!
//! A clear-key protector (type 0x0000) stores the VMK unprotected — BitLocker
//! adds it when protection is *suspended* — so `unlock_clear_key` recovers the
//! plaintext with no password, recovery key, or startup key.
//!
//! Tier-2: the volume is self-minted (`manage-bde -protectors -disable`), but the
//! ground truth is an *independent* `pybde` oracle (not a fixture we authored) —
//! `pybde` opens the same image with no credential and returns identical sectors.
//! The image is not committed (256 MiB), so the test is env-gated on
//! `BDE_CLEARKEY_ORACLE` (the path to `clearkey.raw`) and skips cleanly when
//! absent. Provenance: `tests/data/README.md`.
//!
//! ```bash
//! BDE_CLEARKEY_ORACLE=/tmp/bde-clearkey-oracle/clearkey.raw \
//!   cargo test -p bitlocker-core --test oracle_clearkey -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use bitlocker::BitLockerVolume;
use common::{sha256_hex, OffsetReader};

/// The BitLocker volume sits at byte 65536 (LBA 128) in `clearkey.raw`.
const VOLUME_OFFSET: u64 = 65_536;

#[test]
fn tier2_clearkey_no_credential_matches_pybde() {
    let Ok(path) = std::env::var("BDE_CLEARKEY_ORACLE") else {
        eprintln!("BDE_CLEARKEY_ORACLE unset — skipping Tier-2 clear-key oracle");
        return;
    };
    let file = File::open(&path).expect("open clearkey.raw");
    let reader = OffsetReader::new(file, VOLUME_OFFSET).expect("window volume");

    // No credential — the clear-key protector alone unlocks the volume.
    let mut vol = BitLockerVolume::unlock_clear_key(reader).expect("unlock clearkey.raw");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8004,
        "oracle must be XTS-AES-128"
    );

    // (volume byte offset, expected 512-byte-sector SHA-256) — independent pybde
    // ground truth with NO credential (see tests/data/README.md). Offset 0 is the
    // relocated boot region; 16/32 MiB pin the XTS tweak to the sector number.
    let cases: [(u64, &str); 3] = [
        (
            0,
            "2ec4443a018d15665ab5168b1de633f4f7b368dc1e7492521e90d8238cf96224",
        ),
        (
            16_777_216,
            "ba92072a9e2b2578939d4df43a6dfd64564e546e10e57baf263a1563e784f9a7",
        ),
        (
            33_554_432,
            "285b610ce6bb4ebdbc11d35ee232078d183dbf05941004253a58fe86756f6235",
        ),
    ];

    for (offset, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(offset, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("offset {offset}: {got}");
        assert_eq!(got, want, "decrypted offset {offset} does not match pybde");
    }
}
