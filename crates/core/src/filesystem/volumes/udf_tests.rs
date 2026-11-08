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
fn check_file(image: &Arc<EvidenceSession>, id: &str, expected: &[u8]) {
    let mut session = FsSession::new(image);
    assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), expected);
    for offset in [
        0,
        507,
        2041,
        expected.len().saturating_sub(3),
        expected.len(),
        expected.len() + 7,
    ] {
        assert_eq!(
            session.read_file_range(id, offset as u64, 117).unwrap(),
            expected[offset.min(expected.len())..(offset + 117).min(expected.len())]
        );
    }
    let mut actual = Vec::new();
    assert_eq!(
        fs_tree::stream_file_chunks(image, id, 4093, |chunk| {
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
    assert_eq!(std::fs::read(target).unwrap(), expected);
}

#[test]
fn udf_native_reference_and_sparse_tree_ranges_stream_and_export() {
    for (compressed, path, expected) in [
        (
            include_bytes!("../../../../../vendor/lamfold-udf/tests/fixtures/udf102.zlib")
                .as_slice(),
            "/big.dat",
            (0..5000)
                .map(|index| (index % 256) as u8)
                .collect::<Vec<_>>(),
        ),
        (
            include_bytes!("../../../../../vendor/lamfold-udf/tests/fixtures/udf201.zlib")
                .as_slice(),
            "/big.bin",
            (0..70000)
                .map(|index| ((index * 11 + index / 257) & 255) as u8)
                .collect(),
        ),
        (
            include_bytes!("../../../../../vendor/lamfold-udf/tests/fixtures/synthetic-512.zlib")
                .as_slice(),
            "/big.bin",
            (0..2085)
                .map(|index| {
                    if index < 512 {
                        ((index * 17 + 3) & 255) as u8
                    } else if index < 1536 {
                        0
                    } else {
                        (((index - 1536) * 19 + 5) & 255) as u8
                    }
                })
                .collect(),
        ),
    ] {
        let bytes = decompress(compressed);
        let directory = tempfile::tempdir().unwrap();
        let path_on_host = directory.path().join("source.iso");
        std::fs::write(&path_on_host, &bytes).unwrap();
        let evidence = Arc::new(EvidenceSession::from(
            DiskImage::open(&path_on_host).unwrap(),
        ));
        assert_eq!(evidence.partitions[0].kind, PartitionKind::Udf);
        assert_eq!(evidence.read_range(0, bytes.len()).unwrap(), bytes);
        let mut session = FsSession::new(&evidence);
        let root = session.children("part:0").unwrap();
        assert!(root.iter().any(|entry| entry.name == "sub"));
        assert!(root.iter().any(|entry| entry.timestamps.modified.is_some()));
        let id = format!("udf:part:0:{}", hex::encode(path));
        check_file(&evidence, &id, &expected);
        if root.iter().any(|entry| entry.name == "huge.bin") {
            let link = root.iter().find(|entry| entry.name == "link").unwrap();
            assert_eq!(link.link_target.as_deref(), Some("../big.bin"));
            check_file(&evidence, &link.id, b"../big.bin");
            let huge = root.iter().find(|entry| entry.name == "huge.bin").unwrap();
            assert_eq!(
                session
                    .read_file_range(&huge.id, huge.size - 11, 111)
                    .unwrap(),
                [0; 11]
            );
            let unknown = root
                .iter()
                .find(|entry| entry.name == "unknown.txt")
                .unwrap();
            assert!(unknown.timestamps.modified.is_none());
        }
    }
}

#[test]
fn udf_native_mbr_refinement_and_volume_identity() {
    let first = decompress(include_bytes!(
        "../../../../../vendor/lamfold-udf/tests/fixtures/udf102.zlib"
    ));
    let second = decompress(include_bytes!(
        "../../../../../vendor/lamfold-udf/tests/fixtures/udf201.zlib"
    ));
    let start = 1024 * 1024;
    let second_start = start + first.len();
    let mut bytes = vec![0; second_start + second.len()];
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    for (slot, offset, image) in [(0, start, &first), (1, second_start, &second)] {
        let entry = 446 + slot * 16;
        bytes[entry + 4] = 0x83;
        bytes[entry + 8..entry + 12].copy_from_slice(&((offset / 512) as u32).to_le_bytes());
        bytes[entry + 12..entry + 16].copy_from_slice(&((image.len() / 512) as u32).to_le_bytes());
        bytes[offset..offset + image.len()].copy_from_slice(image);
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
        vec![PartitionKind::Udf, PartitionKind::Udf]
    );
    let mut session = FsSession::new(&image);
    for (partition, expected) in [
        (0, b"udf root file content\n".as_slice()),
        (1, b"udf 2.01 root file\n".as_slice()),
    ] {
        let nodes = session.children(&format!("part:{partition}")).unwrap();
        let readme = nodes.iter().find(|node| node.name == "readme.txt").unwrap();
        assert!(readme.id.starts_with(&format!("udf:part:{partition}:")));
        check_file(&image, &readme.id, expected);
    }
}

#[test]
fn udf_native_bridge_prefers_udf_and_rejects_invalid_ids() {
    let mut bytes = decompress(include_bytes!(
        "../../../../../vendor/lamfold-udf/tests/fixtures/udf102.zlib"
    ));
    bytes[16 * 2048..16 * 2048 + 7].copy_from_slice(b"\x01CD001\x01");
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("bridge.iso");
    std::fs::write(&source, bytes).unwrap();
    let image = DiskImage::open(&source).unwrap();
    assert_eq!(image.partitions[0].kind, PartitionKind::Udf);
    for id in [
        "udf:part:256:",
        "udf:part:0:2e2e",
        "udf:part:0:2f2e2e",
        "udf:part:0:2f610062",
        "udf:part:0:ff",
    ] {
        assert!(parse_udf_id(id).is_none());
    }
    assert_eq!(
        parse_udf_id("udf:part:0:2f737562"),
        Some((0, "/sub".into()))
    );
}
