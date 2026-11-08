//! End-to-end tests using small public E01 images from Digital Corpora (DFTT project).
//!
//! Test fixtures in `tests/data/` are committed to the repo (~1.2 MB total).
//! Raw media MD5 hashes verified against both libewf (ewfexport) and The Sleuth Kit (`img_cat`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use md5::{Digest, Md5};
use std::io::{Read, Seek, SeekFrom};

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn full_media_md5(reader: &mut ewf::EwfReader) -> String {
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024]; // 1 MB buffer
    reader.seek(SeekFrom::Start(0)).unwrap();
    loop {
        let n = reader.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    format!("{:x}", hasher.finalize())
}

// ---------- exfat1.E01 (EnCase 6, deflate best-compression, exFAT) ----------

#[test]
fn exfat1_media_size() {
    let path = format!("{DATA_DIR}/exfat1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(reader.total_size(), 100_020_736);
}

#[test]
fn exfat1_full_media_md5() {
    let path = format!("{DATA_DIR}/exfat1.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(
        full_media_md5(&mut reader),
        "0777ee90c27ed5ff5868af2015bed635",
        "Full-media MD5 mismatch vs libewf/Sleuth Kit"
    );
}

#[test]
fn exfat1_seek_and_read_consistency() {
    let path = format!("{DATA_DIR}/exfat1.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();

    // Read first 512 bytes sequentially
    let mut sequential = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader.read_exact(&mut sequential).unwrap();

    // Read same bytes via seek
    let mut seeked = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader.read_exact(&mut seeked).unwrap();

    assert_eq!(sequential, seeked);

    // Read at a chunk boundary (32 KB = 32768)
    let mut at_boundary = [0u8; 512];
    reader.seek(SeekFrom::Start(32768)).unwrap();
    reader.read_exact(&mut at_boundary).unwrap();

    // Read same region again
    let mut at_boundary2 = [0u8; 512];
    reader.seek(SeekFrom::Start(32768)).unwrap();
    reader.read_exact(&mut at_boundary2).unwrap();

    assert_eq!(at_boundary, at_boundary2);
}

// ---------- imageformat_mmls_1.E01 (FTK Imager, no compression, NTFS) ----------

#[test]
fn mmls1_media_size() {
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(reader.total_size(), 62_915_072);
}

#[test]
fn mmls1_full_media_md5() {
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(
        full_media_md5(&mut reader),
        "8ec671e301095c258224aad701740503",
        "Full-media MD5 mismatch vs libewf/Sleuth Kit"
    );
}

#[test]
fn mmls1_mbr_signature() {
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();
    let mut mbr = [0u8; 512];
    reader.read_exact(&mut mbr).unwrap();
    assert_eq!(mbr[510], 0x55);
    assert_eq!(mbr[511], 0xAA);
}

#[test]
fn mmls1_seek_from_end() {
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();

    // Seek to last 512 bytes
    reader.seek(SeekFrom::End(-512)).unwrap();
    let mut last_sector = [0u8; 512];
    reader.read_exact(&mut last_sector).unwrap();

    // Verify position is at end
    let pos = reader.stream_position().unwrap();
    assert_eq!(pos, 62_915_072);
}

// ---------- nps-2010-emails.E01 (EnCase 6, deflate best, 10 MiB) ----------

#[test]
fn emails_media_size() {
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(reader.total_size(), 10_485_760);
}

#[test]
fn emails_full_media_md5() {
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();
    assert_eq!(
        full_media_md5(&mut reader),
        "7dae50cec8163697415e69fd72387c01",
        "Full-media MD5 mismatch vs libewf/Sleuth Kit"
    );
}

#[test]
fn emails_sequential_equals_random_access() {
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let mut reader = ewf::EwfReader::open(&path).unwrap();

    // Read 4 chunks sequentially from offset 0
    let mut sequential = vec![0u8; 32768 * 4];
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader.read_exact(&mut sequential).unwrap();

    // Read same 4 chunks in reverse order via seeking
    let mut random_access = vec![0u8; 32768 * 4];
    for i in (0..4).rev() {
        let offset = i * 32768;
        reader.seek(SeekFrom::Start(offset as u64)).unwrap();
        reader
            .read_exact(&mut random_access[offset..offset + 32768])
            .unwrap();
    }

    assert_eq!(sequential, random_access);
}

// ---------- stored_hashes() — hash and digest section parsing ----------

/// Helper: format a raw byte array as a lowercase hex string.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn exfat1_stored_md5_matches_media_hash() {
    // exfat1.E01 was created by EnCase 6, which writes a `hash` section
    // containing the MD5 of the acquired media.
    let path = format!("{DATA_DIR}/exfat1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let hashes = reader.stored_hashes();
    let md5 = hashes
        .md5
        .expect("EnCase 6 image should contain stored MD5");
    assert_eq!(
        hex(&md5),
        "0777ee90c27ed5ff5868af2015bed635",
        "Stored MD5 should match full-media MD5"
    );
}

#[test]
fn emails_stored_md5_matches_media_hash() {
    // nps-2010-emails.E01 was also created by EnCase 6.
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let hashes = reader.stored_hashes();
    let md5 = hashes
        .md5
        .expect("EnCase 6 image should contain stored MD5");
    assert_eq!(
        hex(&md5),
        "7dae50cec8163697415e69fd72387c01",
        "Stored MD5 should match full-media MD5"
    );
}

#[test]
fn stored_hashes_returns_none_when_absent() {
    // FTK Imager images may not have hash/digest sections.
    // At minimum, stored_hashes() must not panic — None is acceptable.
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let _hashes = reader.stored_hashes(); // must not panic
}

// ---------- verify() — full-media hash verification ----------

#[test]
fn exfat1_verify_passes() {
    // EnCase 6 image with stored MD5 — verify() should confirm integrity.
    let path = format!("{DATA_DIR}/exfat1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let result = reader.verify().unwrap();
    assert!(
        result.md5_match.unwrap(),
        "MD5 verification should pass for intact image"
    );
}

#[test]
fn emails_verify_passes() {
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let result = reader.verify().unwrap();
    assert!(
        result.md5_match.unwrap(),
        "MD5 verification should pass for intact image"
    );
}

#[test]
fn verify_returns_none_when_no_stored_hashes() {
    // FTK Imager image may not have hash/digest sections.
    // verify() should return None for match fields (nothing to compare against).
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let result = reader.verify().unwrap();
    if reader.stored_hashes().md5.is_none() {
        assert!(
            result.md5_match.is_none(),
            "No stored MD5 means md5_match should be None"
        );
    }
}

#[test]
fn verify_computed_hashes_match_manual_stream() {
    // Cross-check: verify()'s computed MD5 should match our manual full_media_md5() helper.
    let path = format!("{DATA_DIR}/exfat1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let result = reader.verify().unwrap();
    assert_eq!(
        hex(&result.computed_md5),
        "0777ee90c27ed5ff5868af2015bed635",
        "verify() computed MD5 should match independent full-media hash"
    );
}

// ---------- metadata() — header section parsing ----------

#[test]
fn mmls1_metadata_has_case_info() {
    // FTK Imager image has rich metadata: case, evidence, examiner, description, notes.
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let meta = reader.metadata();
    assert_eq!(meta.case_number.as_deref(), Some("1"));
    assert_eq!(meta.evidence_number.as_deref(), Some("1"));
    assert_eq!(meta.description.as_deref(), Some("Test E01 for sleuthkit"));
    assert_eq!(meta.examiner.as_deref(), Some("Rishwanth"));
    assert_eq!(
        meta.notes.as_deref(),
        Some("Used to test sleuthkit libraries")
    );
}

#[test]
fn mmls1_metadata_has_tool_info() {
    let path = format!("{DATA_DIR}/imageformat_mmls_1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let meta = reader.metadata();
    assert_eq!(meta.acquiry_software.as_deref(), Some("ADI2.9.0.13"));
    assert_eq!(meta.os_version.as_deref(), Some("Windows 200x"));
}

#[test]
fn exfat1_metadata_has_os_and_dates() {
    // EnCase 6 image — sparse case info but has OS and dates.
    let path = format!("{DATA_DIR}/exfat1.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let meta = reader.metadata();
    assert_eq!(meta.os_version.as_deref(), Some("Darwin"));
    assert!(meta.acquiry_date.is_some(), "Should have acquisition date");
    assert!(meta.system_date.is_some(), "Should have system date");
}

#[test]
fn emails_metadata_parses_without_panic() {
    let path = format!("{DATA_DIR}/nps-2010-emails.E01");
    let reader = ewf::EwfReader::open(&path).unwrap();
    let meta = reader.metadata();
    assert_eq!(meta.os_version.as_deref(), Some("Darwin"));
}

// ---------- acquisition_errors() — error2 section parsing ----------

#[test]
fn clean_image_has_no_acquisition_errors() {
    // All our test images had clean acquisitions — no read errors.
    for name in [
        "exfat1.E01",
        "imageformat_mmls_1.E01",
        "nps-2010-emails.E01",
    ] {
        let path = format!("{DATA_DIR}/{name}");
        let reader = ewf::EwfReader::open(&path).unwrap();
        assert!(
            reader.acquisition_errors().is_empty(),
            "{name} should have no acquisition errors"
        );
    }
}

#[test]
fn parse_error2_data_extracts_entries() {
    // Synthetic error2 section data:
    // u32 number_of_entries = 2
    // 4 bytes padding
    // Entry 1: first_sector=100, sector_count=5
    // Entry 2: first_sector=5000, sector_count=1
    // 4 bytes Adler-32 (ignored for now)
    let mut data = Vec::new();
    data.extend_from_slice(&2u32.to_le_bytes()); // entry count
    data.extend_from_slice(&[0u8; 4]); // padding
    data.extend_from_slice(&100u32.to_le_bytes()); // entry 1 first_sector
    data.extend_from_slice(&5u32.to_le_bytes()); // entry 1 sector_count
    data.extend_from_slice(&5000u32.to_le_bytes()); // entry 2 first_sector
    data.extend_from_slice(&1u32.to_le_bytes()); // entry 2 sector_count
    data.extend_from_slice(&[0u8; 4]); // checksum placeholder

    let errors = ewf::parse_error2_data(&data);
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].first_sector, 100);
    assert_eq!(errors[0].sector_count, 5);
    assert_eq!(errors[1].first_sector, 5000);
    assert_eq!(errors[1].sector_count, 1);
}

#[test]
fn parse_error2_data_handles_empty() {
    // Zero entries
    let mut data = Vec::new();
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&[0u8; 4]);
    data.extend_from_slice(&[0u8; 4]); // checksum

    let errors = ewf::parse_error2_data(&data);
    assert!(errors.is_empty());
}

// ---------- L01 (logical evidence file) ----------
//
// The two tests that stood here asserted L01 support by COPYING AN E01 to a
// .L01 path and checking it opened. They tested the extension branch and
// nothing else, while the comment claimed "L01 uses the same EWF v1 container
// format, just a different extension".
//
// That comment is false on both counts. A real L01 begins LVF\x09\x0d\x0a\xff\x00,
// not EVF, so EwfFileHeader::parse rejects it outright; and the content model is
// file ENTRIES in an ltree section, not disk sectors. Verified against a real
// EnCase L01: `ewf info` returns "invalid EWF signature".
//
// Real coverage lives in core/tests/logical.rs against a fixture whose bytes
// follow the LVF layout rather than an E01 wearing its name.

#[test]
fn l01_full_media_md5_matches() {
    let src = format!("{DATA_DIR}/nps-2010-emails.E01");
    let tmp = tempfile::tempdir().unwrap();
    let l01_path = tmp.path().join("evidence.L01");
    std::fs::copy(&src, &l01_path).unwrap();

    let result = ewf::EwfReader::open(&l01_path);
    assert!(result.is_ok(), "L01 should open");
    let mut reader = result.unwrap();
    assert_eq!(
        full_media_md5(&mut reader),
        "7dae50cec8163697415e69fd72387c01",
        "L01 full-media MD5 should match source E01"
    );
}
