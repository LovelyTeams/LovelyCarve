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
