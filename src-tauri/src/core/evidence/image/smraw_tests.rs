//! Split RAW through the shared desktop/CLI byte source.
use super::*;
use sha2::Digest;

#[test]
fn smraw_matches_single_raw_geometry_bytes_hash_and_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = partitions::gpt::fixture(512, 4, 128, 2);
    let raw = directory.path().join("single.raw");
    std::fs::write(&raw, &bytes).unwrap();
    let first = directory.path().join("证据 分段.raw.001");
    let second = directory.path().join("证据 分段.raw.002");
    let third = directory.path().join("证据 分段.raw.003");
    // Split a GPT header and array entry at deliberately unaligned offsets.
    std::fs::write(&first, &bytes[..530]).unwrap();
    std::fs::write(&second, &bytes[530..1040]).unwrap();
    std::fs::write(&third, &bytes[1040..]).unwrap();
    let single = DiskImage::open(&raw).unwrap();
    let split = Arc::new(DiskImage::open(&first).unwrap());
    assert!(matches!(split.source, ImageSource::SplitRaw { .. }));
    assert_eq!(split.info().source_format, ImageFormat::Raw);
    assert_eq!(split.info().on_disk_bytes, Some(bytes.len() as u64));
    assert_eq!(split.len(), single.len());
    assert_eq!(split.sector_size, single.sector_size);
    assert_eq!(split.table_scheme, single.table_scheme);
    assert_eq!(split.disk_id, single.disk_id);
    assert_eq!(split.partitions.len(), single.partitions.len());
    assert_eq!(
        split.partitions[0].start_lba,
        single.partitions[0].start_lba
    );
    assert_eq!(split.read_range(510, 550).unwrap(), bytes[510..1060]);
    assert_eq!(split.read_range(0, bytes.len()).unwrap(), bytes);
    let mut bulk = vec![0; bytes.len() + 1];
    let (count, report) = split.collect_read_faults(|| split.read_into(0, &mut bulk).unwrap());
    assert_eq!(count, bytes.len());
    assert!(report.bad_regions.is_empty());
    assert_eq!(
        sha2::Sha256::digest(&bulk[..count]),
        sha2::Sha256::digest(&bytes)
    );
    assert_eq!(split.source_file_stability_snapshots_at_open().len(), 3);
    split.validate_source_file_stable().unwrap();
    assert!(Arc::ptr_eq(&split, &split.concurrent_reader()));
    assert!(DiskImage::open(second)
        .unwrap_err()
        .to_string()
        .contains("第一个分段"));
    std::fs::OpenOptions::new()
        .write(true)
        .open(third)
        .unwrap()
        .set_len(1)
        .unwrap();
    assert!(split.validate_source_file_stable().is_err());
    assert!(split.read_range(1040, 512).is_err());
}

#[test]
fn smraw_missing_segment_is_an_error_without_raw_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("disk.001");
    std::fs::write(&first, [0; 4096]).unwrap();
    std::fs::write(directory.path().join("disk.003"), [0; 4096]).unwrap();
    assert!(format!("{:#}", DiskImage::open(first).unwrap_err()).contains("missing RAW segment 2"));
}

#[test]
fn smraw_plain_raw_keeps_the_mmap_path() {
    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join("single.raw");
    std::fs::write(&raw, [0; 4096]).unwrap();
    let image = DiskImage::open(raw).unwrap();
    assert!(matches!(image.source, ImageSource::Raw { .. }));
}

