//! Tier-1 oracle: unlock the real picoCTF 2025 `bitlocker-1.dd` image (method
//! `0x8002`, AES-128-CBC **without** the Elephant Diffuser) and confirm the
//! decrypted sectors match `pybde`'s output byte-for-byte (SHA-256).
//!
//! Env-gated on `BDE_CBC2_ORACLE` — skips cleanly when the image is absent (it
//! is 100 MiB and not committed). Provenance: `tests/data/README.md`; ground
//! truth: `docs/validation.md`.
//!
//! ```bash
//! BDE_CBC2_ORACLE=/path/to/bitlocker-1.dd \
//!   cargo test -p bitlocker-core --test oracle_bitlocker1 -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use bitlocker::BitLockerVolume;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[test]
fn tier1_bitlocker1_matches_pybde() {
    let Ok(path) = std::env::var("BDE_CBC2_ORACLE") else {
        eprintln!("BDE_CBC2_ORACLE unset — skipping Tier-1 oracle (see tests/data/README.md)");
        return;
    };

    let file = File::open(&path).expect("open BDE_CBC2_ORACLE");
    let mut vol =
        BitLockerVolume::unlock_with_password(file, "jacqueline").expect("unlock bitlocker-1.dd");

    assert_eq!(
        vol.metadata().encryption_method,
        0x8002,
        "oracle must be AES-128-CBC without diffuser"
    );

    // (logical offset, expected 512-byte-sector SHA-256) — pybde ground truth.
    let cases: [(u64, &str); 5] = [
        (
            0,
            "f2468babca05cc29cf32ba3afec1042f18f7ea3e57d41317fd1689ebd27ea65e",
        ),
        (
            512,
            "ef6d6118087d7849e66c32de9859dc5a74b98aa2d28b3f2e6e87275b537eb546",
        ),
        (
            1024,
            "e845941331aaf16324636abd0c499908757d12eaf947c841e02164c4b9e1edad",
        ),
        (
            1536,
            "f49bb7dfe830eaff32cc57199fece014a7120851c126323196877295c94a14fe",
        ),
        (
            2048,
            "7289d589c1a46ab5af939d7da273d155eae76d3f2289aa0e6cd7ddae09c27ee3",
        ),
    ];

    for (offset, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(offset, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("offset {offset:#08x}: {got}");
        assert_eq!(
            got, want,
            "decrypted sector at {offset:#x} does not match pybde"
        );
    }

    // The decrypted first sector is a valid NTFS boot sector ("NTFS    " at
    // offset 3), proving the sector was decrypted, not read raw.
    vol.seek(SeekFrom::Start(0)).unwrap();
    let mut boot = [0u8; 512];
    vol.read_exact(&mut boot).unwrap();
    assert_eq!(sha256_hex(&boot), cases[0].1);
    assert_eq!(&boot[3..11], b"NTFS    ");
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);
}
