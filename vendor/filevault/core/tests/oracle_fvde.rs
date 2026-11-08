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

#[test]
fn validated_inventory_and_selected_volume_match_oracle() {
    let Ok(path) = std::env::var("FVDE_ORACLE_IMAGE") else {
        return;
    };
    let container = filevault::storage::Container::open(File::open(&path).unwrap(), None).unwrap();
    let volumes: Vec<_> = container.volumes().collect();
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].info.lv_name.as_deref(), Some("TestLV"));
    assert_eq!(volumes[0].info.lv_size, 167_772_160);
    assert_eq!(volumes[0].transaction, 7);
    let mut volume = container.unlock_with_password(0, PASSWORD).unwrap();
    let mut bytes = [0; 512];
    volume.read_at(1024, &mut bytes).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "ebedb80407fc8bfdd3cce9c68de94efece7ed748df1babf35deeaacf008990af"
    );
}

#[test]
fn decrypted_oracle_hfs_catalog_and_logical_reads_agree() {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(path) = std::env::var("FVDE_ORACLE_IMAGE") else {
        return;
    };
    let decrypted = filevault::storage::Container::open(File::open(&path).unwrap(), None)
        .unwrap()
        .unlock_with_password(0, PASSWORD)
        .unwrap();
    let mut volume = hfsplus::HfsVolume::open(decrypted).unwrap();
    let entries = volume.list_directory("/").unwrap();
    assert!(!entries.is_empty());
    let paths: Vec<_> = volume
        .walk()
        .unwrap()
        .into_iter()
        .filter(|entry| {
            entry.entry.kind == hfsplus::EntryKind::File
                && entry.entry.size > 0
                && entry.entry.size < 1024 * 1024
        })
        .map(|entry| entry.path)
        .collect();
    assert!(!paths.is_empty());
    for path in paths {
        let full = volume.read_file(&path).unwrap();
        let mut streamed = Vec::new();
        volume.read_file_to(&path, &mut streamed).unwrap();
        assert_eq!(full, streamed, "{path}");
        let offset = (full.len() / 3) as u64;
        let mut reader = volume.open_file(&path).unwrap();
        reader.seek(SeekFrom::Start(offset)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, full[offset as usize..], "{path}");
    }
}

#[test]
fn recovered_vmk_and_xts_pair_match_real_oracle_and_reject_wrong_keys() {
    let Ok(path) = std::env::var("FVDE_ORACLE_IMAGE") else {
        return;
    };
    let key: [u8; 16] = [
        0xd0, 0xd9, 0xc3, 0x23, 0x19, 0x7c, 0x62, 0x40, 0x1c, 0x6e, 0x6b, 0x48, 0xf1, 0xc0, 0xf9,
        0xd7,
    ];
    let tweak: [u8; 16] = [
        0x53, 0xa1, 0x7b, 0xa3, 0x21, 0x3e, 0xc2, 0x13, 0xbe, 0xdc, 0xc3, 0x4f, 0xe4, 0xe2, 0x39,
        0xaf,
    ];
    for bytes in [key.to_vec(), [key.as_slice(), tweak.as_slice()].concat()] {
        let mut volume = filevault::storage::Container::open(File::open(&path).unwrap(), None)
            .unwrap()
            .unlock_with_master_key(0, &bytes)
            .unwrap();
        let mut header = [0; 512];
        volume.read_at(1024, &mut header).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(header)),
            "ebedb80407fc8bfdd3cce9c68de94efece7ed748df1babf35deeaacf008990af"
        );
    }
    assert!(
        filevault::storage::Container::open(File::open(&path).unwrap(), None)
            .unwrap()
            .unlock_with_master_key(0, &[0; 16])
            .is_err()
    );
}
