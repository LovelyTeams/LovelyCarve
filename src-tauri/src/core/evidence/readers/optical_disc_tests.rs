use super::*;
use std::sync::Arc;

use crate::core::evidence::image::{DiskImage, PartitionKind};
use crate::core::evidence::partitions::test_support::BufSource;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, FsSession, NodeKind};

fn both32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    bytes[offset + 4..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn both16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    bytes[offset + 2..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn record(name: &[u8], extent: u32, length: u32, directory: bool) -> Vec<u8> {
    let mut bytes = vec![0; 33 + name.len() + usize::from(name.len().is_multiple_of(2))];
    bytes[0] = bytes.len() as u8;
    both32(&mut bytes, 2, extent);
    both32(&mut bytes, 10, length);
    bytes[18..25].copy_from_slice(&[126, 10, 21, 1, 2, 3, 0]);
    bytes[25] = if directory { 2 } else { 0 };
    both16(&mut bytes, 28, 1);
    bytes[32] = name.len() as u8;
    bytes[33..33 + name.len()].copy_from_slice(name);
    bytes
}

fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let content: Vec<u8> = (0..7001)
        .map(|index| (index * 31 + index / 47) as u8)
        .collect();
    let mut iso = vec![0; 40 * DATA_BYTES];
    for (number, root, joliet) in [(16, 20, false), (17, 21, true)] {
        let descriptor = &mut iso[number * DATA_BYTES..(number + 1) * DATA_BYTES];
        descriptor[0] = if joliet { 2 } else { 1 };
        descriptor[1..7].copy_from_slice(b"CD001\x01");
        both32(descriptor, 80, 40);
        if joliet {
            descriptor[88..91].copy_from_slice(b"%/E");
        }
        both16(descriptor, 120, 1);
        both16(descriptor, 124, 1);
        both16(descriptor, 128, DATA_BYTES as u16);
        descriptor[156..190].copy_from_slice(&record(&[0], root, DATA_BYTES as u32, true));
        descriptor[881] = 1;
    }
    iso[18 * DATA_BYTES] = 255;
    iso[18 * DATA_BYTES + 1..18 * DATA_BYTES + 7].copy_from_slice(b"CD001\x01");
    for (root, name) in [
        (20, b"PAYLOAD.BIN;1".to_vec()),
        (
            21,
            "测试数据.bin;1"
                .encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect(),
        ),
    ] {
        let entries = [
            record(&[0], root, DATA_BYTES as u32, true),
            record(&[1], root, DATA_BYTES as u32, true),
            record(&name, 24, content.len() as u32, false),
        ]
        .concat();
        iso[root as usize * DATA_BYTES..root as usize * DATA_BYTES + entries.len()]
            .copy_from_slice(&entries);
    }
    iso[24 * DATA_BYTES..24 * DATA_BYTES + content.len()].copy_from_slice(&content);
    let mut raw = vec![0xa5; 40 * RAW_SECTOR_BYTES];
    for (number, block) in raw
        .as_chunks_mut::<RAW_SECTOR_BYTES>()
        .0
        .iter_mut()
        .enumerate()
    {
        block[..12].copy_from_slice(&SYNC);
        let absolute = number + 150;
        for (field, value) in [absolute / (60 * 75), absolute / 75 % 60, absolute % 75]
            .into_iter()
            .enumerate()
        {
            block[12 + field] = ((value / 10) * 16 + value % 10) as u8;
        }
        block[15] = 1;
        block[16..16 + DATA_BYTES]
            .copy_from_slice(&iso[number * DATA_BYTES..(number + 1) * DATA_BYTES]);
    }
    (raw, iso, content)
}

#[test]
fn raw_cd_mode1_projects_only_data_and_retains_eof_semantics() {
    let (raw, iso, _) = fixture();
    let source = BufSource(raw);
    let layout = Mode1Layout::probe(&source).unwrap().unwrap();
    assert_eq!(layout.logical_length(), iso.len() as u64);
    for (offset, count) in [
        (0, 0),
        (2043, 19),
        (16 * DATA_BYTES + 4, 8191),
        (iso.len() - 3, 100),
        (iso.len(), 50),
    ] {
        let end = offset.saturating_add(count).min(iso.len());
        assert_eq!(
            layout.read(&source, offset as u64, count).unwrap(),
            iso[offset..end]
        );
    }
    assert!(layout
        .read(&source, u64::MAX, usize::MAX)
        .unwrap()
        .is_empty());
    assert!(Mode1Layout::probe(&BufSource(iso)).unwrap().is_none());
}

#[test]
fn raw_cd_mode1_reads_joliet_files_and_preserves_source_bytes() {
    let (raw, iso, content) = fixture();
    let temp = tempfile::tempdir().unwrap();
    for (name, source, expected_sector) in [
        ("mode1.bin", &raw, RAW_SECTOR_BYTES),
        ("cooked.iso", &iso, 512),
    ] {
        let path = temp.path().join(name);
        std::fs::write(&path, source).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
        assert_eq!(image.sector_size, expected_sector);
        assert_eq!(image.len(), source.len() as u64);
        assert_eq!(image.read_range(0, source.len()).unwrap(), *source);
        assert_eq!(image.partitions.len(), 1);
        assert_eq!(image.partitions[0].kind, PartitionKind::Cdfs);
        assert_eq!(
            image.partitions[0].length_lba * image.sector_size as u64,
            source.len() as u64
        );
        let mut session = FsSession::new(&image);
        let children = session.children("part:0").unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "测试数据.bin");
        let id = &children[0].id;
        assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), content);
        assert_eq!(session.file_stat(id).unwrap().size, content.len() as u64);
        for (offset, length) in [(2043, 17), (4091, 41), (6998, 20), (7001, 1)] {
            let start = (offset as usize).min(content.len());
            let end = start.saturating_add(length).min(content.len());
            assert_eq!(
                session.read_file_range(id, offset, length).unwrap(),
                content[start..end]
            );
        }
        // The public session rejects overflowing ranges before dispatch.
        assert!(session.read_file_range(id, u64::MAX, 20).is_err());
        let mut streamed = Vec::new();
        let count = fs_tree::stream_file_chunks(&image, id, 2047, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, content);
        assert_eq!(count, content.len() as u64);
        let export = temp.path().join(format!("{name}.export"));
        assert_eq!(
            fs_tree::export_file(&image, id, &export).unwrap(),
            content.len() as u64
        );
        assert_eq!(std::fs::read(export).unwrap(), content);
    }
}

