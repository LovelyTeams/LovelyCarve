use super::*;
use crate::evidence::image::PartitionKind;
use crate::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{self, FsSession},
};
use std::io::Read;

fn decompress(bytes: &[u8]) -> Vec<u8> {
    let mut image = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut image)
        .unwrap();
    image
}
fn expected(zone_size: usize, pointers: usize) -> Vec<u8> {
    (0..(7 + pointers + 2) * zone_size + 37)
        .map(|index| {
            let zone = index / zone_size;
            let factor = if zone == 0 {
                13
            } else if zone == 2 || zone == 7 {
                17
            } else if zone == 7 + pointers + 2 {
                19
            } else {
                return 0;
            };
            (((index % zone_size) * factor + 7) & 255) as u8
        })
        .collect()
}
fn check_file(image: &Arc<EvidenceSession>, id: &str, expected: &[u8]) {
    let mut session = FsSession::new(image);
    assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), expected);
    for offset in [
        0,
        1019,
        7161,
        expected.len() - 7,
        expected.len(),
        expected.len() + 7,
    ] {
        assert_eq!(
            session.read_file_range(id, offset as u64, 123).unwrap(),
            expected[offset.min(expected.len())..(offset + 123).min(expected.len())]
        );
    }
    let mut actual = Vec::new();
    assert_eq!(
        fs_tree::stream_file_chunks(image, id, 8191, |chunk| {
            actual.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap(),
        expected.len() as u64
    );
    assert_eq!(actual, expected);
    assert_eq!(session.file_stat(id).unwrap().size, expected.len() as u64);
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file.bin");
    fs_tree::export_file_with_fault_report(image, id, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), expected);
    assert!(fs_tree::export_file_with_fault_report(image, id, &target).is_err());
    assert_eq!(std::fs::read(target).unwrap(), expected);
}

#[test]
fn minix_native_versions_endian_multiblock_zones_tree_range_stream_export() {
    for (compressed, zone_size, pointers) in [
        (
            include_bytes!(
                "../../../../../vendor/minix-reader/tests/fixtures/v1-14-1024-0-le.zlib"
            )
            .as_slice(),
            1024,
            512,
        ),
        (
            include_bytes!(
                "../../../../../vendor/minix-reader/tests/fixtures/v1-30-1024-1-be.zlib"
            )
            .as_slice(),
            2048,
            512,
        ),
        (
            include_bytes!(
                "../../../../../vendor/minix-reader/tests/fixtures/v2-30-1024-2-le.zlib"
            )
            .as_slice(),
            4096,
            256,
        ),
        (
            include_bytes!(
                "../../../../../vendor/minix-reader/tests/fixtures/v3-60-4096-0-le.zlib"
            )
            .as_slice(),
            4096,
            1024,
        ),
    ] {
        let bytes = decompress(compressed);
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("minix.img");
        std::fs::write(&source, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(&source).unwrap()));
        assert_eq!(image.partitions[0].kind, PartitionKind::Minix);
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
        let mut session = FsSession::new(&image);
        let root = session.children("part:0").unwrap();
        let big = root.iter().find(|node| node.name == "big.bin").unwrap();
        let alias = root.iter().find(|node| node.name == "alias.bin").unwrap();
        assert_eq!(big.inode, alias.inode);
        assert_ne!(big.id, alias.id);
        check_file(&image, &big.id, &expected(zone_size, pointers));
        let link = root.iter().find(|node| node.name == "link").unwrap();
        assert_eq!(link.link_target.as_deref(), Some("big.bin"));
        assert_eq!(
            session.read_file_range(&link.id, 0, 64).unwrap(),
            b"big.bin"
        );
        let sub = root.iter().find(|node| node.name == "sub").unwrap();
        let nested = session.children(&sub.id).unwrap();
        assert_eq!(nested[0].name, "nested.txt");
        assert_eq!(
            session.read_file_limited(&nested[0].id, 64).unwrap(),
            b"hello minix"
        );
        assert!(big.timestamps.modified.is_some());
        assert_eq!(big.timestamps.accessed.is_some(), pointers != 512);
    }
}

#[test]
fn minix_native_mbr_partition_refinement_and_volume_identity() {
    let first = decompress(include_bytes!(
        "../../../../../vendor/minix-reader/tests/fixtures/v1-14-1024-0-le.zlib"
    ));
    let second = decompress(include_bytes!(
        "../../../../../vendor/minix-reader/tests/fixtures/v3-60-1024-0-le.zlib"
    ));
    let start = 1024 * 1024;
    let second_start = start + first.len();
    let mut bytes = vec![0; second_start + second.len()];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    for (slot, offset, source) in [(0, start, &first), (1, second_start, &second)] {
        let entry = 446 + slot * 16;
        bytes[entry + 4] = 0x81; // preserve raw Minix MBR type, refine from superblock
        bytes[entry + 8..entry + 12].copy_from_slice(&((offset / 512) as u32).to_le_bytes());
        bytes[entry + 12..entry + 16].copy_from_slice(&((source.len() / 512) as u32).to_le_bytes());
        bytes[offset..offset + source.len()].copy_from_slice(source);
    }
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("two-volumes.img");
    std::fs::write(&source, bytes).unwrap();
    let image = Arc::new(EvidenceSession::from(DiskImage::open(&source).unwrap()));
    assert_eq!(
        image
            .partitions
            .iter()
            .map(|partition| partition.kind)
            .collect::<Vec<_>>(),
        vec![PartitionKind::Minix, PartitionKind::Minix]
    );
    for (partition, pointers) in [(0, 512), (1, 256)] {
        let id = format!("minix:part:{partition}:{}", hex::encode("/big.bin"));
        check_file(&image, &id, &expected(1024, pointers));
    }
}
