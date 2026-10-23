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
fn nilfs2_bare_mbr_and_backup_views_preserve_paths_bytes_and_recorded_time_roles() {
    for (block, packed) in [
        (
            1024,
            include_bytes!("../../../../vendor/nilfs2-reader/tests/fixtures/invented-1024.zlib")
                .as_slice(),
        ),
        (
            4096,
            include_bytes!("../../../../vendor/nilfs2-reader/tests/fixtures/invented-4096.zlib")
                .as_slice(),
        ),
        (
            65536,
            include_bytes!("../../../../vendor/nilfs2-reader/tests/fixtures/invented-65536.zlib")
                .as_slice(),
        ),
    ] {
        let mut volume = Vec::new();
        flate2::read::ZlibDecoder::new(packed)
            .read_to_end(&mut volume)
            .unwrap();
        for (embedded, backup_only) in [(false, false), (true, false), (true, true)] {
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
                bytes[base + 1024 + 16] ^= 1;
            }
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("nilfs2.img");
            std::fs::write(&path, &bytes).unwrap();
            let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
            assert_eq!(image.partitions[0].kind, PartitionKind::Nilfs2);
            let mut session = FsSession::new(&image);
            let root = session.children("part:0").unwrap();
            assert_eq!(root.len(), 9);
            let hello = root.iter().find(|node| node.name == "hello").unwrap();
            let alias = root.iter().find(|node| node.name == "alias").unwrap();
            assert_eq!(hello.inode, alias.inode);
            assert_ne!(hello.id, alias.id);
            assert!(hello.timestamps.created.is_none() && hello.timestamps.accessed.is_none());
            assert_eq!(
                hello.timestamps.modified.as_deref(),
                Some("1960-01-01T00:00:00.000000456Z")
            );
            assert_eq!(
                session.read_file_limited(&hello.id, 100).unwrap(),
                b"hello NILFS2 evidence\n"
            );
            let mut streamed = Vec::new();
            fs_tree::stream_file_chunks(&image, &hello.id, 4096, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(streamed, b"hello NILFS2 evidence\n");
            let big = root.iter().find(|node| node.name == "big").unwrap();
            let full = session.read_file_limited(&big.id, block * 8).unwrap();
            assert_eq!(
                session
                    .read_file_range(&big.id, (block * 4 - 9) as u64, 77)
                    .unwrap(),
                full[block * 4 - 9..block * 4 + 68]
            );
            let far = root.iter().find(|node| node.name == "far").unwrap();
            assert_eq!(
                session.read_file_limited(&far.id, 100).unwrap(),
                b"second inode allocation group"
            );
            assert!(parse_nilfs2_id("nilfs2:part:0:2f2e2e2f61").is_none());
            assert_eq!(
                parse_nilfs2_id("nilfs2:part:0:2f615c62").unwrap().1,
                "/a\\b"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
