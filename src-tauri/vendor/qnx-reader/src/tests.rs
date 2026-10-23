use super::*;
use lamfold::{BlockCache, NoVerifier, SliceSource};
use std::io::Read;
fn unpack(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut output)
        .unwrap();
    output
}
fn context(cache: &mut BlockCache) -> SubstrateCtx<'_> {
    SubstrateCtx {
        cache,
        verifier: &NoVerifier,
    }
}

#[test]
fn qnx6_qnx_created_image_matches_independent_tar() {
    let image = unpack(include_bytes!("../tests/fixtures/nfi-qnx6.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = Qnx6::open(SliceSource::new(&image), &mut cx).unwrap();
    assert_eq!(fs.serial(), 2);
    assert!(fs
        .lookup(1, "this_file_is_removed", &mut cx)
        .unwrap()
        .is_none());
    assert!(fs
        .lookup(1, "this_file_is_not_renamed", &mut cx)
        .unwrap()
        .is_none());
    let archive = flate2::read::GzDecoder::new(
        include_bytes!("../tests/fixtures/nfi-qnx6.tar.gz").as_slice(),
    );
    let mut archive = tar::Archive::new(archive);
    let mut count = 0;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry
            .path()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        let mut node = fs.root();
        for component in path.split('/') {
            node = fs.lookup(node, component, &mut cx).unwrap().unwrap();
        }
        let metadata = fs.metadata(node, &mut cx).unwrap();
        assert_eq!(
            metadata.mode & 0xfff,
            entry.header().mode().unwrap() & 0xfff,
            "{path}"
        );
        assert_eq!(
            fs.timestamps(node, &mut cx).unwrap().modified,
            Some((entry.header().mtime().unwrap() as i64, 0)),
            "{path}"
        );
        if entry.header().entry_type().is_file() {
            let mut expected = Vec::new();
            entry.read_to_end(&mut expected).unwrap();
            let mut actual = vec![0; expected.len()];
            assert_eq!(
                fs.read_at(node, 0, &mut actual, &mut cx).unwrap(),
                expected.len()
            );
            assert_eq!(actual, expected, "{path}");
            for offset in [0, 500, 1020, actual.len().saturating_sub(3), actual.len()] {
                if offset > actual.len() {
                    continue;
                }
                let mut bytes = [0; 83];
                let size = fs
                    .read_at(node, offset as u64, &mut bytes, &mut cx)
                    .unwrap();
                assert_eq!(
                    &bytes[..size],
                    &expected[offset..(offset + 83).min(expected.len())]
                );
            }
        } else if entry.header().entry_type().is_symlink() {
            assert_eq!(metadata.kind, FileKind::Symlink);
            assert_eq!(
                fs.read_link(node, &mut cx).unwrap().unwrap(),
                entry
                    .link_name()
                    .unwrap()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .as_bytes()
            );
        } else if entry.header().entry_type().is_dir() {
            assert_eq!(metadata.kind, FileKind::Directory);
        } else {
            assert_eq!(metadata.kind, FileKind::Other);
        }
        count += 1;
    }
    assert_eq!(count, 32);
}

#[test]
fn qnx6_byte_orders_blocks_mmi_names_links_sparse_ranges() {
    for (compressed, block_size, endian, mmi) in [
        (
            include_bytes!("../tests/fixtures/qnx6-512-le.zlib").as_slice(),
            512,
            Endian::Little,
            false,
        ),
        (
            include_bytes!("../tests/fixtures/qnx6-1024-be.zlib").as_slice(),
            1024,
            Endian::Big,
            false,
        ),
        (
            include_bytes!("../tests/fixtures/qnx6-2048-le.zlib").as_slice(),
            2048,
            Endian::Little,
            false,
        ),
        (
            include_bytes!("../tests/fixtures/qnx6-4096-be.zlib").as_slice(),
            4096,
            Endian::Big,
            false,
        ),
        (
            include_bytes!("../tests/fixtures/qnx6-bootless-le.zlib").as_slice(),
            1024,
            Endian::Little,
            false,
        ),
        (
            include_bytes!("../tests/fixtures/qnx6-mmi-le.zlib").as_slice(),
            1024,
            Endian::Little,
            true,
        ),
    ] {
        let bytes = unpack(compressed);
        let mut cache = BlockCache::new(0);
        let mut cx = context(&mut cache);
        let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
        assert_eq!(fs.serial(), 9);
        assert_eq!(fs.block_size(), block_size);
        assert_eq!(fs.endian(), endian);
        assert_eq!(fs.is_mmi(), mmi);
        let root = fs.read_dir(1, &mut cx).unwrap();
        assert_eq!(root.len(), 6);
        assert_eq!(
            fs.lookup(1, "small.txt", &mut cx).unwrap(),
            fs.lookup(1, "alias.txt", &mut cx).unwrap()
        );
        assert!(fs
            .lookup(1, "this_name_is_longer_than_twenty_seven.txt", &mut cx)
            .unwrap()
            .is_some());
        assert_eq!(fs.read_dir(5, &mut cx).unwrap()[0].name, "测试.txt");
        assert_eq!(fs.read_link(4, &mut cx).unwrap().unwrap(), b"small.txt");
        let mut actual = vec![0; 20 * block_size as usize + 37];
        fs.read_at(3, 0, &mut actual, &mut cx).unwrap();
        let expected: Vec<_> = (0..actual.len())
            .map(|index| {
                if index as u64 / block_size == 3 {
                    0
                } else {
                    ((index as u64 % block_size * 7 + index as u64 / block_size) % 256) as u8
                }
            })
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(
            fs.timestamps(2, &mut cx).unwrap().created,
            Some((1600000000, 0))
        );
    }
}

#[test]
fn qnx4_extent_chains_long_names_hardlinks_and_ranges() {
    let bytes = unpack(include_bytes!("../tests/fixtures/qnx4.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = Qnx4::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert_eq!(fs.read_dir(8, &mut cx).unwrap().len(), 7);
    assert_eq!(
        fs.lookup(8, "alias.txt", &mut cx).unwrap(),
        fs.lookup(8, "a_long_name_for_qnx4.txt", &mut cx).unwrap()
    );
    let big = fs.lookup(8, "big.bin", &mut cx).unwrap().unwrap();
    let expected: Vec<_> = (0..1300)
        .map(|index| ((index * 13 + 7) % 256) as u8)
        .collect();
    for offset in [0, 500, 1015, 1290, 1300] {
        let mut actual = [0; 2000];
        let size = fs
            .read_at(big, offset as u64, &mut actual, &mut cx)
            .unwrap();
        assert_eq!(&actual[..size], &expected[offset..]);
    }
    let link = fs.lookup(8, "link", &mut cx).unwrap().unwrap();
    assert_eq!(fs.read_link(link, &mut cx).unwrap().unwrap(), b"small.txt");
    let directory = fs.lookup(8, "nested", &mut cx).unwrap().unwrap();
    assert_eq!(fs.read_dir(directory, &mut cx).unwrap()[0].name, "测试.txt");
}

#[test]
fn qnx_corruption_truncation_and_backup_selection() {
    let original = unpack(include_bytes!("../tests/fixtures/qnx6-512-le.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut bytes = original.clone();
    let second = 12288 + 256 * 512;
    bytes[second + 4] ^= 1;
    assert_eq!(
        Qnx6::open(SliceSource::new(&bytes), &mut cx)
            .unwrap()
            .serial(),
        4
    );
    bytes[8192 + 4] ^= 1;
    assert!(Qnx6::open(SliceSource::new(&bytes), &mut cx).is_err());
    let mut bytes = original.clone();
    bytes[8192] ^= 1;
    assert_eq!(
        Qnx6::open(SliceSource::new(&bytes), &mut cx)
            .unwrap()
            .serial(),
        9
    );
    assert!(Qnx6::open(SliceSource::new(&original[..second + 500]), &mut cx).is_err());
    let mut bytes = original.clone();
    bytes[12288 + 9 * 512 + 2] ^= 1;
    let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(fs.read_dir(1, &mut cx).is_err());
    let mut bytes = original;
    bytes[12288 + 12 * 512..12288 + 12 * 512 + 4].copy_from_slice(&999u32.to_le_bytes());
    let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(fs.read_at(3, 0, &mut [0; 1], &mut cx).is_err());
    let mut bytes = unpack(include_bytes!("../tests/fixtures/qnx4.zlib"));
    bytes[7 * 512 + 496] ^= 1;
    assert!(Qnx4::open(SliceSource::new(&bytes), &mut cx).is_err());
}

#[test]
fn qnx6_all_indirect_depths_and_cycles() {
    let original = unpack(include_bytes!("../tests/fixtures/qnx6-bootless-le.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    for levels in 2..=5 {
        let mut bytes = original.clone();
        let mut index = 1u64;
        for digit in 2..levels + 2 {
            index = index * 256 + digit as u64;
        }
        let inode = 4096 + 256;
        bytes[inode..inode + 8].copy_from_slice(&((index + 1) * 1024).to_le_bytes());
        bytes[inode + 36..inode + 100].fill(255);
        bytes[inode + 40..inode + 44].copy_from_slice(&100u32.to_le_bytes());
        bytes[inode + 100] = levels;
        for depth in 0..levels {
            let start = 4096 + (100 + depth as usize) * 1024;
            bytes[start..start + 1024].fill(255);
            let slot = (depth as usize + 2) * 4;
            let next = if depth + 1 == levels {
                200
            } else {
                101 + depth as u32
            };
            bytes[start + slot..start + slot + 4].copy_from_slice(&next.to_le_bytes());
        }
        bytes[4096 + 200 * 1024] = 73;
        let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
        let mut actual = [0; 1];
        fs.read_at(3, index * 1024, &mut actual, &mut cx).unwrap();
        assert_eq!(actual, [73]);
        bytes[4096 + 100 * 1024 + 8..4096 + 100 * 1024 + 12].copy_from_slice(&100u32.to_le_bytes());
        let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
        assert!(fs.read_at(3, index * 1024, &mut actual, &mut cx).is_err());
    }
}

#[test]
fn qnx6_extended_content_is_not_returned_as_plaintext() {
    let mut bytes = unpack(include_bytes!("../tests/fixtures/qnx6-512-le.zlib"));
    bytes[12288 + 128 + 34] = 0x80;
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = Qnx6::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(fs.metadata(2, &mut cx).is_ok());
    assert!(matches!(
        fs.read_at(2, 0, &mut [0; 1], &mut cx),
        Err(FoldError::Unsupported(_))
    ));
}

#[test]
fn qnx6_newer_unsupported_features_are_not_hidden_by_old_generation() {
    let original = unpack(include_bytes!("../tests/fixtures/qnx6-512-le.zlib"));
    let second = 12288 + 256 * 512;
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut bytes = original.clone();
    bytes[second + 24..second + 28].copy_from_slice(&0x80000000u32.to_le_bytes());
    let checksum = qnx6::superblock_checksum(&bytes[second + 8..second + 512]);
    bytes[second + 4..second + 8].copy_from_slice(&checksum.to_le_bytes());
    assert!(matches!(
        Qnx6::open(SliceSource::new(&bytes), &mut cx),
        Err(FoldError::Unsupported(_))
    ));
    let mut bytes = original.clone();
    bytes[second + 32] ^= 1;
    let checksum = qnx6::superblock_checksum(&bytes[second + 8..second + 512]);
    bytes[second + 4..second + 8].copy_from_slice(&checksum.to_le_bytes());
    assert!(Qnx6::open(SliceSource::new(&bytes), &mut cx).is_err());
    let mut bytes = original;
    for offset in [8192, second] {
        bytes[offset + 80..offset + 84].fill(255);
        let checksum = qnx6::superblock_checksum(&bytes[offset + 8..offset + 512]);
        bytes[offset + 4..offset + 8].copy_from_slice(&checksum.to_le_bytes());
    }
    assert!(Qnx6::open(SliceSource::new(&bytes), &mut cx).is_err());
}

#[test]
fn qnx4_cyclic_chain_and_out_of_range_inode_link_fail() {
    let original = unpack(include_bytes!("../tests/fixtures/qnx4.zlib"));
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut bytes = original.clone();
    bytes[1024 + 128 + 48..1024 + 128 + 50].copy_from_slice(&4u16.to_le_bytes());
    bytes[9 * 512..9 * 512 + 4].copy_from_slice(&8u32.to_le_bytes());
    assert!(Qnx4::open(SliceSource::new(&bytes), &mut cx).is_err());
    let mut bytes = original;
    bytes[1024 + 5 * 64 + 52] = 8;
    assert!(Qnx4::open(SliceSource::new(&bytes), &mut cx).is_err());
}
