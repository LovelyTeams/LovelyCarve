use super::*;
use std::cell::Cell;

struct Bytes(Vec<u8>);
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        Ok(self
            .0
            .get(offset as usize..offset as usize + length)
            .context("test source bounds")?
            .to_vec())
    }
}
fn little() -> Geometry {
    Geometry {
        format: Format::Yaffs2,
        page_size: 2048,
        spare_size: 64,
        tags_offset: 0,
        order: ByteOrder::Little,
    }
}
const LE: &[u8] = include_bytes!("../test_data/base-le.yaffs");
const BE: &[u8] = include_bytes!("../test_data/base-be-ecc.yaffs");

#[test]
fn independent_endians_names_links_metadata_and_ranged_bytes() {
    let expected = b"Native YAFFS2 fixture.\n".repeat(300);
    for bytes in [LE, BE] {
        let bytes = Bytes(bytes.to_vec());
        let image = YaffsImage::open(&bytes).unwrap();
        assert_eq!(
            image
                .children(1)
                .unwrap()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alias", "console", "empty", "link", "目录"]
        );
        assert_eq!(image.node(1).unwrap().modified, None);
        let number = image.resolve("/目录/hello").unwrap();
        let node = image.content_node(number).unwrap();
        assert_eq!(
            (node.uid, node.gid, node.modified),
            (Some(1000), Some(1001), Some(1700000000))
        );
        assert_eq!(
            image
                .read_range(&bytes, number, 0, expected.len() + 1)
                .unwrap(),
            expected
        );
        for offset in [0, 1, 2047, 4095, 6599, 6600, 6601] {
            let start = offset.min(expected.len());
            assert_eq!(
                image
                    .read_range(&bytes, number, offset as u64, 4097)
                    .unwrap(),
                expected[start..(start + 4097).min(expected.len())]
            );
        }
        assert_eq!(
            image
                .read_range(&bytes, image.resolve("/alias").unwrap(), 11, 97)
                .unwrap(),
            expected[11..108]
        );
        assert_eq!(
            image
                .read_range(&bytes, image.resolve("/link").unwrap(), 0, 100)
                .unwrap(),
            "目录/hello".as_bytes()
        );
        assert_eq!(
            image
                .read_range(&bytes, image.resolve("/empty").unwrap(), 0, 10)
                .unwrap(),
            b""
        );
        assert!(
            image
                .read_range(&bytes, image.resolve("/console").unwrap(), 0, 0)
                .is_err()
        );
        assert!(image.resolve("/gone").is_err());
        for path in ["hello", "/../hello", "//alias", "/目录/./hello"] {
            assert!(image.resolve(path).is_err());
        }
    }
}
#[test]
fn sequence_order_truncation_zeroes_and_post_header_growth() {
    let bytes = Bytes(include_bytes!("../test_data/versions.yaffs").to_vec());
    let image = YaffsImage::open(&bytes).unwrap();
    let number = image.resolve("/renamed").unwrap();
    assert!(image.resolve("/old").is_err());
    let expected = [vec![b'X'; 100], vec![0; 3996], b"tail-end".to_vec()].concat();
    assert_eq!(image.node(number).unwrap().size, 4104);
    assert_eq!(image.read_range(&bytes, number, 0, 5000).unwrap(), expected);
    assert_eq!(
        image.read_range(&bytes, number, 95, 2060).unwrap(),
        expected[95..2155]
    );
    let bytes = Bytes(include_bytes!("../test_data/shadow.yaffs").to_vec());
    let image = YaffsImage::open(&bytes).unwrap();
    assert_eq!(image.children(1).unwrap().len(), 1);
    assert_eq!(
        image
            .read_range(&bytes, image.resolve("/new").unwrap(), 0, 10)
            .unwrap(),
        b"yes"
    );
}
#[test]
fn sparse_64_bit_size_times_and_large_seek_are_not_truncated() {
    let bytes = Bytes(include_bytes!("../test_data/sparse-64bit.yaffs").to_vec());
    let image = YaffsImage::open(&bytes).unwrap();
    let number = image.resolve("/long").unwrap();
    assert_eq!(image.node(number).unwrap().size, (1 << 32) + 3);
    assert_eq!(image.node(number).unwrap().changed, Some((1 << 32) + 45678));
    assert_eq!(
        image.read_range(&bytes, number, (1 << 32) - 1, 9).unwrap(),
        b"\0end"
    );
}
#[test]
fn unlink_promotes_a_hardlink_without_losing_remaining_content() {
    let bytes = Bytes(include_bytes!("../test_data/unlink-hardlink.yaffs").to_vec());
    let image = YaffsImage::open(&bytes).unwrap();
    assert!(image.resolve("/original").is_err());
    assert_eq!(image.children(1).unwrap().len(), 2);
    for path in ["/promoted", "/remaining"] {
        let number = image.resolve(path).unwrap();
        assert_eq!(image.content_node(number).unwrap().object_id, 257);
        assert_eq!(image.read_range(&bytes, number, 0, 10).unwrap(), b"survive");
        assert_eq!(image.read_range(&bytes, number, 2, 3).unwrap(), b"rvi");
    }
}
#[test]
fn malformed_tags_ecc_headers_cycles_and_source_short_reads_fail() {
    let stride = little().stride();
    for (at, value) in [
        (2048 + 4, 255),
        (2048 + 8, 2),
        (0, 0),
        (10, b'/'),
        (269, 0x81),
    ] {
        let mut bytes = LE.to_vec();
        bytes[at] = value;
        assert!(
            YaffsImage::open_with_geometry(&Bytes(bytes), little(), &CancellationToken::new())
                .is_err()
        );
    }
    let mut bytes = BE.to_vec();
    bytes[4096 + 2 + 16] ^= 1;
    let geometry = Geometry {
        format: Format::Yaffs2,
        page_size: 4096,
        spare_size: 128,
        tags_offset: 2,
        order: ByteOrder::Big,
    };
    assert!(
        YaffsImage::open_with_geometry(&Bytes(bytes), geometry, &CancellationToken::new()).is_err()
    );
    // A directory cannot be its own parent, and hard links cannot dangle.
    let mut bytes = LE.to_vec();
    bytes[4..8].copy_from_slice(&256u32.to_le_bytes());
    bytes[2048 + 8..2048 + 12].copy_from_slice(&0x8000_0100u32.to_le_bytes());
    assert!(
        YaffsImage::open_with_geometry(&Bytes(bytes), little(), &CancellationToken::new()).is_err()
    );
    let mut bytes = LE.to_vec();
    let at = 6 * stride;
    bytes[at + 296..at + 300].copy_from_slice(&999u32.to_le_bytes());
    bytes[at + 2048 + 12..at + 2048 + 16].copy_from_slice(&999u32.to_le_bytes());
    assert!(
        YaffsImage::open_with_geometry(&Bytes(bytes), little(), &CancellationToken::new()).is_err()
    );
    for removed in [1, 16, 64, 2111] {
        assert!(
            YaffsImage::open_with_geometry(
                &Bytes(LE[..LE.len() - removed].to_vec()),
                little(),
                &CancellationToken::new()
            )
            .is_err()
        );
    }
    struct Short;
    impl ReadAt for Short {
        fn size(&self) -> u64 {
            LE.len() as u64
        }
        fn read_at(&self, _: u64, length: usize) -> Result<Vec<u8>> {
            Ok(vec![0; length.saturating_sub(1)])
        }
    }
    assert!(YaffsImage::open(&Short).is_err());
}

