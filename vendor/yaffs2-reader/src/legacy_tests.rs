use super::*;
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
const LE: &[u8] = include_bytes!("../test_data/legacy-le.yaffs");
const BE: &[u8] = include_bytes!("../test_data/legacy-be.yaffs");
#[test]
fn legacy_endians_ecc_links_sparse_ranges_and_orphans() {
    let expected = b"Native YAFFS1 data.\n".repeat(61);
    for (source, order) in [(LE, ByteOrder::Little), (BE, ByteOrder::Big)] {
        let bytes = Bytes(source.to_vec());
        let image = YaffsImage::open(&bytes).unwrap();
        assert_eq!(image.geometry.format, Format::Yaffs1);
        assert_eq!(image.geometry.order, order);
        assert_eq!(image.orphan_data_objects, 1);
        assert_eq!(image.children(1).unwrap().len(), 5);
        assert!(image.resolve("/gone").is_err());
        for path in ["/目录/message", "/alias"] {
            let number = image.resolve(path).unwrap();
            assert_eq!(image.content_node(number).unwrap().object_id, 257);
            assert_eq!(image.read_range(&bytes, number, 0, 2000).unwrap(), expected);
            for offset in [0, 1, 511, 512, 1023, expected.len(), expected.len() + 1] {
                let start = offset.min(expected.len());
                assert_eq!(
                    image
                        .read_range(&bytes, number, offset as u64, 517)
                        .unwrap(),
                    expected[start..(start + 517).min(expected.len())]
                );
            }
        }
        assert_eq!(image.node(257).unwrap().modified, Some(1700000000));
        assert_eq!(
            image
                .read_range(&bytes, image.resolve("/sym").unwrap(), 0, 100)
                .unwrap(),
            "目录/message".as_bytes()
        );
        assert!(
            image
                .read_range(&bytes, image.resolve("/console").unwrap(), 0, 1)
                .is_err()
        );
    }
}
#[test]
fn legacy_forward_serial_wrap_deletion_shadow_and_size_policy() {
    let bytes = Bytes(include_bytes!("../test_data/legacy-versions.yaffs").to_vec());
    let image = YaffsImage::open(&bytes).unwrap();
    assert_eq!(image.children(1).unwrap().len(), 1);
    assert!(image.resolve("/old").is_err());
    let number = image.resolve("/replacement").unwrap();
    assert_eq!(image.content_node(number).unwrap().object_id, 257);
    let expected = [vec![b'X'; 512], vec![b'y'; 100], vec![0; 188]].concat();
    assert_eq!(image.read_range(&bytes, number, 0, 1000).unwrap(), expected);
    let image = YaffsImage::open_yaffs1(
        &bytes,
        ByteOrder::Little,
        FileSizeSource::DataPages,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(image.node(number).unwrap().size, 612);
    assert_eq!(
        image.read_range(&bytes, number, 0, 1000).unwrap(),
        expected[..612]
    );
}
#[test]
fn legacy_tag_and_full_page_ecc_faults_markers_truncation_and_cancellation() {
    for position in [512, 512 + 5, 512 + 4, 20] {
        let mut bytes = LE.to_vec();
        bytes[position] ^= 1;
        assert!(
            YaffsImage::open_yaffs1(
                &Bytes(bytes),
                ByteOrder::Little,
                FileSizeSource::Header,
                &CancellationToken::new()
            )
            .is_err()
        );
    }
    let mut bytes = LE.to_vec();
    bytes[3 * 528 + 400] ^= 1;
    let bytes = Bytes(bytes);
    let image = YaffsImage::open(&bytes).unwrap();
    // Even a range which excludes the flipped bit checks the whole allocated page.
    assert!(image.read_range(&bytes, 257, 0, 1).is_err());
    assert!(YaffsImage::open(&Bytes(LE[..LE.len() - 1].to_vec())).is_err());
    let token = CancellationToken::new();
    token.cancel();
    assert!(
        YaffsImage::open_yaffs1(
            &Bytes(LE.to_vec()),
            ByteOrder::Little,
            FileSizeSource::Header,
            &token
        )
        .is_err()
    );
}
#[test]
#[ignore = "set YAFFS2_MATRIX_DIR to the public Unblob input directory"]
fn legacy_external_public_fruits_and_links() {
    let directory = std::env::var("YAFFS2_MATRIX_DIR").unwrap();
    for (name, order) in [
        ("fruits.dir.le.yffs", ByteOrder::Little),
        ("fruits.dir.be.yffs", ByteOrder::Big),
    ] {
        let bytes = Bytes(std::fs::read(std::path::Path::new(&directory).join(name)).unwrap());
        let image = YaffsImage::open(&bytes).unwrap();
        assert_eq!(image.geometry.format, Format::Yaffs1);
        assert_eq!(image.geometry.order, order);
        for name in ["apple", "banana", "cherry"] {
            let number = image.resolve(&format!("/dir/{name}.txt")).unwrap();
            let expected = format!("{name}\n");
            assert_eq!(
                image.read_range(&bytes, number, 0, 100).unwrap(),
                expected.as_bytes()
            );
        }
    }
    let bytes = Bytes(std::fs::read(std::path::Path::new(&directory).join("links.yaffs")).unwrap());
    let image = YaffsImage::open(&bytes).unwrap();
    for path in ["/dir/hardlink", "/hardlink", "/file"] {
        let number = image.resolve(path).unwrap();
        assert_eq!(image.content_node(number).unwrap().object_id, 259);
        assert_eq!(
            image.read_range(&bytes, number, 0, 100).unwrap(),
            b"content\n"
        );
    }
    for (path, target) in [
        ("/dir/symlink", "../file"),
        ("/symlink", "file"),
        ("/symlink2", "dir/hardlink"),
    ] {
        let number = image.resolve(path).unwrap();
        assert_eq!(
            image.read_range(&bytes, number, 0, 100).unwrap(),
            target.as_bytes()
        );
    }
}