#[test]
fn smraw_forced_prefix_preserves_source_identity_bytes_and_audit() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("disk.dd");
    let second = directory.path().join("disk.002");
    std::fs::write(&first, [17; 517]).unwrap();
    std::fs::write(&second, [23; 510]).unwrap();
    std::fs::write(directory.path().join("disk.004"), [99; 4096]).unwrap();
    let image = DiskImage::open_forced_raw(&first).unwrap();
    assert_eq!(image.path, first);
    assert_eq!(image.len(), 1027);
    assert_eq!(image.info().on_disk_bytes, Some(1027));
    assert_eq!(image.info().raw_read_mode, RawReadMode::Force);
    assert_eq!(image.info().source_format, ImageFormat::Raw);
    assert_eq!(image.source_file_stability_snapshots_at_open().len(), 2);
    assert_eq!(image.read_range(515, 5).unwrap(), vec![17, 17, 23, 23, 23]);
    assert_eq!(image.read_range(1025, 10).unwrap(), vec![23; 2]);
    let mut reused = [255; 12];
    let (count, report) = image.collect_read_faults(|| image.read_into(1040, &mut reused).unwrap());
    assert_eq!(count, 12);
    assert_eq!(reused, [0; 12]);
    assert_eq!(
        report.bad_regions,
        vec![BadRegion {
            offset: 1040,
            len: 12
        }]
    );
    assert_eq!(report.synthetic_bytes, 12);
    assert!(image.has_bad_region_in_range(1040, 12));
    let (bytes, faults) = image.read_range_traced(1045, 4).unwrap();
    assert_eq!(bytes, [0; 4]);
    assert_eq!(
        faults,
        vec![BadRegion {
            offset: 1045,
            len: 4
        }]
    );
    assert!(image.read_range(u64::MAX, 2).is_err());
    image.validate_source_file_stable().unwrap();
    let middle = DiskImage::open_forced_raw(second).unwrap();
    assert_eq!(middle.len(), 510);
    assert_eq!(middle.read_range(0, 510).unwrap(), vec![23; 510]);
}

#[test]
fn smraw_forced_short_read_does_not_claim_complete_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join("single.raw");
    std::fs::write(&raw, [31; 4096]).unwrap();
    let image = DiskImage::open_forced_raw(&raw).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&raw)
        .unwrap()
        .set_len(1000)
        .unwrap();
    let mut output = [255; 4096];
    let (count, report) = image.collect_read_faults(|| image.read_into(0, &mut output).unwrap());
    assert_eq!(count, 1000);
    assert_eq!(&output[..1000], &[31; 1000]);
    assert!(output[1000..].iter().all(|byte| *byte == 255));
    assert!(!report.read_faults_detected);
    assert!(image.read_range(1000, 100).unwrap().is_empty());
    assert_eq!(image.len(), 4096);
    assert!(image.validate_source_file_stable().is_err());
}

#[test]
fn smraw_forced_mode_has_distinct_fingerprint_and_keeps_default_eof() {
    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join("single.raw");
    std::fs::write(&raw, [31; 4096]).unwrap();
    let strict = DiskImage::open(&raw).unwrap();
    let forced = DiskImage::open_forced_raw(&raw).unwrap();
    assert_eq!(strict.info().raw_read_mode, RawReadMode::Strict);
    assert!(strict.read_range(4096, 20).unwrap().is_empty());
    assert_eq!(forced.read_range(4096, 20).unwrap(), vec![0; 20]);
    assert_ne!(strict.evidence_fingerprint(), forced.evidence_fingerprint());
    assert_eq!(
        strict.read_range(0, 4096).unwrap(),
        forced.read_range(0, 4096).unwrap()
    );
    assert_eq!(
        serde_json::to_value(forced.info()).unwrap()["raw_read_mode"],
        "force"
    );
}

#[test]
fn smraw_forced_open_probe_keeps_outside_capacity_faults() {
    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join("single.raw");
    std::fs::write(&raw, [31; 512]).unwrap();
    let source = ImageSource::SplitRaw {
        reader: smraw::RawImage::open_forced(raw).unwrap(),
    };
    let bad_regions = Mutex::new(Vec::new());
    let probe = OpenSectorSource {
        source: &source,
        cache: None,
        bad_regions: &bad_regions,
    };
    assert_eq!(probe.read(2048, 16).unwrap(), vec![0; 16]);
    assert_eq!(
        *bad_regions.lock().unwrap(),
        vec![BadRegion {
            offset: 2048,
            len: 16
        }]
    );
}
