use super::*;
use md5::{Digest, Md5};

#[test]
fn ewf_header_dates_preserve_local_wall_time_and_precision() {
    for (raw, display) in [
        ("2026 9 21 9 13 5", "2026-09-21 09:13:05"),
        ("2002 03 04 10 19 59", "2002-03-04 10:19:59"),
        (" \t2024  2 29 0 0 0\r\n", "2024-02-29 00:00:00"),
        ("2026-09-21 09:13:05", "2026-09-21 09:13:05"),
        ("2026-09-21T09:13:05", "2026-09-21 09:13:05"),
        ("2025-01-15", "2025-01-15"),
        ("0001 1 1 0 0 0", "0001-01-01 00:00:00"),
        ("9999 12 31 23 59 59", "9999-12-31 23:59:59"),
    ] {
        assert_eq!(
            parse_header_date(raw),
            Some(EwfHeaderDate {
                display: display.into(),
                time_basis: EwfDateTimeBasis::Local,
            }),
            "{raw:?}"
        );
    }
}

#[test]
fn ewf_header_dates_decode_posix_seconds_without_viewer_timezone() {
    for (raw, display) in [
        ("0", "1970-01-01 00:00:00"),
        ("1142163845", "2006-03-12 11:44:05"),
        ("2147483648", "2038-01-19 03:14:08"),
        ("253402300799", "9999-12-31 23:59:59"),
    ] {
        assert_eq!(
            parse_header_date(raw),
            Some(EwfHeaderDate {
                display: display.into(),
                time_basis: EwfDateTimeBasis::Unix,
            }),
            "{raw:?}"
        );
    }
}

#[test]
fn ewf_header_dates_reject_invalid_or_unsupported_values() {
    for raw in [
        "",
        " ",
        "unknown",
        "2026 2 29 1 2 3",
        "2024 13 1 0 0 0",
        "2024 1 0 0 0 0",
        "2024 1 1 24 0 0",
        "2024 1 1 0 60 0",
        "2024 1 1 0 0 61",
        "2026 9 21 9 13",
        "2026 9 21 9 13 5 +0800",
        "2026-09-21T09:13:05Z",
        "2026-02-29",
        "0000 1 1 0 0 0",
        "-1",
        "253402300800",
        "1770000000000",
        "18446744073709551616",
    ] {
        assert_eq!(parse_header_date(raw), None, "{raw:?}");
    }
}

#[test]
fn ewf_header_dates_keep_serialized_raw_fields_and_read_old_payloads() -> anyhow::Result<()> {
    let old = serde_json::json!({ "acquiry_date": "2026 9 21 9 13 5", "system_date": "bad" });
    let mut info: EwfAcquiryInfo = serde_json::from_value(old.clone())?;
    assert!(info.acquiry_date_parsed.is_none());
    info.acquiry_date_parsed = info.acquiry_date.as_deref().and_then(parse_header_date);
    let wire = serde_json::to_value(info)?;
    assert_eq!(wire["acquiry_date"], old["acquiry_date"]);
    assert_eq!(wire["system_date"], old["system_date"]);
    assert_eq!(
        wire["acquiry_date_parsed"],
        serde_json::json!({
            "display": "2026-09-21 09:13:05", "time_basis": "local",
        })
    );
    Ok(())
}

#[test]
fn ewf_external_acquisition_date_display() -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("EWF_DATE_IMAGE") else {
        return Ok(());
    };
    let reader = EwfHandle::open(Path::new(&path))?;
    let info = reader.acquiry_info();
    assert_eq!(info.acquiry_date.as_deref(), Some("2026 9 21 9 13 5"));
    assert_eq!(
        info.acquiry_date_parsed,
        Some(EwfHeaderDate {
            display: "2026-09-21 09:13:05".into(),
            time_basis: EwfDateTimeBasis::Local,
        })
    );
    Ok(())
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/ewf/tests/data")
        .join(name)
}

