//! Tier-2 oracle: unlock the self-minted `m8003.raw` image (method `0x8003`,
//! AES-256-CBC, no diffuser) with its recovery password and confirm the
//! decrypted sectors match `pybde` byte-for-byte (SHA-256).
//!
//! Env-gated on `BDE_MINT_ORACLE_DIR` (the directory holding `m8003.raw`) —
//! skips cleanly when absent (the images are ~128 MiB, not committed).
//! Provenance: `tests/data/README.md`; ground truth:
//! `/tmp/bde-mint-oracle/GROUND-TRUTH.md` and `docs/validation.md`.
//!
//! ```bash
//! BDE_MINT_ORACLE_DIR=/tmp/bde-mint-oracle \
//!   cargo test -p bitlocker-core --test oracle_m8003 -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use bitlocker::BitLockerVolume;
use common::{sha256_hex, OffsetReader};

/// The BitLocker partition sits at LBA 128 (byte 65536) in the minted images.
const PARTITION_OFFSET: u64 = 65536;
const RECOVERY_PW: &str = "068002-479633-277629-623568-540826-435039-327756-375705";

#[test]
fn tier2_m8003_matches_pybde() {
    let Ok(dir) = std::env::var("BDE_MINT_ORACLE_DIR") else {
        eprintln!("BDE_MINT_ORACLE_DIR unset — skipping Tier-2 m8003 oracle");
        return;
    };
    let path = format!("{dir}/m8003.raw");
    let file = File::open(&path).expect("open m8003.raw");
    let reader = OffsetReader::new(file, PARTITION_OFFSET).expect("window partition");

    let mut vol = BitLockerVolume::unlock_with_recovery_password(reader, RECOVERY_PW)
        .expect("unlock m8003.raw");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8003,
        "oracle must be AES-256-CBC (no diffuser)"
    );

    // (LBA, expected 512-byte-sector SHA-256) — pybde ground truth.
    let cases: [(u64, &str); 6] = [
        (
            0,
            "7ba645fe7a0a344f60c4bdeaeb551afb2ea53eec5949ed0ae14a5d063df09a98",
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
            "3a415b7d215ade07dd00c8001b88de30ea2f015967192f3f3c31cbfe3c45a67c",
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
        println!("LBA {lba}: {got}");
        assert_eq!(got, want, "decrypted LBA {lba} does not match pybde");
    }

    // Decrypted boot sector is the plaintext NTFS boot record.
    let mut boot = [0u8; 512];
    vol.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);
}
