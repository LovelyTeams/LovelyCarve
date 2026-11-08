use super::*;
use crate::{
    evidence::image::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use std::io::Read;
#[test]
fn reiserfs_native_versions_bare_mbr_range_stream_export_and_names() {
    for (fixture, block_size, embedded) in [
        (
            include_bytes!(
                "../../../../../vendor/reiserfs-reader/tests/fixtures/v35-1024-old.zlib"
            )
            .as_slice(),
            1024,
            false,
        ),
        (
            include_bytes!(
                "../../../../../vendor/reiserfs-reader/tests/fixtures/v36-4096-relocated.zlib"
            )
            .as_slice(),
            4096,
            true,
        ),
    ] {
        let mut volume = Vec::new();
        flate2::read::ZlibDecoder::new(fixture)
            .read_to_end(&mut volume)
            .unwrap();
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
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("reiserfs.img");
        std::fs::write(&source, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&source).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Reiserfs);
        assert_eq!(
            image.partitions[0].start_lba,
            if embedded { 2048 } else { 0 }
        );
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        let big = root.iter().find(|node| node.name == "big.bin").unwrap();
        let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
        assert_eq!(alias.inode, big.inode);
        assert_ne!(alias.id, big.id);
        let expected = [
            (0..block_size)
                .map(|i| (i * 17 + 5) as u8)
                .collect::<Vec<_>>(),
            vec![0; block_size],
            (0..block_size).map(|i| (i * 23 + 7) as u8).collect(),
            b"packed tail".to_vec(),
        ]
        .concat();
        assert_eq!(
            session.read_file_limited(&big.id, usize::MAX).unwrap(),
            expected
        );
        assert_eq!(
            session.read_file_limited(&alias.id, usize::MAX).unwrap(),
            expected
        );
        for offset in [
            0,
            block_size - 3,
            2 * block_size - 2,
            3 * block_size - 1,
            expected.len(),
            expected.len() + 5,
        ] {
            assert_eq!(
                session.read_file_range(&big.id, offset as u64, 71).unwrap(),
                expected[offset.min(expected.len())..(offset + 71).min(expected.len())]
            );
        }
        let mut streamed = Vec::new();
        fs_tree::stream_file_chunks(&image, &big.id, 1027, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, expected);
        let output = directory.path().join("export.bin");
        fs_tree::export_file_with_fault_report(&image, &big.id, &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        assert!(fs_tree::export_file_with_fault_report(&image, &big.id, &output).is_err());
        let named = root
            .iter()
            .find(|node| node.name == "中文\\evidence")
            .unwrap();
        assert_eq!(parse_reiserfs_id(&named.id).unwrap().1, "/中文\\evidence");
        assert_eq!(session.read_file_limited(&named.id, 100).unwrap(), b"names");
        let sub = root.iter().find(|node| node.name == "sub").unwrap();
        let children = session.children(&sub.id).unwrap();
        assert_eq!(children[0].inode, Some((88u64 << 32) | 6));
        assert_eq!(
            session.read_file_limited(&children[0].id, 100).unwrap(),
            b"nested"
        );
        assert!(root.iter().any(|node| node.name == ".reiserfs_priv"));
        assert!(!root.iter().any(|node| node.name == "hidden"));
        let link = root.iter().find(|node| node.name == "link").unwrap();
        assert_eq!(link.link_target.as_deref(), Some("big.bin"));
        assert!(big.timestamps.accessed.is_some());
        assert!(big.timestamps.created.is_none());
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(big.timestamps.modified.as_deref().unwrap())
                .unwrap()
                .timestamp(),
            4_000_000_000
        );
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    }
    assert!(
        parse_reiserfs_id(&format!("reiserfs:part:0:{}", hex::encode("/../outside"))).is_none()
    );
}
#[test]
fn reiserfsprogs_native_and_dirty_volume_refusal() {
    let original = zstd::stream::decode_all(
        include_bytes!(
            "../../../../../vendor/reiserfs-reader/tests/fixtures/reiserfsprogs.img.zst"
        )
        .as_slice(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    for dirty in [false, true] {
        let mut bytes = original.clone();
        if dirty {
            bytes[65536 + 50..65536 + 52].copy_from_slice(&2u16.to_le_bytes());
        }
        let source = directory
            .path()
            .join(if dirty { "dirty.img" } else { "clean.img" });
        std::fs::write(&source, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(source).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Reiserfs);
        let mut session = FsSession::new(&image);
        if dirty {
            assert!(
                format!("{:#}", session.children("part:0").unwrap_err()).contains("journal replay")
            );
        } else {
            let root = session.children("part:0").unwrap();
            let big = root.iter().find(|node| node.name == "large.bin").unwrap();
            assert_eq!(
                session.read_file_limited(&big.id, usize::MAX).unwrap(),
                (0..24576).map(|i| (i * 37 + 11) as u8).collect::<Vec<_>>()
            );
        }
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
    }
}