#[test]
fn explicit_empty_geometry_and_non_user_summary_checkpoint_pages() {
    let blank = Bytes(vec![255; 2112]);
    assert!(YaffsImage::open(&blank).is_err());
    let image =
        YaffsImage::open_with_geometry(&blank, little(), &CancellationToken::new()).unwrap();
    assert!(image.children(1).unwrap().is_empty());
    assert_eq!(image.node(1).unwrap().modified, None);
    let mut bytes = LE.to_vec();
    for (sequence, object) in [(0x21u32, 0x20u32), (0x1000, 0x10)] {
        let mut page = vec![255; 2112];
        page[2048..2052].copy_from_slice(&sequence.to_le_bytes());
        page[2052..2056].copy_from_slice(&object.to_le_bytes());
        page[2056..2060].copy_from_slice(&1u32.to_le_bytes());
        page[2060..2064].copy_from_slice(&100u32.to_le_bytes());
        bytes.extend_from_slice(&page);
    }
    let image = YaffsImage::open(&Bytes(bytes)).unwrap();
    assert_eq!(image.children(1).unwrap().len(), 5);
}
#[test]
fn cancellation_resource_admission_and_allocated_read_faults_are_explicit() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(YaffsImage::open_cancellable(&Bytes(LE.to_vec()), &token).is_err());
    struct Fault;
    impl ReadAt for Fault {
        fn size(&self) -> u64 {
            LE.len() as u64
        }
        fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
            ensure!(offset != 4224, "source data read fault");
            Ok(LE[offset as usize..offset as usize + length].to_vec())
        }
    }
    let image =
        YaffsImage::open_with_geometry(&Fault, little(), &CancellationToken::new()).unwrap();
    assert!(
        image
            .read_range(&Fault, image.resolve("/目录/hello").unwrap(), 0, 10)
            .is_err()
    );
    struct Huge;
    impl ReadAt for Huge {
        fn size(&self) -> u64 {
            4_000_001 * 2112
        }
        fn read_at(&self, _: u64, _: usize) -> Result<Vec<u8>> {
            panic!("admission must precede reads")
        }
    }
    assert!(YaffsImage::open_with_geometry(&Huge, little(), &CancellationToken::new()).is_err());
    struct Stop {
        reads: Cell<usize>,
        token: CancellationToken,
    }
    impl ReadAt for Stop {
        fn size(&self) -> u64 {
            LE.len() as u64
        }
        fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            if self.reads.get() == 2 {
                self.token.cancel();
            }
            Ok(LE[offset as usize..offset as usize + length].to_vec())
        }
    }
    let source = Stop {
        reads: Cell::new(0),
        token: CancellationToken::new(),
    };
    assert!(YaffsImage::open_with_geometry(&source, little(), &source.token).is_err());
}

