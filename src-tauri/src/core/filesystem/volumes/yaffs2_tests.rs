use super::*;
use crate::core::{
    evidence::image::PartitionKind,
    filesystem::{
        evidence_session::{EvidenceSession, ImageExportMode},
        tree::fs_tree::{stream_file_chunks, FsSession},
    },
};
use std::path::Path;

#[test]
fn yaffs2_host_endians_firmware_identity_metadata_stream_and_range() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = b"Native YAFFS2 fixture.\n".repeat(300);
    for file in [
        "vendor/yaffs2-reader/test_data/base-le.yaffs",
        "vendor/yaffs2-reader/test_data/base-be-ecc.yaffs",
        "test_data/firmware/synthetic-yaffs2.enc",
    ] {
        let path = root.join(file);
        let original = std::fs::read(&path).unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        assert_eq!(image.partitions[0].kind, PartitionKind::Yaffs2);
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let exported = image.raw_export_reader(ImageExportMode::Source).unwrap();
        let mut bytes = vec![0; original.len()];
        assert_eq!(exported.read_into(0, &mut bytes).unwrap(), bytes.len());
        assert_eq!(bytes, original);
        let mut session = FsSession::new(&image);
        let children = session.children("part:0").unwrap();
        assert_eq!(children.len(), 5);
        assert!(!children
            .iter()
            .any(|node| node.name == "gone" || node.kind == NodeKind::Unallocated));
        let payload = node_id(0, "/目录/hello");
        assert_eq!(
            crate::core::filesystem::fs_dispatch::volume_root_key(&payload),
            Some("yaffs2:part:0".into())
        );
        for id in [&payload, &node_id(0, "/alias")] {
            assert_eq!(session.read_file_limited(id, usize::MAX).unwrap(), expected);
            assert_eq!(
                session.read_file_range(id, 2047, 4097).unwrap(),
                expected[2047..6144]
            );
            let mut streamed = Vec::new();
            stream_file_chunks(&image, id, 317, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(streamed, expected);
            let stat = session.file_stat(id).unwrap();
            assert_eq!(stat.inode, Some(257));
            assert!(stat.data_runs.is_empty());
        }
        assert_eq!(
            children
                .iter()
                .find(|node| node.name == "alias")
                .unwrap()
                .timestamps
                .modified
                .as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(
            children
                .iter()
                .find(|node| node.name == "link")
                .unwrap()
                .link_target
                .as_deref(),
            Some("目录/hello")
        );
        assert!(session
            .read_file_limited(&node_id(0, "/console"), 100)
            .is_err());
        assert_eq!(
            session.read_file_limited(&node_id(0, "/empty"), 1).unwrap(),
            b""
        );
    }
}
#[test]
fn yaffs2_host_node_identity_validation_and_sparse_large_seek() {
    for path in ["/../escape", "/a\\b", "/a\0b", "relative", "/a//b"] {
        assert!(parse_yaffs2_id(&node_id(0, path)).is_none());
    }
    assert!(parse_yaffs2_id("yaffs2:part:256:00").is_none());
    assert_ne!(node_id(0, "/file"), node_id(1, "/file"));
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/yaffs2-reader/test_data/sparse-64bit.yaffs");
    let image = Arc::new(EvidenceSession::open(path).unwrap());
    let mut session = FsSession::new(&image);
    let id = node_id(0, "/long");
    assert_eq!(session.file_stat(&id).unwrap().size, (1 << 32) + 3);
    assert_eq!(
        session.read_file_range(&id, (1 << 32) - 1, 9).unwrap(),
        b"\0end"
    );
}
#[test]
#[ignore = "set LOVELYCARVE_YAFFS2_IMAGE and YAFFS2_ORACLE to the public image and independent extracted directory"]
fn yaffs2_host_external_all_files_match_independent_bytes_stream_range() {
    let path = std::env::var("LOVELYCARVE_YAFFS2_IMAGE").unwrap();
    let original = std::fs::read(&path).unwrap();
    let image = Arc::new(EvidenceSession::open(path).unwrap());
    assert_eq!(image.partitions[0].kind, PartitionKind::Yaffs2);
    assert_eq!(image.read_range(0, original.len()).unwrap(), original);
    let mut session = FsSession::new(&image);
    let entries = session.children("part:0").unwrap();
    assert_eq!(entries.len(), 13);
    for entry in entries {
        let expected =
            std::fs::read(Path::new(&std::env::var("YAFFS2_ORACLE").unwrap()).join(&entry.name))
                .unwrap();
        assert_eq!(
            session.read_file_limited(&entry.id, usize::MAX).unwrap(),
            expected
        );
        let mut streamed = Vec::new();
        stream_file_chunks(&image, &entry.id, 4096, |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed, expected);
        for offset in [0, 1, 2047, 4095, expected.len(), expected.len() + 1] {
            let start = offset.min(expected.len());
            assert_eq!(
                session
                    .read_file_range(&entry.id, offset as u64, 4097)
                    .unwrap(),
                expected[start..(start + 4097).min(expected.len())]
            );
        }
    }
}
