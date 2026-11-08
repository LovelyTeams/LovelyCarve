//! Tier-1 oracle test: decrypt the real dfvfs `fvdetest` CoreStorage volume and
//! check each sector's SHA-256 against the ground-truth table in
//! `core/docs/RESEARCH.md`. The ground truth was produced by an independent
//! engine (libfvde's `fvdeinfo`) — this is an independent-oracle check, not a
//! self-authored round-trip.
//!
//! Env-gated on `FVDE_ORACLE_IMAGE` (the carved 512 MiB CS partition) so it
//! skips cleanly when the fixture is absent, like an oracle-binary gate.
//!
//! Run: `FVDE_ORACLE_IMAGE=/tmp/fvde-oracle/fvde_cs_p1.raw \
//!   cargo test -p filevault-core --test oracle_fvde -- --nocapture`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

use std::fs::File;

use filevault::FileVaultVolume;
use sha2::{Digest, Sha256};

const PASSWORD: &str = "fvde-TEST";

/// (LV offset, expected decrypted-sector SHA-256) from RESEARCH.md.
const ORACLE: &[(u64, &str)] = &[
    (
        0,
        "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
    ),
    (
        1024,
        "ebedb80407fc8bfdd3cce9c68de94efece7ed748df1babf35deeaacf008990af",
    ),
    (
        163_840,
        "a863e21577e54cd763729803a621804da4b5030afa35bcf879ea3b3413488a66",
    ),
    (
        1_048_576,
        "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
    ),
    (
        10_485_760,
        "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
    ),
];

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(data);
    digest.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[test]
fn decrypts_fvdetest_sectors_to_ground_truth() {
    let Ok(path) = std::env::var("FVDE_ORACLE_IMAGE") else {
        eprintln!("FVDE_ORACLE_IMAGE not set — skipping Tier-1 oracle test");
        return;
    };

    let file = File::open(&path).expect("open oracle image");
    let mut volume =
        FileVaultVolume::unlock_with_password(file, PASSWORD).expect("unlock with password");

    // The parsed metadata (no password needed) matches the ground truth.
    let info = volume.info().clone();
    assert_eq!(info.pbkdf2_iterations, 90506, "PBKDF2 iterations");
    assert_eq!(
        info.family_uuid, "1F01CA34-5F6C-4123-AC0C-B0A256889DB2",
        "family UUID"
    );
    assert_eq!(info.encryption_method, "AES-XTS-128");
    assert_eq!(info.lv_size, 167_772_160, "LV size");

    let mut all_ok = true;
    for &(offset, expected) in ORACLE {
        let mut sector = [0u8; 512];
        let read = volume.read_at(offset, &mut sector).expect("read_at");
        assert_eq!(read, 512, "short read at offset {offset}");
        let got = sha256_hex(&sector);
        if got == expected {
            eprintln!("OK at LV offset {offset}: {got}");
        } else {
            all_ok = false;
            eprintln!("MISMATCH at LV offset {offset}:\n  expected {expected}\n  got      {got}");
        }
    }
    assert!(
        all_ok,
        "one or more sector SHA-256s did not match ground truth"
    );
}
