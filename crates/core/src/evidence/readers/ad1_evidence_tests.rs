//! AD1 evidence end to end: `EvidenceSession::open` on a multi-segment image built
//! by the reader's independent fixture encoder, then the public `fs_tree`
//! entry points the commands use.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ad1::testfix;

use crate::evidence::image::ImageFormat;
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{self, FsSession, NodeKind};
use crate::filesystem::tree::fs_tree_walk::directory_stats;

/// The sample tree split over several segments: `case.ad1`, `case.ad2`, ….
fn write_case(dir: &Path) -> (PathBuf, testfix::Built, Vec<Vec<u8>>) {
    let built = testfix::build(testfix::sample_tree());
    let fragments_size = 2u32;
    let stride = fragments_size as usize * 0x1_0000 - testfix::MARGIN;
    let count = (built.bytes.len() - testfix::MARGIN).div_ceil(stride) as u32;
    let segments = testfix::split(&built.bytes, count, fragments_size);
    for (index, bytes) in segments.iter().enumerate() {
        std::fs::write(dir.join(format!("case.ad{}", index + 1)), bytes).unwrap();
    }
    (dir.join("case.ad1"), built, segments)
}

fn data<'a>(built: &'a testfix::Built, path: &str) -> &'a [u8] {
    built
        .expected
        .iter()
        .find(|entry| entry.path == path)
        .and_then(|entry| entry.data.as_deref())
        .unwrap()
}

fn names(image: &Arc<EvidenceSession>, parent: &str) -> Vec<(String, NodeKind)> {
    fs_tree::children_for_image(image, parent)
        .unwrap()
        .into_iter()
        .map(|node| (node.name, node.kind))
        .collect()
}

#[test]
fn ad1_evidence_browses_and_reads_through_fs_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (path, built, segments) = write_case(dir.path());
    assert!(segments.len() >= 2);
    let image = Arc::new(EvidenceSession::open(&path).unwrap());
    let info = image.info();
    assert_eq!(info.source_format, ImageFormat::Ad1);
    assert!(info.partitions.is_empty());
    assert_eq!(
        image.len(),
        segments
            .iter()
            .map(|segment| segment.len() as u64)
            .sum::<u64>()
    );
    assert_eq!(image.read_range(0, 15).unwrap(), b"ADSEGMENTEDFILE");

    assert_eq!(
        names(&image, "root"),
        vec![("case.ad1".to_string(), NodeKind::Dir)]
    );
    assert_eq!(
        names(&image, "ad1:"),
        vec![("root".to_string(), NodeKind::Dir)]
    );
    assert_eq!(
        names(&image, "ad1:0"),
        vec![
            ("sub".to_string(), NodeKind::Dir),
            ("hello.txt".to_string(), NodeKind::File)
        ]
    );
    let hello = fs_tree::children_for_image(&image, "ad1:0")
        .unwrap()
        .into_iter()
        .find(|node| node.name == "hello.txt")
        .unwrap();
    assert_eq!(hello.parent_id.as_deref(), Some("ad1:0"));
    assert_eq!(
        hello.timestamps.modified.as_deref(),
        Some("2024-11-30T10:15:00+00:00")
    );
    assert_eq!(
        fs_tree::read_file_limited(&image, &hello.id, 1024).unwrap(),
        b"Hello, AD1!\n"
    );

    // `a.bin` is entry 3 in depth-first order; its chunks cross segments.
    let large = data(&built, "root/sub/a.bin");
    assert_eq!(fs_tree::file_stat(&image, "ad1:3").unwrap().size, 200_000);
    assert_eq!(
        fs_tree::read_file_range(&image, "ad1:3", 131_000, 1_000).unwrap(),
        &large[131_000..132_000]
    );
    let mut streamed = Vec::new();
    let total = fs_tree::stream_file_chunks(&image, "ad1:3", 64 * 1024, |chunk| {
        streamed.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!((total, streamed.as_slice()), (200_000, large));

    let mut session = FsSession::new(&image);
    let mut ranged = Vec::new();
    let delivered = session
        .stream_file_range("ad1:3", 199_000, 5_000, 4096, |chunk| {
            ranged.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    assert_eq!((delivered, ranged.as_slice()), (1_000, &large[199_000..]));

    let stats = directory_stats(&image, "ad1:2", "root/sub", 1000).unwrap();
    assert_eq!((stats.file_count, stats.total_bytes), (2, 200_000));
}

#[test]
fn later_segments_encrypted_and_foreign_ad1_files() {
    let dir = tempfile::tempdir().unwrap();
    write_case(dir.path());
    let error = EvidenceSession::open(dir.path().join("case.ad2")).unwrap_err();
    assert!(
        format!("{error:#}").contains("请打开第一个分段"),
        "{error:#}"
    );

    let foreign = dir.path().join("notes.ad1");
    std::fs::write(&foreign, vec![0x41u8; 8192]).unwrap();
    let raw = EvidenceSession::open(&foreign).unwrap();
    assert_eq!(raw.info().source_format, ImageFormat::Raw);

    let locked = dir.path().join("locked.ad1");
    let mut bytes = vec![0u8; 1024];
    bytes[..8].copy_from_slice(b"ADCRYPT\0");
    std::fs::write(&locked, bytes).unwrap();
    let error = EvidenceSession::open(&locked).unwrap_err();
    assert!(format!("{error:#}").contains("ADCRYPT"), "{error:#}");
}
