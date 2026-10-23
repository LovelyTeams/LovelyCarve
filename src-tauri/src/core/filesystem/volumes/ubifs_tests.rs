use super::*;
use crate::core::evidence::image::PartitionKind;
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{stream_file_chunks, FsSession},
};
use std::path::Path;

fn verify(image: &Arc<EvidenceSession>, source: UbifsSource, expected: &[u8]) {
    let mut session = FsSession::new(image);
    let entries = session.children(&source.node_id("")).unwrap();
    assert_eq!(entries.len(), 7);
    let payload = entries
        .iter()
        .find(|entry| entry.name == "payload.bin")
        .unwrap();
    assert_eq!(payload.size, 9001);
    assert_eq!(payload.inode, Some(2));
    assert_eq!(
        payload.timestamps.accessed.as_deref(),
        Some("1969-12-31T23:57:57.123456789Z")
    );
    assert!(payload.timestamps.created.is_none());
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "relative-link")
            .unwrap()
            .link_target
            .as_deref(),
        Some("payload.bin")
    );
    assert_eq!(
        session.read_file_limited(&payload.id, usize::MAX).unwrap(),
        expected
    );
    for name in ["alias.bin", "Ak", "B`", "子目录/tail.txt"] {
        assert_eq!(
            session
                .read_file_limited(&source.node_id(&format!("/{name}")), usize::MAX)
                .unwrap(),
            expected
        );
    }
    for (offset, length) in [(4080u64, 100usize), (8180, 100), (8990, 100), (9002, 100)] {
        let start = (offset as usize).min(expected.len());
        let end = (start + length).min(expected.len());
        assert_eq!(
            session
                .read_file_range(&payload.id, offset, length)
                .unwrap(),
            expected[start..end]
        );
    }
    let mut streamed = Vec::new();
    stream_file_chunks(image, &payload.id, 1000, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);
    assert!(session.read_file_range(&payload.id, u64::MAX, 10).is_err());
}
#[test]
fn ubifs_host_bare_firmware_and_ubi_sources_preserve_bytes_and_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = std::fs::read(root.join("vendor/ubidump/test_data/payload.bin")).unwrap();
    for name in ["stored", "lzo", "deflate", "zstd", "zstd-compressed"] {
        let path = root.join(format!("vendor/ubidump/test_data/synthetic-{name}.ubifs"));
        let original = std::fs::read(&path).unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.partitions[0].kind, PartitionKind::Ubifs);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        verify(&image, UbifsSource::Partition(0), &expected);
    }
    let original = std::fs::read(root.join("test_data/firmware/synthetic-ubifs.enc")).unwrap();
    let path = root.join("test_data/firmware/synthetic-ubifs.enc");
    let image = Arc::new(EvidenceSession::open(&path).unwrap());
    assert_eq!(image.read_range(0, original.len()).unwrap(), original);
    assert_eq!(image.partitions[0].kind, PartitionKind::Ubifs);
    verify(&image, UbifsSource::Partition(0), &expected);
    let payload =
        std::fs::read(root.join("vendor/ubidump/test_data/synthetic-deflate.ubifs")).unwrap();
    let original = super::super::ubi::tests::ubi_fixture(&payload);
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("reordered.ubi");
    std::fs::write(&path, &original).unwrap();
    let image = Arc::new(EvidenceSession::open(&path).unwrap());
    assert_eq!(image.read_range(0, original.len()).unwrap(), original);
    let entries = FsSession::new(&image).children("part:0").unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.kind == NodeKind::Dir)
            .count(),
        1
    );
    let source = UbifsSource::Ubi {
        part_id: 0,
        volume_id: 2,
    };
    verify(&image, source, &expected);
    assert_eq!(
        crate::core::filesystem::fs_dispatch::volume_root_key(&source.node_id("/payload.bin")),
        Some(source.cache_key())
    );
    assert_ne!(
        source.cache_key(),
        UbifsSource::Ubi {
            part_id: 0,
            volume_id: 3
        }
        .cache_key()
    );
}

#[test]
#[ignore = "set LOVELYCARVE_UBIFS_IMAGES to semicolon-separated public Unblob UBIFS inputs"]
fn ubifs_host_external_independent_outputs() {
    for path in std::env::var("LOVELYCARVE_UBIFS_IMAGES")
        .unwrap()
        .split(';')
    {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.partitions[0].kind, PartitionKind::Ubifs);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let mut pending = vec!["part:0".to_string()];
        let mut contents = std::collections::BTreeSet::new();
        let mut session = FsSession::new(&image);
        while let Some(parent) = pending.pop() {
            for entry in session.children(&parent).unwrap() {
                if entry.kind == NodeKind::Dir {
                    pending.push(entry.id);
                } else {
                    let bytes = session.read_file_limited(&entry.id, 4096).unwrap();
                    let mut stream = Vec::new();
                    stream_file_chunks(&image, &entry.id, 4096, |chunk| {
                        stream.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(bytes, stream);
                    assert_eq!(
                        session.read_file_range(&entry.id, 2, 3).unwrap(),
                        bytes[2..5]
                    );
                    contents.insert(bytes);
                }
            }
        }
        assert_eq!(
            contents,
            std::collections::BTreeSet::from([
                b"banana1\n".to_vec(),
                b"banana2\n".to_vec(),
                b"banana3\n".to_vec(),
                b"banana4\n".to_vec()
            ])
        );
    }
}