#[test]
#[ignore = "set YAFFS2_EXTERNAL and YAFFS2_ORACLE to a public image and independent extracted directory"]
fn external_all_files_match_independent_oracle() {
    // Compare source bytes with a separate Python extractor's output directory.
    let bytes = Bytes(std::fs::read(std::env::var("YAFFS2_EXTERNAL").unwrap()).unwrap());
    let image = YaffsImage::open(&bytes).unwrap();
    assert_eq!(image.geometry, little());
    assert_eq!(image.children(1).unwrap().len(), 13);
    for entry in image.children(1).unwrap() {
        let node = image.content_node(entry.object_id).unwrap();
        let data = image
            .read_range(&bytes, entry.object_id, 0, node.size as usize + 1)
            .unwrap();
        let directory = std::env::var("YAFFS2_ORACLE").unwrap();
        assert_eq!(
            data,
            std::fs::read(std::path::Path::new(&directory).join(&entry.name)).unwrap(),
            "{}",
            entry.name
        );
    }
}

#[test]
#[ignore = "set YAFFS2_MATRIX_DIR to the verified public Unblob input directory"]
fn external_unblob_geometry_matrix_files_links_and_special_nodes() {
    let directory = std::env::var("YAFFS2_MATRIX_DIR").unwrap();
    for (name, page_size, spare_size, order) in [
        ("sample.2048.64.ecc.le.yaffs2", 2048, 64, ByteOrder::Little),
        ("sample.4096.128.ecc.be.yaffs2", 4096, 128, ByteOrder::Big),
        ("sample.16384.512.le.yaffs2", 16384, 512, ByteOrder::Little),
    ] {
        let source = Bytes(std::fs::read(std::path::Path::new(&directory).join(name)).unwrap());
        let image = YaffsImage::open(&source).unwrap();
        assert_eq!(image.geometry.page_size, page_size);
        assert_eq!(image.geometry.spare_size, spare_size);
        assert_eq!(image.geometry.order, order);
        assert_eq!(
            image.geometry.tags_offset,
            if name.contains(".ecc.") { 0 } else { 2 }
        );
        assert_eq!(image.children(1).unwrap().len(), 4);
        for path in [
            "/fruits/apple.txt",
            "/fruits/banana.txt",
            "/fruits/cherry.txt",
            "/fruits/tomato.txt",
            "/vegetables/potato.txt",
            "/vegetables/kale.txt",
        ] {
            let number = image.resolve(path).unwrap();
            let expected = format!(
                "{}\n",
                path.rsplit('/').next().unwrap().trim_end_matches(".txt")
            );
            assert_eq!(
                image.read_range(&source, number, 0, 100).unwrap(),
                expected.as_bytes()
            );
            assert_eq!(
                image.read_range(&source, number, 2, 3).unwrap(),
                &expected.as_bytes()[2..5]
            );
        }
        for path in ["/passwd", "/vegetables/tomato.txt"] {
            let number = image.resolve(path).unwrap();
            let node = image.content_node(number).unwrap();
            assert_eq!(node.kind, FileType::Symlink);
            assert_eq!(
                image.read_range(&source, number, 0, 100).unwrap(),
                node.link_target.as_bytes()
            );
        }
        for path in ["/nuts/macadamia", "/nuts/hazelnut"] {
            let number = image.resolve(path).unwrap();
            assert_eq!(image.content_node(number).unwrap().kind, FileType::Special);
            assert!(image.read_range(&source, number, 0, 1).is_err());
        }
    }
    for name in ["links.yaffs", "fruits.dir.le.yffs", "fruits.dir.be.yffs"] {
        let source = Bytes(std::fs::read(std::path::Path::new(&directory).join(name)).unwrap());
        assert_eq!(
            YaffsImage::open(&source).unwrap().geometry.format,
            Format::Yaffs1
        );
    }
}