#[test]
fn raw_cd_mode1_rejects_truncated_geometry_and_corrupt_headers() {
    let (mut raw, _, _) = fixture();
    raw.pop();
    assert!(Mode1Layout::probe(&BufSource(raw)).is_err());
    let (mut raw, _, _) = fixture();
    raw[16 * RAW_SECTOR_BYTES + 16 + 84] ^= 1;
    assert!(Mode1Layout::probe(&BufSource(raw)).is_err());
    let (mut raw, _, _) = fixture();
    let source = BufSource(raw.clone());
    let layout = Mode1Layout::probe(&source).unwrap().unwrap();
    raw[25 * RAW_SECTOR_BYTES] = 42;
    assert!(layout
        .read(&BufSource(raw), 25 * DATA_BYTES as u64, 10)
        .is_err());
    let (mut raw, _, _) = fixture();
    raw[25 * RAW_SECTOR_BYTES + 14] = 0x7f;
    assert!(layout
        .read(&BufSource(raw), 25 * DATA_BYTES as u64, 10)
        .is_err());
}

#[test]
fn raw_cd_mode1_file_extent_beyond_volume_fails_without_synthetic_content() {
    let (mut raw, _, _) = fixture();
    let file_record = 21 * RAW_SECTOR_BYTES + 16 + 68;
    both32(&mut raw, file_record + 2, 39);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invalid.bin");
    std::fs::write(&path, raw).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    let mut session = FsSession::new(&image);
    let files = session.children("part:0").unwrap();
    assert!(session.read_file_limited(&files[0].id, 10000).is_err());
    let mut count = 0;
    assert!(
        fs_tree::stream_file_chunks(&image, &files[0].id, 1024, |chunk| {
            count += chunk.len();
            Ok(())
        })
        .is_err()
    );
    assert!(count < 7001);
    let export = temp.path().join("invalid.export");
    assert!(fs_tree::export_file(&image, &files[0].id, &export).is_err());
    assert!(!export.exists());
}

#[test]
#[ignore = "requires LOVELYCARVE_RAW_CD_IMAGE pointing to an external MODE1/2352 ISO 9660 image"]
fn raw_cd_mode1_real_image_lists_and_reads_files() {
    let path = std::env::var_os("LOVELYCARVE_RAW_CD_IMAGE").expect("LOVELYCARVE_RAW_CD_IMAGE");
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.sector_size, RAW_SECTOR_BYTES);
    assert_eq!(image.partitions[0].kind, PartitionKind::Cdfs);
    let mut session = FsSession::new(&image);
    let root = session.children("part:0").unwrap();
    let data = root
        .iter()
        .find(|node| node.name == "data" && node.kind == NodeKind::Dir)
        .unwrap();
    let mut selected = None;
    session
        .walk_visit_from(&data.id, &mut |entry| {
            if entry.node.kind == NodeKind::File && entry.node.size > 4096 {
                selected = Some(entry.node.clone());
                return Ok(false);
            }
            Ok(true)
        })
        .unwrap();
    let file = selected.expect("non-empty data file");
    let bytes = session.read_file_limited(&file.id, 8192).unwrap();
    assert_eq!(bytes.len(), 8192.min(file.size as usize));
    assert_eq!(
        session.read_file_range(&file.id, 2041, 32).unwrap(),
        bytes[2041..2073]
    );
}

#[test]
fn raw_cd_mode1_rejects_declared_volume_overflow_and_source_short_reads() {
    let (mut raw, _, _) = fixture();
    both32(&mut raw, 16 * RAW_SECTOR_BYTES + 16 + 80, 41);
    assert!(Mode1Layout::probe(&BufSource(raw)).is_err());
    struct Short(BufSource);
    impl SectorSource for Short {
        fn size(&self) -> u64 {
            self.0.size() + 1
        }
        fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
            self.0.read(offset, length)
        }
    }
    let (mut raw, _, _) = fixture();
    raw.pop();
    let short = Short(BufSource(raw));
    let layout = Mode1Layout::probe(&short).unwrap().unwrap();
    assert!(layout.read(&short, 39 * DATA_BYTES as u64, 1).is_err());
}

#[test]
fn raw_cd_mode1_does_not_read_files_from_trailing_track_padding() {
    let (mut raw, _, _) = fixture();
    let mut padding = raw[39 * RAW_SECTOR_BYTES..].to_vec();
    padding[14] = 0x40;
    raw.extend(padding);
    both32(&mut raw, 21 * RAW_SECTOR_BYTES + 16 + 68 + 2, 39);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("padding.bin");
    std::fs::write(&path, raw).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.partitions[0].length_lba, 41);
    let mut session = FsSession::new(&image);
    let files = session.children("part:0").unwrap();
    assert!(session
        .read_file_range(&files[0].id, DATA_BYTES as u64, 1)
        .is_err());
}
