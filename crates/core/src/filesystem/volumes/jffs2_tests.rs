use super::*;
use crate::evidence::image::PartitionKind;
use crate::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{stream_file_chunks, FsSession},
};
use std::path::Path;

fn verify(image: &Arc<EvidenceSession>, expected: &[u8]) {
    let mut session = FsSession::new(image);
    let entries = session.children("part:0").unwrap();
    assert_eq!(entries.len(), 6);
    assert!(!entries.iter().any(|entry| entry.name == "deleted.txt"));
    let payload = entries
        .iter()
        .find(|entry| entry.name == "payload.bin")
        .unwrap();
    assert_eq!(payload.size, 9001);
    assert_eq!(
        payload.timestamps.accessed.as_deref(),
        Some("2023-11-14T22:13:21Z")
    );
    assert_eq!(
        payload.timestamps.modified.as_deref(),
        Some("2023-11-14T22:13:22Z")
    );
    assert_eq!(
        payload.timestamps.mft_changed.as_deref(),
        Some("2023-11-14T22:13:23Z")
    );
    assert!(payload.timestamps.created.is_none());
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap()
            .link_target
            .as_deref(),
        Some("payload.bin")
    );
    for path in ["/payload.bin", "/alias.bin", "/子目录/tail.txt"] {
        let id = node_id(0, path);
        assert_eq!(
            session.read_file_limited(&id, usize::MAX).unwrap(),
            expected
        );
        assert_eq!(
            crate::filesystem::fs_dispatch::volume_root_key(&id),
            Some("jffs2:part:0".into())
        );
    }
    for (offset, length) in [
        (6u64, 17usize),
        (4080, 100),
        (8180, 100),
        (8990, 100),
        (9002, 100),
    ] {
        let start = (offset as usize).min(expected.len());
        assert_eq!(
            session
                .read_file_range(&payload.id, offset, length)
                .unwrap(),
            expected[start..(start + length).min(expected.len())]
        );
    }
    let mut bytes = Vec::new();
    stream_file_chunks(image, &payload.id, 317, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(bytes, expected);
    let cut = node_id(0, "/cut.txt");
    assert_eq!(session.read_file_limited(&cut, 100).unwrap(), b"01234");
}
#[test]
fn jffs2_host_both_endian_codecs_and_firmware_preserve_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = std::fs::read(root.join("../../vendor/jffs2/test_data/payload.bin")).unwrap();
    let mut paths = Vec::new();
    for order in ["le", "be"] {
        for codec in ["stored", "rtime", "zlib", "lzo"] {
            paths.push(root.join(format!(
                "../../vendor/jffs2/test_data/synthetic-{order}-{codec}.jffs2"
            )));
        }
    }
    paths.push(root.join("test_data/firmware/synthetic-jffs2.enc"));
    for path in paths {
        let original = std::fs::read(&path).unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        assert_eq!(image.partitions[0].kind, PartitionKind::Jffs2);
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        verify(&image, &expected);
    }
}
#[test]
fn jffs2_host_node_paths_are_validated_and_partition_scoped() {
    assert_eq!(
        parse_jffs2_id(&node_id(7, "/子目录/tail.txt")),
        Some((7, "/子目录/tail.txt".into()))
    );
    for path in ["/../escape", "/a\\b", "/a\0b", "relative", "/a//b"] {
        assert!(parse_jffs2_id(&node_id(0, path)).is_none());
    }
    assert_ne!(node_id(0, "/file"), node_id(1, "/file"));
    assert!(parse_jffs2_id("jffs2:part:256:00").is_none());
}
#[test]
#[ignore = "set LOVELYCARVE_JFFS2_IMAGES to semicolon-separated public Unblob fruits images"]
fn jffs2_host_external_independent_outputs() {
    for path in std::env::var("LOVELYCARVE_JFFS2_IMAGES")
        .unwrap()
        .split(';')
    {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.partitions[0].kind, PartitionKind::Jffs2);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let mut session = FsSession::new(&image);
        let entries = session.children("part:0").unwrap();
        assert_eq!(entries.len(), 3);
        for (name, expected) in [
            ("apple.txt", b"AAAAAAAAAAAAAAAAAAAAAAAAA\n".as_slice()),
            ("banana.txt", b"banana\n"),
            ("cherry.txt", b"cherry\n"),
        ] {
            let entry = entries.iter().find(|entry| entry.name == name).unwrap();
            assert_eq!(session.read_file_limited(&entry.id, 100).unwrap(), expected);
            assert_eq!(
                session.read_file_range(&entry.id, 2, 4).unwrap(),
                expected[2..6]
            );
            let mut streamed = Vec::new();
            stream_file_chunks(&image, &entry.id, 4096, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(streamed, expected);
        }
    }
}
