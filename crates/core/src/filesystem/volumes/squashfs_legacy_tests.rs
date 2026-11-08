use super::*;
use crate::evidence::image::PartitionKind;
use crate::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{stream_file_chunks, FsSession},
};

fn verify(image: &Arc<EvidenceSession>, source: SquashfsSource, expected: &[u8]) {
    let mut session = FsSession::new(image);
    let entries = session.children(&source.node_id("")).unwrap();
    assert_eq!(entries.len(), 10);
    let payload = entries
        .iter()
        .find(|entry| entry.name == "payload.bin")
        .unwrap();
    assert_eq!(payload.size, expected.len() as u64);
    assert_eq!(
        payload.timestamps.modified.as_deref(),
        Some("2023-11-14T22:13:22+00:00")
    );
    assert!(payload.timestamps.created.is_none());
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "empty.txt")
            .unwrap()
            .timestamps
            .modified
            .as_deref(),
        Some("1969-12-31T23:57:57+00:00")
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap()
            .link_target
            .as_deref(),
        Some("payload.bin")
    );
    for name in ["link", "huge-link", "block", "char", "fifo", "socket"] {
        assert!(
            entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .timestamps
                .modified
                .is_none(),
            "{name}"
        );
    }
    for path in ["/payload.bin", "/alias.bin", "/子目录/tail.txt"] {
        assert_eq!(
            session
                .read_file_limited(&source.node_id(path), usize::MAX)
                .unwrap(),
            expected
        );
    }
    for (offset, count) in [
        (4090u64, 100usize),
        (8180, 100),
        (32765, 100),
        (65530, 100),
        (expected.len() as u64, 100),
    ] {
        let start = (offset as usize).min(expected.len());
        assert_eq!(
            session.read_file_range(&payload.id, offset, count).unwrap(),
            expected[start..(start + count).min(expected.len())]
        );
    }
    let mut bytes = Vec::new();
    stream_file_chunks(image, &payload.id, 4097, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(bytes, expected);
}

#[test]
fn squashfs_legacy_host_endian_variants_firmware_and_ubi_keep_source_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let folder = root.join("../../vendor/backhand/test_data/legacy");
    for item in std::fs::read_dir(&folder).unwrap() {
        let path = item.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("squashfs") {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let expected = std::fs::read(folder.join(if name.contains("32k") {
            "payload-32k.bin"
        } else if name.contains("64k") {
            "payload-64k.bin"
        } else {
            "payload.bin"
        }))
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        let image = Arc::new(EvidenceSession::open(&path).unwrap());
        assert_eq!(image.partitions[0].kind, PartitionKind::Squashfs);
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        verify(&image, SquashfsSource::Partition(0), &expected);
    }
    let expected = std::fs::read(folder.join("payload.bin")).unwrap();
    let path = root.join("test_data/firmware/synthetic-legacy-squashfs.enc");
    let original = std::fs::read(&path).unwrap();
    let image = Arc::new(EvidenceSession::open(&path).unwrap());
    assert_eq!(image.read_range(0, original.len()).unwrap(), original);
    verify(&image, SquashfsSource::Partition(0), &expected);
    let payload = std::fs::read(folder.join("v1.0-be-zlib.squashfs")).unwrap();
    let original = super::super::ubi::tests::ubi_fixture(&payload);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy.ubi");
    std::fs::write(&path, &original).unwrap();
    let image = Arc::new(EvidenceSession::open(&path).unwrap());
    assert_eq!(image.read_range(0, original.len()).unwrap(), original);
    verify(
        &image,
        SquashfsSource::Ubi {
            part_id: 0,
            volume_id: 2,
        },
        &expected,
    );
}

#[test]
fn squashfs_legacy_missing_mtime_does_not_hide_a_real_epoch_timestamp() {
    let mut header = NodeHeader {
        mtime: 0,
        mtime_known: true,
        ..NodeHeader::default()
    };
    assert_eq!(
        header_timestamps(&header).modified.as_deref(),
        Some("1970-01-01T00:00:00+00:00")
    );
    header.mtime_known = false;
    assert!(header_timestamps(&header).modified.is_none());
}

#[test]
#[ignore = "set LOVELYCARVE_SQUASHFS_LEGACY to semicolon-separated public legacy inputs"]
fn squashfs_legacy_host_external_files_stream_and_range() {
    for path in std::env::var("LOVELYCARVE_SQUASHFS_LEGACY")
        .unwrap()
        .split(';')
    {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let mut session = FsSession::new(&image);
        let mut pending = vec!["part:0".to_owned()];
        let mut count = 0;
        while let Some(parent) = pending.pop() {
            for entry in session.children(&parent).unwrap() {
                if entry.kind == NodeKind::Dir {
                    pending.push(entry.id);
                } else if entry.kind == NodeKind::File && entry.size > 0 {
                    let bytes = session
                        .read_file_limited(&entry.id, 128 * 1024 * 1024)
                        .unwrap();
                    assert_eq!(bytes.len() as u64, entry.size);
                    let mut streamed = Vec::new();
                    stream_file_chunks(&image, &entry.id, 65536, |chunk| {
                        streamed.extend_from_slice(chunk);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(streamed, bytes);
                    for offset in [
                        0,
                        4095,
                        32765,
                        65530,
                        bytes.len() / 2,
                        bytes.len(),
                        bytes.len() + 1,
                    ] {
                        let start = offset.min(bytes.len());
                        assert_eq!(
                            session
                                .read_file_range(&entry.id, offset as u64, 8193)
                                .unwrap(),
                            bytes[start..(start + 8193).min(bytes.len())]
                        );
                    }
                    count += 1;
                }
            }
        }
        assert!(count > 0);
    }
}
