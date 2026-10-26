use super::*;
use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
#[test]
fn jfs_native_bare_and_mbr_read_range_stream_export_and_large_directory() {
    let volume = zstd::stream::decode_all(
        include_bytes!("../../../../vendor/jfs-reader/tests/fixtures/jfsutils.img.zst").as_slice(),
    )
    .unwrap();
    for embedded in [false, true] {
        let mut bytes = if embedded {
            vec![0; 1024 * 1024 + volume.len()]
        } else {
            volume.clone()
        };
        if embedded {
            bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
            bytes[450] = 0x83;
            bytes[454..458].copy_from_slice(&2048u32.to_le_bytes());
            bytes[458..462].copy_from_slice(&((volume.len() / 512) as u32).to_le_bytes());
            bytes[1024 * 1024..].copy_from_slice(&volume);
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jfs.img");
        std::fs::write(&path, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Jfs);
        assert_eq!(
            image.partitions[0].start_lba,
            if embedded { 2048 } else { 0 }
        );
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        let large = root.iter().find(|node| node.name == "large.bin").unwrap();
        let expected: Vec<u8> = (0..24 * 1024)
            .map(|index| (index * 37 + 11) as u8)
            .collect();
        assert_eq!(
            session.read_file_limited(&large.id, usize::MAX).unwrap(),
            expected
        );
        for offset in [
            0,
            4091,
            8191,
            expected.len() - 3,
            expected.len(),
            expected.len() + 3,
        ] {
            assert_eq!(
                session
                    .read_file_range(&large.id, offset as u64, 71)
                    .unwrap(),
                expected[offset.min(expected.len())..(offset + 71).min(expected.len())]
            );
        }
        let mut actual = Vec::new();
        fs_tree::stream_file_chunks(&image, &large.id, 4111, |chunk| {
            actual.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected);
        let output = temp.path().join("export.bin");
        fs_tree::export_file_with_fault_report(&image, &large.id, &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        assert!(fs_tree::export_file_with_fault_report(&image, &large.id, &output).is_err());
        let big = root.iter().find(|node| node.name == "bigdir").unwrap();
        let entries = session.children(&big.id).unwrap();
        assert_eq!(entries.len(), 200);
        for index in [0, 31, 32, 127, 128, 199] {
            let name = format!("file_{:03}", index + 1);
            let entry = entries.iter().find(|node| node.name == name).unwrap();
            assert_eq!(
                session.read_file_limited(&entry.id, 100).unwrap(),
                format!("line {:03}", index + 1).as_bytes()
            );
        }
        assert!(large.timestamps.accessed.is_some());
        assert!(large.timestamps.created.is_some());
        let link = root.iter().find(|node| node.name == "link.txt").unwrap();
        assert_eq!(link.link_target.as_deref(), Some("hello.txt"));
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    }
}

#[test]
fn jfs_native_multiple_iags_hardlinks_unix_names_and_sparse_ranges() {
    use std::io::Read;
    for (compressed, block_size) in [
        (
            include_bytes!(
                "../../../../vendor/jfs-reader/tests/fixtures/multi-iag-512-legacy.zlib"
            )
            .as_slice(),
            512,
        ),
        (
            include_bytes!(
                "../../../../vendor/jfs-reader/tests/fixtures/multi-iag-1024-indexed.zlib"
            )
            .as_slice(),
            1024,
        ),
        (
            include_bytes!(
                "../../../../vendor/jfs-reader/tests/fixtures/multi-iag-2048-legacy.zlib"
            )
            .as_slice(),
            2048,
        ),
        (
            include_bytes!(
                "../../../../vendor/jfs-reader/tests/fixtures/multi-iag-4096-indexed.zlib"
            )
            .as_slice(),
            4096,
        ),
    ] {
        let mut bytes = Vec::new();
        flate2::read::ZlibDecoder::new(compressed)
            .read_to_end(&mut bytes)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic.jfs");
        std::fs::write(&path, bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Jfs);
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        let file = root.iter().find(|node| node.name == "big.bin").unwrap();
        let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
        assert_eq!(file.inode, alias.inode);
        assert_ne!(file.id, alias.id);
        assert_eq!(
            session
                .read_file_range(&file.id, (6 * block_size - 7) as u64, 31)
                .unwrap(),
            [0; 31]
        );
        assert_eq!(
            session.read_file_limited(&file.id, usize::MAX).unwrap(),
            session.read_file_limited(&alias.id, usize::MAX).unwrap()
        );
        let name = format!("long_中文_{}\\tail", "x".repeat(180));
        let long = root.iter().find(|node| node.name == name).unwrap();
        assert_eq!(parse_jfs_id(&long.id).unwrap().1, format!("/{name}"));
        assert_eq!(
            session.read_file_limited(&long.id, 100).unwrap(),
            b"unicode path"
        );
        let sub = root.iter().find(|node| node.name == "sub").unwrap();
        let nested = session.children(&sub.id).unwrap();
        assert_eq!(nested[0].inode, Some(4099));
        assert_eq!(
            session.read_file_limited(&nested[0].id, 100).unwrap(),
            b"hello JFS"
        );
        let times = &file.timestamps;
        assert_eq!(times.accessed.as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(times.modified.as_deref().unwrap())
                .unwrap()
                .timestamp(),
            4_000_000_000
        );
        assert!(parse_jfs_id(&format!("jfs:part:0:{}", hex::encode("/../outside"))).is_none());
    }
}

#[test]
fn jfs_dirty_volume_is_identified_and_refused_without_stale_header_fallback() {
    use std::io::Read;
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/jfs-reader/tests/fixtures/multi-iag-4096-indexed.zlib")
            .as_slice(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    bytes[32768 + 40] = 2;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("dirty.jfs");
    std::fs::write(&path, bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
    assert_eq!(image.partitions[0].kind, PartitionKind::Jfs);
    assert!(FsSession::new(&image)
        .children("part:0")
        .unwrap_err()
        .to_string()
        .contains("jfs"));
}
