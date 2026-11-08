//! Tier-2 oracle: unlock the self-minted `m8005.raw` image (method `0x8005`,
//! XTS-AES-256) with its recovery password and confirm the decrypted sectors
//! match `pybde` (SHA-256).
//!
//! Env-gated on `BDE_MINT_ORACLE_DIR` (the directory holding `m8005.raw`).
//! Ground truth: `/tmp/bde-mint-oracle/GROUND-TRUTH.md`.
//!
//! ```bash
//! BDE_MINT_ORACLE_DIR=/tmp/bde-mint-oracle \
//!   cargo test -p bitlocker-core --test oracle_m8005 -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use bitlocker::BitLockerVolume;
use common::{sha256_hex, OffsetReader};

const PARTITION_OFFSET: u64 = 65536;
const RECOVERY_PW: &str = "031174-056914-397793-502348-055847-196306-284306-262174";

#[test]
fn tier2_m8005_matches_pybde() {
    let Ok(dir) = std::env::var("BDE_MINT_ORACLE_DIR") else {
        eprintln!("BDE_MINT_ORACLE_DIR unset — skipping Tier-2 m8005 oracle");
        return;
    };
    let path = format!("{dir}/m8005.raw");
    let file = File::open(&path).expect("open m8005.raw");
    let reader = OffsetReader::new(file, PARTITION_OFFSET).expect("window partition");

    let mut vol = BitLockerVolume::unlock_with_recovery_password(reader, RECOVERY_PW)
        .expect("unlock m8005.raw");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8005,
        "oracle must be XTS-AES-256"
    );

    let cases: [(u64, &str); 6] = [
        (
            0,
            "4d42f1742978a08922baccb6fa1541bbd811acbd316aa16490e5142d6a6aa413",
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
            "4bd6fcaf2201244b074ea212adfe07a9bf117b072783ebc5dc77eb98c1e0c913",
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

    let mut boot = [0u8; 512];
    vol.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);
}
