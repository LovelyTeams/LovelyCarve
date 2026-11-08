//! Tests for the LovelyCarve changes: positioned (shareable) reads, parent
//! indices, chunk-table slicing in `read_at`, and the tree-walk read-ahead
//! window, against fixtures from the independent `testfix` encoder.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use std::path::{Path, PathBuf};

use ad1::testfix::{self, Node};
use ad1::Ad1Reader;

fn write_segments(dir: &Path, stem: &str, segments: &[Vec<u8>]) -> PathBuf {
    for (index, bytes) in segments.iter().enumerate() {
        std::fs::write(dir.join(format!("{stem}.ad{}", index + 1)), bytes).unwrap();
    }
    dir.join(format!("{stem}.ad1"))
}

fn expected_data<'a>(built: &'a testfix::Built, path: &str) -> &'a [u8] {
    built
        .expected
        .iter()
        .find(|entry| entry.path == path)
        .and_then(|entry| entry.data.as_deref())
        .unwrap()
}

fn leak(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// A wide tree whose item records and metadata span many read-ahead windows.
fn wide_tree() -> Node {
    let folders = (0..12)
        .map(|folder| {
            let files = (0..250)
                .map(|file| {
                    Node::File(
                        leak(format!("file-{folder:02}-{file:03}-with-a-longer-name.txt")),
                        format!("folder {folder} file {file}\n").into_bytes(),
                    )
                })
                .collect();
            Node::Dir(leak(format!("folder-{folder:02}")), files)
        })
        .collect();
    Node::Dir("root", folders)
}

#[test]
fn reader_is_send_and_sync() {
    fn shareable<T: Send + Sync>() {}
    shareable::<Ad1Reader>();
}

#[test]
fn parent_indices_follow_the_tree() {
    let built = testfix::build(testfix::sample_tree());
    let dir = tempfile::tempdir().unwrap();
    let path = write_segments(dir.path(), "image", &[built.bytes]);
    let reader = Ad1Reader::open(&path).unwrap();
    let shape: Vec<(&str, Option<usize>)> = reader
        .entries()
        .iter()
        .map(|entry| (entry.path.as_str(), entry.parent))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("root", None),
            ("root/hello.txt", Some(0)),
            ("root/sub", Some(0)),
            ("root/sub/a.bin", Some(2)),
            ("root/sub/empty.dat", Some(2)),
        ]
    );
    assert_eq!(reader.segment_paths(), [path]);
}

#[test]
fn ranged_reads_match_the_data_at_every_chunk_boundary() {
    let built = testfix::build(testfix::sample_tree());
    let dir = tempfile::tempdir().unwrap();
    let path = write_segments(dir.path(), "image", std::slice::from_ref(&built.bytes));
    let reader = Ad1Reader::open(&path).unwrap();
    let entry = reader
        .entries()
        .iter()
        .find(|entry| entry.path == "root/sub/a.bin")
        .unwrap();
    let data = expected_data(&built, "root/sub/a.bin");
    let chunk = u64::from(testfix::CHUNK_SIZE);
    let offsets = [
        0,
        1,
        chunk - 1,
        chunk,
        chunk + 7,
        2 * chunk - 3,
        3 * chunk,
        199_990,
    ];
    let lengths = [1usize, 100, chunk as usize, 2 * chunk as usize + 5, 250_000];
    for offset in offsets {
        for length in lengths {
            let mut buf = vec![0u8; length];
            let read = reader.read_at(entry, offset, &mut buf).unwrap();
            let start = offset as usize;
            let end = (start + length).min(data.len());
            assert_eq!(read, end - start, "offset {offset} length {length}");
            assert_eq!(
                &buf[..read],
                &data[start..end],
                "offset {offset} length {length}"
            );
        }
    }
}

#[test]
fn one_reader_serves_concurrent_reads() {
    let built = testfix::build(testfix::sample_tree());
    let dir = tempfile::tempdir().unwrap();
    let path = write_segments(dir.path(), "image", std::slice::from_ref(&built.bytes));
    let reader = Ad1Reader::open(&path).unwrap();
    let entry = reader
        .entries()
        .iter()
        .find(|entry| entry.path == "root/sub/a.bin")
        .unwrap();
    let data = expected_data(&built, "root/sub/a.bin");
    std::thread::scope(|scope| {
        for worker in 0..8u64 {
            let reader = &reader;
            scope.spawn(move || {
                for round in 0..40u64 {
                    let offset = (worker * 24_989 + round * 4_099) % 190_000;
                    let mut buf = vec![0u8; 9_000];
                    let read = reader.read_at(entry, offset, &mut buf).unwrap();
                    let start = offset as usize;
                    assert_eq!(&buf[..read], &data[start..start + read]);
                    assert_eq!(read, 9_000.min(data.len() - start));
                }
            });
        }
    });
}

#[test]
fn wide_tree_walk_crosses_read_ahead_windows_and_segments() {
    let built = testfix::build(wide_tree());
    let want: Vec<(String, bool, u64)> = built
        .expected
        .iter()
        .map(|entry| (entry.path.clone(), entry.is_dir, entry.size))
        .collect();

    for fragments_size in [0, 2] {
        let dir = tempfile::tempdir().unwrap();
        let segments = if fragments_size == 0 {
            vec![built.bytes.clone()]
        } else {
            let stride = fragments_size as usize * 0x1_0000 - testfix::MARGIN;
            let count = (built.bytes.len() - testfix::MARGIN).div_ceil(stride) as u32;
            assert!(count >= 3, "the tree should span several segments");
            testfix::split(&built.bytes, count, fragments_size)
        };
        let path = write_segments(dir.path(), "wide", &segments);
        let reader = Ad1Reader::open(&path).unwrap();
        let got: Vec<(String, bool, u64)> = reader
            .entries()
            .iter()
            .map(|entry| (entry.path.clone(), entry.is_dir, entry.size))
            .collect();
        assert_eq!(got, want, "fragments_size {fragments_size}");
        for (entry, expected) in reader.entries().iter().zip(&built.expected) {
            assert_eq!(entry.md5, expected.md5, "{}", expected.path);
            if let Some(data) = &expected.data {
                let mut buf = vec![0u8; data.len()];
                assert_eq!(reader.read_at(entry, 0, &mut buf).unwrap(), data.len());
                assert_eq!(&buf, data, "{}", expected.path);
            }
        }
        assert_eq!(reader.segment_paths().len(), segments.len());
    }
}
