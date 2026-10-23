use super::*;
use crate::core::{
    evidence::image::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use std::io::Read;

fn inflate(raw: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(raw)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn mbr(volume: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 1024 * 1024 + volume.len()];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    bytes[450] = 0xeb;
    bytes[454..458].copy_from_slice(&2048u32.to_le_bytes());
    bytes[458..462].copy_from_slice(&((volume.len() / 512) as u32).to_le_bytes());
    bytes[1024 * 1024..].copy_from_slice(volume);
    bytes
}
#[test]
fn befs_native_bare_and_mbr_cover_endian_blocks_attributes_and_read_paths() {
    for (packed, block_size) in [
        (
            include_bytes!("../../../../vendor/befs-reader/tests/fixtures/le-1024-2560.zlib")
                .as_slice(),
            1024,
        ),
        (
            include_bytes!("../../../../vendor/befs-reader/tests/fixtures/be-8192-2560.zlib")
                .as_slice(),
            8192,
        ),
        (
            include_bytes!("../../../../vendor/befs-reader/tests/fixtures/le-1024-10000.zlib")
                .as_slice(),
            1024,
        ),
    ] {
        for embedded in [false, true] {
            let raw = inflate(packed);
            let bytes = if embedded { mbr(&raw) } else { raw };
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("beos.img");
            std::fs::write(&path, &bytes).unwrap();
            let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
            assert_eq!(image.partitions[0].kind, PartitionKind::Befs);
            let mut session = FsSession::new(&image);
            let root = session.children("part:0").unwrap();
            assert_eq!(root.len(), 9);
            let big = root.iter().find(|node| node.name == "big.bin").unwrap();
            let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
            assert_eq!(big.inode, alias.inode);
            assert_ne!(big.id, alias.id);
            let expected: Vec<_> = (0..26 * block_size - 37)
                .map(|index| (index * 17 + 5) as u8)
                .collect();
            assert_eq!(
                session.read_file_limited(&big.id, usize::MAX).unwrap(),
                expected
            );
            assert_eq!(
                session.read_file_limited(&alias.id, usize::MAX).unwrap(),
                expected
            );
            for offset in [
                block_size - 7,
                12 * block_size - 9,
                18 * block_size - 11,
                expected.len() - 3,
                expected.len(),
            ] {
                let start = offset.min(expected.len());
                assert_eq!(
                    session.read_file_range(&big.id, offset as u64, 73).unwrap(),
                    expected[start..(start + 73).min(expected.len())]
                );
            }
            let mut streamed = Vec::new();
            fs_tree::stream_file_chunks(&image, &big.id, 4091, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(streamed, expected);
            assert_eq!(
                big.timestamps.created.as_deref(),
                Some("1960-01-01T00:00:00Z")
            );
            assert_eq!(
                big.timestamps.modified.as_deref(),
                Some("2023-11-14T22:13:20Z")
            );
            assert!(big.timestamps.mft_changed.is_none());
            let streams = fs_tree::list_data_streams(&image, &big.id).unwrap();
            assert_eq!(
                streams
                    .iter()
                    .map(|stream| stream.name.as_str())
                    .collect::<Vec<_>>(),
                ["", "BEOS:TYPE", "big-attribute", "custom"]
            );
            assert_eq!(
                fs_tree::read_named_stream(&image, &big.id, "custom", 100).unwrap(),
                b"\0\xffinline\x80"
            );
            let attribute: Vec<_> = (0..12003).map(|index| (index * 23 + 7) as u8).collect();
            assert_eq!(
                fs_tree::read_named_stream(&image, &big.id, "big-attribute", 100).unwrap(),
                attribute[..100]
            );
            let output = temp.path().join("attribute.bin");
            assert_eq!(
                fs_tree::export_named_stream(&image, &big.id, "big-attribute", &output).unwrap(),
                12003
            );
            assert_eq!(std::fs::read(&output).unwrap(), attribute);
            assert!(
                fs_tree::export_named_stream(&image, &big.id, "big-attribute", &output).is_err()
            );
            let output = temp.path().join("file.bin");
            fs_tree::export_file_with_fault_report(&image, &big.id, &output).unwrap();
            assert_eq!(std::fs::read(&output).unwrap(), expected);
            assert!(fs_tree::export_file_with_fault_report(&image, &big.id, &output).is_err());
            let sub = root.iter().find(|node| node.name == "sub").unwrap();
            assert_eq!(session.children(&sub.id).unwrap()[0].name, "nested.txt");
            let unicode = root
                .iter()
                .find(|node| node.name == "中文\\evidence")
                .unwrap();
            assert_eq!(
                session.read_file_limited(&unicode.id, 100).unwrap(),
                b"names"
            );
            let link = root.iter().find(|node| node.name == "link").unwrap();
            assert_eq!(link.link_target.as_deref(), Some("sub/nested.txt"));
            assert_eq!(
                session.read_file_limited(&link.id, 100).unwrap(),
                b"sub/nested.txt"
            );
            let pipe = root.iter().find(|node| node.name == "pipe").unwrap();
            assert!(session.read_file_limited(&pipe.id, 100).is_err());
            assert!(
                parse_befs_id(&format!("befs:part:0:{}", hex::encode("/../outside"))).is_none()
            );
            assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
        }
    }
}
#[test]
fn befs_native_dirty_journal_refuses_without_modifying_evidence() {
    let mut bytes = inflate(include_bytes!(
        "../../../../vendor/befs-reader/tests/fixtures/le-1024-2560.zlib"
    ));
    bytes[512 + 84..512 + 88].copy_from_slice(&0x44495254u32.to_le_bytes());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("dirty.img");
    std::fs::write(&path, &bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(&path).unwrap()));
    assert_eq!(image.partitions[0].kind, PartitionKind::Befs);
    assert!(format!(
        "{:#}",
        FsSession::new(&image).children("part:0").unwrap_err()
    )
    .contains("dirty"));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
