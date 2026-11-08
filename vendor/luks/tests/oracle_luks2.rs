//! Tier-2 oracle: unlock the self-minted `luks2.img` LUKS2 container (Argon2id
//! KDF, aes-xts-plain64 with 4096-byte sectors) and confirm the decrypted
//! sectors match `cryptsetup` byte-for-byte (SHA-256).
//!
//! Tier-2: minted on the "Ubuntu 24.04" VM (`cryptsetup luksFormat --type luks2
//! --pbkdf argon2id`), ground truth derived independently by `cryptsetup`. The
//! image is not committed (48 MiB) so the test is env-gated on `LUKS2_ORACLE`
//! (path to `luks2.img`). Provenance: `/tmp/luks-oracle/GROUND-TRUTH.md`.
//!
//! ```bash
//! LUKS2_ORACLE=/tmp/luks-oracle/luks2.img \
//!   cargo test -p luks-core --test oracle_luks2 -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use common::sha256_hex;
use luks::LuksVolume;

const PASSPHRASE: &[u8] = b"luks-TEST";

#[test]
fn tier2_luks2_argon2id_xts256_matches_cryptsetup() {
    let Ok(path) = std::env::var("LUKS2_ORACLE") else {
        eprintln!("LUKS2_ORACLE unset — skipping Tier-2 LUKS2 oracle");
        return;
    };
    let file = File::open(&path).expect("open luks2.img");
    // auto-detect version 2 through the unified entry point.
    let mut vol = LuksVolume::unlock_with_passphrase(file, PASSPHRASE).expect("unlock luks2.img");

    assert_eq!(vol.info().version, 2);
    assert_eq!(vol.info().cipher_mode, "xts-plain64");
    assert_eq!(vol.info().sector_size, 4096);
    assert_eq!(vol.info().key_bytes, 64);

    // (payload LBA in 512-byte units, expected decrypted-sector SHA-256).
    let cases: [(u64, &str); 6] = [
        (
            0,
            "205671f7bc4ba5c0589baa514222f66e6e2c62e464f7ed0a00f7f451439c4bac",
        ),
        (
            1,
            "091344e5c2087f37fad9fc83b68167ae7136cb88f97310393c2289f40079ce5a",
        ),
        (
            2,
            "510bc5bda3f1659b0fc8a7235ec927025d4363c823964acaa1c6795f9186080c",
        ),
        (
            16,
            "3d4ac612bd846109b5e6c7ebd664e3a2f29883b1b9a87fff39643aee2efdb36e",
        ),
        (
            100,
            "0dd58d3c600646aa31968daa1a4231d7b1067ccbe94d2806d94579938271334c",
        ),
        (
            199,
            "dd0399377354f80af98f0c511142ad5b59b16d04754bb0c97ed0b98a2114283a",
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
