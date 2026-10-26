use super::*;
use crate::core::{
    evidence::image::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use std::io::Read;

#[test]
fn bcachefs_bare_mbr_backup_and_snapshot_views_keep_native_identity_and_bytes() {
    let mut volume = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../../../../vendor/bcachefs-reader/tests/fixtures/invented.zlib")
            .as_slice(),
    )
    .read_to_end(&mut volume)
    .unwrap();
    for (embedded, backup_only) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut bytes = if embedded {
            vec![0; 1024 * 1024]
        } else {
            Vec::new()
        };
        if embedded {
            bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
            bytes[450] = 0x83;
            bytes[454..458].copy_from_slice(&2048u32.to_le_bytes());
            bytes[458..462].copy_from_slice(&((volume.len() / 512) as u32).to_le_bytes());
        }
        let base = bytes.len();
        bytes.extend_from_slice(&volume);
        if backup_only {
            bytes[base + 4096] ^= 1;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bcachefs.img");
        std::fs::write(&path, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Bcachefs);
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        assert_eq!(root.len(), 10);
        let hello = root.iter().find(|node| node.name == "hello.txt").unwrap();
        let alias = root.iter().find(|node| node.name == "alias.txt").unwrap();
        assert_eq!(hello.inode, Some(5000));
        assert_eq!(hello.inode, alias.inode);
        assert_ne!(hello.id, alias.id);
        assert_eq!(
            hello.timestamps.accessed.as_deref(),
            Some("1960-01-01T00:00:00.123456789Z")
        );
        assert_eq!(
            hello.timestamps.created.as_deref(),
            Some("2017-07-14T02:40:00.111222333Z")
        );
        assert_eq!(
            session.read_file_limited(&hello.id, 100).unwrap(),
            b"Pure Rust bcachefs evidence\n"
        );
        let mut streamed = Vec::new();
        fs_tree::stream_file_chunks(&image, &hello.id, 4096, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, b"Pure Rust bcachefs evidence\n");
        for (name, expected) in [("sv", b"LIVE"), ("snap", b"PAST")] {
            let node = root.iter().find(|node| node.name == name).unwrap();
            assert_eq!(node.inode, Some(6000));
            let children = session.children(&node.id).unwrap();
            assert_eq!(children.len(), 1);
            assert_eq!(
                session.read_file_limited(&children[0].id, 100).unwrap(),
                expected
            );
        }
        let checked = root
            .iter()
            .find(|node| node.name == "checksum.bin")
            .unwrap();
        let shared = root.iter().find(|node| node.name == "reflink.bin").unwrap();
        let full = session.read_file_limited(&checked.id, 1024).unwrap();
        assert_eq!(full, session.read_file_limited(&shared.id, 1024).unwrap());
        assert_eq!(
            session.read_file_range(&checked.id, 499, 77).unwrap(),
            full[499..]
        );
        let sparse = root.iter().find(|node| node.name == "sparse.bin").unwrap();
        assert_eq!(
            session
                .read_file_range(&sparse.id, 5 * 1024u64.pow(3), 5)
                .unwrap(),
            b"TAIL!"
        );
        assert!(parse_bcachefs_id("bcachefs:part:0:2f2e2e2f61").is_none());
        assert_eq!(
            parse_bcachefs_id("bcachefs:part:0:2f615c62").unwrap().1,
            "/a\\b"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
