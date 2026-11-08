//! Tier-2 oracle: unlock the self-minted `luks1.img` LUKS1 container (aes /
//! xts-plain64, AES-256-XTS) with its passphrase and confirm the decrypted
//! sectors match `cryptsetup` byte-for-byte (SHA-256).
//!
//! Tier-2: we minted the container on the "Ubuntu 24.04" VM (`cryptsetup
//! luksFormat`), but the ground truth is derived independently by `cryptsetup`
//! (the answer key). The image is not committed (32 MiB) so the test is env-gated
//! on `LUKS1_ORACLE` (path to `luks1.img`) and skips cleanly when absent.
//! Provenance + ground truth: `/tmp/luks-oracle/GROUND-TRUTH.md`.
//!
//! ```bash
//! LUKS1_ORACLE=/tmp/luks-oracle/luks1.img \
//!   cargo test -p luks-core --test oracle_luks1 -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use common::sha256_hex;
use luks::LuksVolume;

const PASSPHRASE: &[u8] = b"luks-TEST";

#[test]
fn tier2_luks1_xts256_matches_cryptsetup() {
    let Ok(path) = std::env::var("LUKS1_ORACLE") else {
        eprintln!("LUKS1_ORACLE unset — skipping Tier-2 LUKS1 oracle");
        return;
    };
    let file = File::open(&path).expect("open luks1.img");
    let mut vol = LuksVolume::unlock1_with_passphrase(file, PASSPHRASE).expect("unlock luks1.img");

    assert_eq!(vol.info().cipher_name, "aes");
    assert_eq!(vol.info().cipher_mode, "xts-plain64");
    assert_eq!(vol.info().key_bytes, 64);

    // (payload LBA, expected decrypted-sector SHA-256) — cryptsetup ground truth.
    let cases: [(u64, &str); 6] = [
        (
            0,
            "c9d8e3352f9f790d8b0be13cb1c18ed7963009888be04acc065ee5efbd934076",
        ),
        (
            1,
            "cb287e82f2af042cd65f1e72f3e4738a0a6dba5d9941b0dfd476fd5c8ea27cb0",
        ),
        (
            2,
            "492524d6fe4f2fe9309718c3d531078342c32bde0da7dd9d111bd17812f3cbe9",
        ),
        (
            16,
            "037f30c89ab88aa9d58417d2d95adb10b22f8691a17f7db71169ad3194ace706",
        ),
        (
            100,
            "d3c7f1b00475d01452cdbfcb4d9666dddecb3c633e67206016be05980293b57c",
        ),
        (
            199,
            "3e9dc47b357b72b7256d9fc953ee61be87d391ce34b6d3932ac5421294a84ae2",
        ),
    ];

    for (lba, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(lba * 512, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("sector {lba}: {got}");
        assert_eq!(
            got, want,
            "decrypted sector {lba} does not match cryptsetup"
        );
    }
}

#[test]
fn wrong_passphrase_fails() {
    let Ok(path) = std::env::var("LUKS1_ORACLE") else {
        return;
    };
    let file = File::open(&path).unwrap();
    let res = LuksVolume::unlock1_with_passphrase(file, b"wrong-pw");
    assert!(res.is_err());
}