#[test]
fn ewf_real_ntfs_media_matches_acquisition_hash() {
    let reader = EwfHandle::open(&fixture("ntfs_sample.E01")).unwrap();
    let mut bytes = vec![0; reader.media_size() as usize];
    assert_eq!(reader.read_at(0, &mut bytes).unwrap(), bytes.len());
    assert_eq!(
        hex::encode(Md5::digest(&bytes)),
        "e4e9578a9c8bf6c6b375a80c630196a4"
    );
    assert_eq!(
        reader.acquiry_info().stored_md5.as_deref(),
        Some("e4e9578a9c8bf6c6b375a80c630196a4")
    );
    assert_eq!(
        (reader.bytes_per_sector(), &bytes[3..11]),
        (512, b"NTFS    ".as_slice())
    );
    let mut boundary = [0; 1024];
    reader.read_at(32760, &mut boundary).unwrap();
    assert_eq!(boundary.as_slice(), &bytes[32760..33784]);
    assert_eq!(
        reader.read_at(reader.media_size(), &mut boundary).unwrap(),
        0
    );
}

#[test]
fn ewf_multisegment_snapshots_and_hash_remain_complete() {
    let reader = EwfHandle::open(&fixture("multiseg_v1.E01")).unwrap();
    assert_eq!(reader.segment_file_snapshots().len(), 8);
    assert_eq!(reader.acquiry_info().segment_count, Some(8));
    assert_eq!(
        reader.segments_on_disk_bytes(),
        Some(
            reader
                .segment_file_snapshots()
                .iter()
                .map(|entry| entry.len)
                .sum()
        )
    );
    let mut digest = Md5::new();
    let mut offset = 0;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = reader.read_at(offset, &mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        offset += count as u64;
    }
    assert_eq!(offset, reader.media_size());
    assert_eq!(
        Some(hex::encode(digest.finalize())),
        reader.acquiry_info().stored_md5.clone()
    );
}

#[test]
fn ewf_v2_compressed_and_raw_media_are_identical() {
    for name in ["zeros_128s.Ex01", "zeros_128s_compressed.Ex01"] {
        let reader = EwfHandle::open(&fixture(name)).unwrap();
        assert_eq!(
            (reader.media_size(), reader.bytes_per_sector()),
            (65536, 512)
        );
        let mut bytes = vec![1; 65536];
        assert_eq!(reader.read_at(0, &mut bytes).unwrap(), 65536);
        assert_eq!(bytes, vec![0; 65536]);
    }
}

#[test]
fn ewf_missing_segment_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("case.E01");
    std::fs::copy(fixture("multiseg_v1.E01"), &first).unwrap();
    assert!(EwfHandle::open(&first)
        .unwrap_err()
        .to_string()
        .contains("incomplete"));
    std::fs::copy(
        fixture("multiseg_v1.E03"),
        directory.path().join("case.E03"),
    )
    .unwrap();
    assert!(EwfHandle::open(&first).is_err());
}

#[test]
fn ewf_unicode_and_glob_characters_in_paths_are_literal() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("案件[1]");
    std::fs::create_dir(&nested).unwrap();
    let path = nested.join("证据[01].EX01");
    std::fs::copy(fixture("zeros_128s_compressed.Ex01"), &path).unwrap();
    let reader = EwfHandle::open(&path).unwrap();
    assert_eq!(reader.media_size(), 65536);
    let renamed = nested.join("证据.bin");
    std::fs::rename(path, &renamed).unwrap();
    assert_eq!(EwfHandle::open(&renamed).unwrap().media_size(), 65536);
}

#[test]
fn ewf_corruption_reaches_existing_read_fault_reporting() {
    let original = fixture("ntfs_sample.E01");
    let probe = ewf::EwfReader::open(&original).unwrap();
    let (offset, _, compressed, _) = probe.debug_chunk(0).unwrap();
    assert!(compressed);
    let directory = tempfile::tempdir().unwrap();
    let damaged = directory.path().join("damaged.E01");
    let mut bytes = std::fs::read(original).unwrap();
    bytes[offset as usize] = 0; // Invalid zlib header, leaving all tables intact.
    std::fs::write(&damaged, bytes).unwrap();
    let reader = EwfHandle::open(&damaged).unwrap();
    assert!(reader.read_at(0, &mut [0; 512]).is_err());
    let image = crate::core::evidence::image::DiskImage::open(&damaged).unwrap();
    let (result, report) = image.collect_read_faults(|| image.read_range(0, 512));
    assert_eq!(result.unwrap(), vec![0; 512]);
    assert!(report.read_faults_detected);
    assert_eq!(report.synthetic_bytes, 512);
}
