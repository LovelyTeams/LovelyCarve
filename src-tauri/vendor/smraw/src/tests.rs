use super::*;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "smraw-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn single_raw_empty_and_basename() {
    let fixture = Fixture::new();
    let path = fixture.file("disk.raw", b"abcdef");
    let image = RawImage::open(fixture.0.join("disk")).unwrap();
    assert_eq!(image.media_size(), 6);
    assert_eq!(image.maximum_segment_size(), 6);
    assert!(!discover(path).unwrap().numbered);
    let mut destination = [9; 8];
    assert_eq!(image.read_at(2, &mut destination).unwrap(), 4);
    assert_eq!(&destination, b"cdef\x09\x09\x09\x09");
    assert_eq!(image.read_at(u64::MAX, &mut destination).unwrap(), 0);
    assert_eq!(image.read_at(0, &mut []).unwrap(), 0);
    assert!(image.require_media_size(7).is_err());
    let empty = RawImage::open(fixture.file("empty.raw", b"")).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.read_at(0, &mut destination).unwrap(), 0);
}

#[test]
fn every_read_range_matches_independent_concatenation() {
    let fixture = Fixture::new();
    let paths = [
        fixture.file("证据 空格.001", b"abc"),
        fixture.file("证据 空格.002", b"defgh"),
        fixture.file("证据 空格.003", b"ijk"),
    ];
    let image = RawImage::open(&paths[1]).unwrap();
    image.set_maximum_open_handles(1).unwrap();
    assert_eq!(image.segments().len(), 3);
    let expected = b"abcdefghijk";
    for offset in 0..=13 {
        for length in 0..=13 {
            let mut output = vec![255; length];
            let count = image.read_at(offset, &mut output).unwrap();
            let start = (offset as usize).min(expected.len());
            let end = (start + length).min(expected.len());
            assert_eq!(&output[..count], &expected[start..end]);
            assert!(output[count..].iter().all(|byte| *byte == 255));
        }
    }
    let mut cursor = image.cursor();
    cursor.seek(SeekFrom::End(-5)).unwrap();
    let mut output = Vec::new();
    cursor.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"ghijk");
    assert!(cursor.seek(SeekFrom::Start(u64::MAX)).is_ok());
    assert!(cursor.seek(SeekFrom::Current(1)).is_err());
    cursor.seek(SeekFrom::Start(0)).unwrap();
    assert!(cursor.seek(SeekFrom::Current(-1)).is_err());
}

#[test]
fn numbered_families_and_carry() {
    for (first, second) in [
        ("disk.0", "disk.1"),
        ("disk.00", "disk.01"),
        ("disk.000", "disk.001"),
        ("disk.01", "disk.02"),
        ("disk.raw.001", "disk.raw.002"),
        ("disk00", "disk01"),
        ("disk000", "disk001"),
        ("disk-aa", "disk-ab"),
        ("disk-aaa", "disk-aab"),
    ] {
        let fixture = Fixture::new();
        let first = fixture.file(first, b"a");
        let second = fixture.file(second, b"b");
        let result = discover(second).unwrap();
        assert!(result.numbered, "{}", first.display());
        assert_eq!(result.paths.len(), 2);
        assert_eq!(result.paths[0], first);
    }
    let fixture = Fixture::new();
    for index in 0..=100 {
        fixture.file(&format!("carry.{index:02}"), b"x");
    }
    assert_eq!(
        discover(fixture.0.join("carry.00")).unwrap().paths.len(),
        101
    );
    assert_eq!(
        discover(fixture.0.join("carry.100")).unwrap().paths.len(),
        101
    );
    let ordinary = fixture.file("document.txt", b"x");
    assert!(!discover(ordinary).unwrap().numbered);
}

#[test]
fn caller_metadata_preserves_order_unicode_and_unknown_geometry() {
    let mut metadata = MediaMetadata::default();
    assert_eq!(metadata.bytes_per_sector, None);
    metadata.information.set("case_number", "案例 123").unwrap();
    metadata
        .information
        .set_utf16("examiner", &[0x68C0, 0x9A8C, 0xD83D, 0xDE00])
        .unwrap();
    metadata.information.set("case_number", "更新").unwrap();
    assert_eq!(metadata.information.len(), 2);
    assert_eq!(
        metadata.information.get_index(0),
        Some(("case_number", "更新"))
    );
    assert_eq!(metadata.information.get("examiner"), Some("检验😀"));
    assert_eq!(
        metadata.information.get_utf16("examiner").unwrap(),
        [0x68C0, 0x9A8C, 0xD83D, 0xDE00]
    );
    assert!(metadata
        .information
        .set_utf16("invalid", &[0xD800])
        .is_err());
    assert!(metadata.information.set("bad key", "text").is_err());
    assert!(metadata.information.set("nul", "a\0b").is_err());
    assert!(metadata
        .information
        .set("oversize", &"x".repeat(65_537))
        .is_err());
    assert!(metadata.integrity_hashes.is_empty());
    assert_eq!(metadata.information.iter().count(), 2);
}

#[cfg(windows)]
#[test]
fn discovery_accepts_windows_case_variants() {
    let fixture = Fixture::new();
    fixture.file("disk.001", b"a");
    fixture.file("disk.002", b"b");
    assert_eq!(
        RawImage::open(fixture.0.join("DISK.001"))
            .unwrap()
            .media_size(),
        2
    );
}

