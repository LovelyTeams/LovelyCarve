//! Tier-1 oracle: unlock the `BelkaCTF` #6 `vault.raw` BitLocker volume (method
//! `0x8004`, XTS-AES-128) with its **published recovery password** and confirm
//! the decrypted sectors match `pybde` byte-for-byte (SHA-256).
//!
//! Tier-1: a third party (Belkasoft) authored the image AND published the
//! recovery key; verified independently by `pybde`. The image is Belkasoft CTF
//! material — **not committed** (2 GiB, license) — so the test is env-gated on
//! `BDE_XTS_ORACLE` (the path to `vault.raw`) and skips cleanly when absent.
//! Provenance: `tests/data/README.md`.
//!
//! ```bash
//! BDE_XTS_ORACLE=/tmp/bde-xts-oracle/vault.raw \
//!   cargo test -p bitlocker-core --test oracle_vault -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use bitlocker::BitLockerVolume;
use common::{sha256_hex, OffsetReader};

/// The BitLocker volume sits at byte 16777216 (sector 32768) in `vault.raw`.
const VOLUME_OFFSET: u64 = 16_777_216;
const RECOVERY_PW: &str = "590238-514580-359986-088242-029766-319495-410509-636911";

#[test]
fn tier1_vault_xts128_matches_pybde() {
    let Ok(path) = std::env::var("BDE_XTS_ORACLE") else {
        eprintln!("BDE_XTS_ORACLE unset — skipping Tier-1 vault XTS-128 oracle");
        return;
    };
    let file = File::open(&path).expect("open vault.raw");
    let reader = OffsetReader::new(file, VOLUME_OFFSET).expect("window volume");

    let mut vol = BitLockerVolume::unlock_with_recovery_password(reader, RECOVERY_PW)
        .expect("unlock vault.raw");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8004,
        "oracle must be XTS-AES-128"
    );

    // (sector LBA, expected 512-byte-sector SHA-256) — self-derived pybde ground
    // truth (see tests/data/README.md). Sectors 0–5 are the relocated boot
    // region (they settle the header-region XTS tweak); 32768/131072/262144 are
    // 16/64/128 MiB deep (they pin the tweak to the sector number).
    let cases: [(u64, &str); 8] = [
        (
            0,
            "7000a9d734628583f2debea8496ef532eb71f82f17a11acbcb084c0e2a27ce67",
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
            3,
            "f49bb7dfe830eaff32cc57199fece014a7120851c126323196877295c94a14fe",
        ),
        (
            5,
            "e07295e7e678283f029f21271922fe254ee3e6b44f80180a71e9d78b4e7823b3",
        ),
        (
            12298,
            "62fe68b09231ab0019696043859aeb010a92b224a32b01bd7e7cc47b3d85c324",
        ),
        (
            32768,
            "b525ecbce7a22825746e9f938498131c3c8d99a22e5c9a47294be43c35a0748d",
        ),
        (
            262_144,
            "999e459d1b2996dc8bc0826d0ef2e6521c3a8636b8f58a6c5ba192961f674f4a",
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
