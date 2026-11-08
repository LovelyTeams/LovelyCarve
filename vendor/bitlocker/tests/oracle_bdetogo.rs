//! Tier-1 oracle: unlock the real dfvfs `bdetogo.raw` image and confirm the
//! decrypted sectors match `pybde`'s output byte-for-byte (SHA-256).
//!
//! Env-gated on `BDE_ORACLE_IMAGE` — skips cleanly when the image is absent (it
//! is 64 MiB and not committed). Provenance: `tests/data/README.md`; ground
//! truth: `docs/validation.md`.
//!
//! ```bash
//! BDE_ORACLE_IMAGE=/path/to/bdetogo.raw \
//!   cargo test -p bitlocker-core --test oracle_bdetogo -- --nocapture
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
fn tier1_bdetogo_matches_pybde() {
    let Ok(path) = std::env::var("BDE_ORACLE_IMAGE") else {
        eprintln!("BDE_ORACLE_IMAGE unset — skipping Tier-1 oracle (see tests/data/README.md)");
        return;
    };

    let file = File::open(&path).expect("open BDE_ORACLE_IMAGE");
    let mut vol =
        BitLockerVolume::unlock_with_password(file, "bde-TEST").expect("unlock bdetogo.raw");

    // (logical offset, read length, expected SHA-256) — pybde ground truth.
    let cases: [(u64, usize, &str); 5] = [
        (
            0,
            512,
            "139b857c537e341ceb98bcfde2d31825efcf4b0c0281dd66672e954b34ed28f3",
        ),
        (
            512,
            512,
            "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
        ),
        (
            2048,
            512,
            "bf762af77278b30a1b8bbdfb6cbf2e3565cdf6fcb09d48e683c464af2c3abd71",
        ),
        (
            35840,
            512,
            "48ddda42757a815d391ff1d3f01a37b085fd05ec57a9d903946412551bfe5a7b",
        ),
        (
            0x8000,
            4096,
            "1d138f11707f0a3b4832c5173167c756e1bac9796518b64800d5dc374986fe4d",
        ),
    ];

    for (offset, len, want) in cases {
        let mut buf = vec![0u8; len];
        vol.read_at(offset, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("offset {offset:#08x} len {len}: {got}");
        assert_eq!(
            got, want,
            "decrypted sector at {offset:#x} does not match pybde"
        );
    }

    // Exercise the Read + Seek surface too.
    vol.seek(SeekFrom::Start(0)).unwrap();
    let mut boot = [0u8; 512];
    vol.read_exact(&mut boot).unwrap();
    assert_eq!(sha256_hex(&boot), cases[0].2);
    // The decrypted first sector is the ORIGINAL FAT boot sector (OEM name
    // "mkdosfs"), not the BitLocker To Go wrapper — proving the relocation was
    // followed and the sector decrypted, not read raw.
    assert_eq!(&boot[3..11], b"mkdosfs\0");
    assert_eq!(&boot[510..512], &[0x55, 0xaa]);
}