#[test]
fn externally_truncated_output_cannot_finish() {
    let fixture = Fixture::new();
    let mut writer = RawWriter::create(fixture.0.join("output"), 8, 4).unwrap();
    writer.write_all(b"abcdefgh").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&writer.paths()[1])
        .unwrap()
        .set_len(1)
        .unwrap();
    assert!(writer.finish().is_err());
    assert!(!fixture.0.join("output.raw.001").exists());
    assert!(!fixture.0.join("output.raw.002").exists());
}

#[test]
fn gaps_empty_duplicates_and_nonfiles_fail() {
    let fixture = Fixture::new();
    let first = fixture.file("disk.001", b"a");
    fixture.file("disk.003", b"c");
    assert!(discover(first)
        .unwrap_err()
        .to_string()
        .contains("missing RAW segment 2"));
    assert!(RawImage::open_segments(Vec::new()).is_err());
    let path = fixture.file("one", b"a");
    assert!(RawImage::open_segments([path.clone(), path]).is_err());
    let empty = fixture.file("empty", b"");
    assert!(RawImage::open_segments([empty, fixture.file("other", b"b")]).is_err());
    assert!(RawImage::open_segments([fixture.0.clone()]).is_err());
    assert!(RawImage::open(fixture.file("only.002", b"a")).is_err());
    assert!(RawImage::open(fixture.0.join("absent")).is_err());
}

#[test]
fn truncation_and_reopen_never_return_synthetic_bytes() {
    let fixture = Fixture::new();
    let first = fixture.file("disk.001", b"abc");
    let second = fixture.file("disk.002", b"def");
    let image = RawImage::open(first).unwrap();
    assert_eq!(image.read_at(3, &mut [0; 3]).unwrap(), 3);
    fs::OpenOptions::new()
        .write(true)
        .open(&second)
        .unwrap()
        .set_len(1)
        .unwrap();
    assert!(image.read_at(0, &mut [0; 6]).is_err());
    assert!(image.validate_sources().is_err());
    let fixture = Fixture::new();
    let first = fixture.file("disk.001", b"abc");
    let second = fixture.file("disk.002", b"def");
    let image = RawImage::open(first).unwrap();
    image.set_maximum_open_handles(1).unwrap();
    image.read_at(0, &mut [0; 3]).unwrap();
    fs::remove_file(second).unwrap();
    assert!(image.read_at(3, &mut [0; 3]).is_err());
}

#[test]
fn parallel_positioned_reads_and_abort() {
    let fixture = Fixture::new();
    let image =
        RawImage::open_segments([fixture.file("one", b"abcd"), fixture.file("two", b"efgh")])
            .unwrap();
    std::thread::scope(|scope| {
        for offset in 0..8 {
            let image = &image;
            scope.spawn(move || {
                let mut output = [0; 3];
                let count = image.read_at(offset, &mut output).unwrap();
                assert_eq!(
                    &output[..count],
                    &b"abcdefgh"[offset as usize..offset as usize + count]
                );
            });
        }
    });
    assert!(image.set_maximum_open_handles(0).is_err());
    image.abort_handle().abort();
    assert!(image.read_at(0, &mut [0; 1]).is_err());
    assert!(image.cursor().seek(SeekFrom::Start(0)).is_err());
}

#[test]
fn writer_roundtrip_random_overwrite_and_finish() {
    let fixture = Fixture::new();
    let mut writer = RawWriter::create(fixture.0.join("output"), 11, 4).unwrap();
    writer.write_all(b"abcdefghijk").unwrap();
    writer.write_at(3, b"DEF").unwrap();
    writer.seek(SeekFrom::Start(0)).unwrap();
    writer.write_all(b"A").unwrap();
    let paths = writer.finish().unwrap();
    assert_eq!(paths.len(), 3);
    assert!(paths[0].ends_with(Path::new("output.raw.001")));
    let image = RawImage::open(&paths[0]).unwrap();
    let mut output = Vec::new();
    image.cursor().read_to_end(&mut output).unwrap();
    assert_eq!(output, b"AbcDEFghijk");
    assert_eq!(
        image
            .segments()
            .iter()
            .map(|segment| segment.len)
            .collect::<Vec<_>>(),
        [4, 4, 3]
    );
    RawWriter::create(fixture.0.join("empty"), 0, 4)
        .unwrap()
        .finish()
        .unwrap();
    assert!(fixture.0.join("empty.raw").exists());
}

#[test]
fn failed_writes_clean_only_owned_outputs() {
    let fixture = Fixture::new();
    let basename = fixture.0.join("collision");
    let existing = fixture.file("collision.raw.002", b"keep evidence");
    let mut writer = RawWriter::create(&basename, 8, 4).unwrap();
    assert!(writer.write_all(b"abcdefgh").is_err());
    assert!(writer.finish().is_err());
    assert!(!fixture.0.join("collision.raw.001").exists());
    assert_eq!(fs::read(existing).unwrap(), b"keep evidence");
    let basename = fixture.0.join("partial");
    let mut writer = RawWriter::create(&basename, 10, 4).unwrap();
    writer.write_all(b"abc").unwrap();
    assert!(writer.finish().is_err());
    assert!(!fixture.0.join("partial.raw.001").exists());
    assert!(RawWriter::create(&basename, 5, 0).is_err());
    assert!(RawWriter::create(&basename, 1000, 1).is_err());
    let mut writer = RawWriter::create(&basename, 5, 5).unwrap();
    assert!(writer.write_at(1, b"a").is_err());
    assert!(writer.finish().is_err());
    let mut writer = RawWriter::create(&basename, 5, 5).unwrap();
    writer.abort_handle().abort();
    assert!(writer.write_all(b"abcde").is_err());
    assert!(writer.finish().is_err());
    assert!(!fixture.0.join("partial.raw").exists());
}
