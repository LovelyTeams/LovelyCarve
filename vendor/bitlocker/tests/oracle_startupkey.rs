//! Tier-2 oracle: unlock the self-minted `sk8004` BitLocker volume (method
//! `0x8004`, XTS-AES-128) with its **startup-key `.BEK` file** and confirm the
//! decrypted sectors match `pybde` byte-for-byte (SHA-256).
//!
//! Tier-2: we minted the image on a Windows VM (`manage-bde -protectors -add
//! -StartupKey`), but the ground truth is derived independently by `pybde`
//! (`read_startup_key`) — and cross-checked against the recovery password (the
//! `.BEK`-decrypted plaintext equals the recovery-decrypted plaintext). The
//! image is not committed (132 MiB) so the test is env-gated on
//! `BDE_STARTUPKEY_ORACLE` (path to `sk8004.raw`) and skips cleanly when absent.
//! The `.BEK` is located next to it (`*.BEK` in the same directory).
//! Provenance: `tests/data/README.md`.
//!
//! ```bash
//! BDE_STARTUPKEY_ORACLE=/tmp/bde-startupkey-oracle/sk8004.raw \
//!   cargo test -p bitlocker-core --test oracle_startupkey -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;
use std::path::Path;

use bitlocker::BitLockerVolume;
use common::{sha256_hex, OffsetReader};

/// The BitLocker volume sits at byte 65536 (sector 128) in `sk8004.raw`.
const VOLUME_OFFSET: u64 = 65_536;

#[test]
fn tier2_startupkey_xts128_matches_pybde() {
    let Ok(path) = std::env::var("BDE_STARTUPKEY_ORACLE") else {
        eprintln!("BDE_STARTUPKEY_ORACLE unset — skipping Tier-2 startup-key oracle");
        return;
    };

    // Locate the .BEK next to the raw image.
    let dir = Path::new(&path).parent().expect("raw path has a parent");
    let bek_path = std::fs::read_dir(dir)
        .expect("read oracle dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bek")))
        .expect("a .BEK file next to sk8004.raw");
    let bek = std::fs::read(&bek_path).expect("read .BEK");

    let file = File::open(&path).expect("open sk8004.raw");
    let reader = OffsetReader::new(file, VOLUME_OFFSET).expect("window volume");

    let mut vol = BitLockerVolume::unlock_with_startup_key(reader, &bek).expect("unlock via .BEK");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8004,
        "oracle must be XTS-AES-128"
    );

    // (sector LBA, expected 512-byte-sector SHA-256) — pybde ground truth via
    // read_startup_key (see tests/data/README.md and the oracle GROUND-TRUTH).
    let cases: [(u64, &str); 6] = [
        (
            0,
            "343e0202d4c45a9c7d4f753f7b0f9bf5c19492f634bf12ded6d81957e367f53e",
        ),
        (
            1,
            "ef6d6118087d7849e66c32de9859dc5a74b98aa2d28b3f2e6e87275b537eb546",
        ),
        (
            2,
            "e845941331aaf16324636abd0c499908757d12eaf947c841e02164c4b9e1edad",
        ),
        (
            16,
            "94f1d42602ae60a6bc0af6af26b623181f1530d8bea27b803d67d50a0fb65f67",
        ),
        (
            100,
            "a23c6ada5be04b4f1ffcc694843fb07f549f72bd9cd61b418be86549a7397c9b",
        ),
        (
            200,
            "ad9b47e60ca28ec76ee35d6c4e1ac008a2911d2fc4107d3c70e74f2919808945",
        ),
    ];

    for (lba, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(lba * 512, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("sector {lba}: {got}");
        assert_eq!(got, want, "decrypted sector {lba} does not match pybde");
    }

    let mut boot = [0u8; 512];
    vol.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);
}
