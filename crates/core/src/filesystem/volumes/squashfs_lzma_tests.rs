use std::{path::Path, sync::Arc};

use crate::evidence::image::PartitionKind;
use crate::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{stream_file_chunks, FsSession},
};

fn verify_file(image: &Arc<EvidenceSession>, path: &str, expected: &[u8]) {
    let id = format!("squashfs:part:0:{}", hex::encode(path));
    let mut session = FsSession::new(image);
    assert_eq!(
        session.read_file_limited(&id, usize::MAX).unwrap(),
        expected
    );
    let mut streamed = Vec::new();
    stream_file_chunks(image, &id, 1500, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(streamed, expected);
    for (offset, length) in [
        (0, 3),
        (4080, 100),
        (8180, 100),
        (expected.len() as u64 + 1, 20),
    ] {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(expected.len());
        let end = start.saturating_add(length).min(expected.len());
        assert_eq!(
            session.read_file_range(&id, offset, length).unwrap(),
            expected[start..end]
        );
    }
    assert!(session.read_file_range(&id, u64::MAX, 20).is_err());
}

#[test]
fn squashfs_lzma_dispatch_preserves_original_and_reads_all_block_formats() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/backhand/test_data");
    let content = b"Independent Rust LZMA fixture.\n".repeat(350);
    for mode in ["raw", "alone", "props", "gzip"] {
        for endian in ["le", "be"] {
            let path = root.join(format!("lzma-v3-{mode}-{endian}.squashfs"));
            let original = std::fs::read(&path).unwrap();
            let image = Arc::new(EvidenceSession::open(&path).unwrap());
            assert_eq!(image.sector_size, 1);
            assert_eq!(image.partitions[0].kind, PartitionKind::Squashfs);
            assert_eq!(image.partitions[0].length_lba, original.len() as u64);
            assert_eq!(image.read_range(0, original.len()).unwrap(), original);
            let entries = FsSession::new(&image).children("part:0").unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "hello");
            assert_eq!(entries[0].size, content.len() as u64);
            verify_file(&image, "/hello", &content);
        }
    }
}

#[test]
#[ignore = "set LOVELYCARVE_SQUASHFS_LZMA to semicolon-separated Unblob LE/BE adaptive inputs"]
fn squashfs_lzma_external_independent_oracle() {
    // Expected bytes are from Unblob's separately recorded extraction output:
    // apple SHA256 303980bc..., orange SHA256 be6fe118.... No host writer round trip.
    let paths = std::env::var("LOVELYCARVE_SQUASHFS_LZMA").unwrap();
    for path in paths.split(';') {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let entries = FsSession::new(&image).children("part:0").unwrap();
        assert_eq!(entries.len(), 2);
        for (name, expected) in [
            ("apple.txt", b"apple\n".as_slice()),
            ("orange.txt", b"orange\n".as_slice()),
        ] {
            assert!(entries.iter().any(|entry| entry.name == name));
            verify_file(&image, &format!("/{name}"), expected);
        }
    }
}
