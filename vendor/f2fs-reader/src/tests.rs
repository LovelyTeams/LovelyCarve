use super::*;
use lamfold::{BlockCache, NoVerifier, SliceSource};

fn synthetic() -> Vec<u8> {
    miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(
        "../tests/fixtures/synthetic.f2fs.zlib"
    ))
    .unwrap()
}
fn context(cache: &mut BlockCache) -> SubstrateCtx<'_> {
    SubstrateCtx {
        cache,
        verifier: &NoVerifier,
    }
}

#[test]
fn f2fs_real_mkfs_syzkaller_corpus() {
    let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(
        "../tests/fixtures/syzkaller_0.f2fs.zlib"
    ))
    .unwrap();
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let root = fs.root();
    for (path, size, zero) in [
        ("/file1", 10, false),
        ("/file2", 9000, true),
        ("/file3", 9000, true),
        ("/file.cold", 100, false),
        ("/file0/file0", 1050, false),
    ] {
        let node = lamfold::resolve(&mut fs, &mut cx, path).unwrap();
        let mut output = vec![0; size];
        assert_eq!(fs.read_at(node, 0, &mut output, &mut cx).unwrap(), size);
        let expected: Vec<_> = (0..size)
            .map(|index| if zero { 0 } else { b"syzkaller"[index % 9] })
            .collect();
        assert_eq!(output, expected, "{path}");
    }
    assert_eq!(
        fs.lookup(root, "file2", &mut cx).unwrap(),
        fs.lookup(root, "file3", &mut cx).unwrap()
    );
}
fn checksum_inode(bytes: &mut [u8], nid: u32) {
    let base = (4096 + nid as usize - 3) * 4096;
    let node = &mut bytes[base..base + 4096];
    node[368..372].fill(0);
    let mut crc = crc32_raw(u32::MAX, &(0..16).collect::<Vec<u8>>());
    crc = crc32_raw(crc, &nid.to_le_bytes());
    crc = crc32_raw(crc, &node[68..72]);
    let crc = crc32_raw(crc, node);
    node[368..372].copy_from_slice(&crc.to_le_bytes());
}
fn checksum_checkpoint(bytes: &mut [u8], base: usize) {
    let head = &mut bytes[base * 4096..(base + 1) * 4096];
    let crc = crc32_raw(0xf2f52010, &head[..4092]);
    head[4092..].copy_from_slice(&crc.to_le_bytes());
    let head = head.to_vec();
    bytes[(base + 3) * 4096..(base + 4) * 4096].copy_from_slice(&head);
}

#[test]
fn f2fs_inline_indirect_sparse_links_and_timestamps() {
    let bytes = synthetic();
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let entries = fs.read_dir(3, &mut cx).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(
        fs.lookup(3, "small.txt", &mut cx).unwrap(),
        fs.lookup(3, "alias.txt", &mut cx).unwrap()
    );
    let mut small = [0; 13];
    assert_eq!(fs.read_at(4, 0, &mut small, &mut cx).unwrap(), 13);
    assert_eq!(&small, b"hello F2FS!\r\n");
    assert_eq!(fs.read_link(6, &mut cx).unwrap().unwrap(), b"small.txt");
    assert_eq!(fs.read_dir(7, &mut cx).unwrap()[0].name, "测试.txt");
    let times = fs.timestamps(4, &mut cx).unwrap();
    assert_eq!(times.modified, Some((1700000000, 123456789)));
    assert_eq!(times.created, Some((1600000000, 987654321)));
    for offset in [
        0,
        4090,
        12280,
        16380,
        20470,
        913 * 4096 - 9,
        1000 * 4096 + 30,
    ] {
        let mut buffer = [0; 97];
        let actual = fs.read_at(5, offset, &mut buffer, &mut cx).unwrap();
        for (index, byte) in buffer[..actual].iter().enumerate() {
            let logical = offset + index as u64;
            assert_eq!(
                *byte,
                if (3..5).contains(&(logical / 4096)) {
                    0
                } else {
                    (logical % 256) as u8
                }
            );
        }
    }
    let mut buffer = [0; 1];
    assert_eq!(
        fs.read_at(5, 1000 * 4096 + 37, &mut buffer, &mut cx)
            .unwrap(),
        0
    );
}

#[test]
fn f2fs_legacy_inline_data_and_directory_reserve_different_xattr_space() {
    let mut bytes = synthetic();
    // Before flexible xattrs, only inline-xattr and inline-directory flags
    // reserve 50 addresses. Inline data alone has the full remaining area.
    for offset in [1024, 5120] {
        bytes[offset + 0x884..offset + 0x888].copy_from_slice(&0x128u32.to_le_bytes());
    }
    let small = 4097 * 4096;
    bytes[small + 3] = 0x22;
    bytes[small + 16..small + 24].copy_from_slice(&3500u64.to_le_bytes());
    bytes[small + 388..small + 388 + 3500].fill(0x61);
    checksum_inode(&mut bytes, 4);

    let root = 4096 * 4096;
    bytes[root + 3] = 0x24;
    let inline = &mut bytes[root + 388..root + 4052];
    inline.fill(0);
    // 3464-byte legacy inline directory: 181 slots, 23-byte bitmap,
    // two reserved bytes, then 181 eleven-byte entries and eight-byte names.
    inline[0] = 3;
    inline[29..33].copy_from_slice(&4u32.to_le_bytes());
    inline[33..35].copy_from_slice(&10u16.to_le_bytes());
    inline[35] = 1;
    inline[2016..2026].copy_from_slice(b"legacy.txt");
    checksum_inode(&mut bytes, 3);

    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let entries = fs.read_dir(3, &mut cx).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "legacy.txt");
    let mut output = vec![0; 3500];
    assert_eq!(fs.read_at(4, 0, &mut output, &mut cx).unwrap(), 3500);
    assert!(output.iter().all(|byte| *byte == b'a'));
}

#[test]
fn f2fs_checkpoint_fallback_bitmap_and_journal() {
    let mut bytes = synthetic();
    bytes[1027 * 4096] ^= 1;
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    assert_eq!(
        F2fs::open(SliceSource::new(&bytes), &mut cx)
            .unwrap()
            .cp
            .version,
        1
    );
    bytes[515 * 4096] ^= 1;
    assert!(F2fs::open(SliceSource::new(&bytes), &mut cx).is_err());
    let mut bytes = synthetic();
    let nat = bytes[2560 * 4096..2561 * 4096].to_vec();
    bytes[3072 * 4096..3073 * 4096].copy_from_slice(&nat);
    bytes[2560 * 4096..2561 * 4096].fill(0);
    bytes[1024 * 4096 + 0xc0 + 64] = 0x80;
    checksum_checkpoint(&mut bytes, 1024);
    assert_eq!(
        F2fs::open(SliceSource::new(&bytes), &mut cx)
            .unwrap()
            .read_dir(3, &mut cx)
            .unwrap()
            .len(),
        5
    );
    let mut bytes = synthetic();
    bytes[2560 * 4096 + 4 * 9..2560 * 4096 + 5 * 9].fill(0);
    let summary = &mut bytes[1025 * 4096..1026 * 4096];
    summary[..2].copy_from_slice(&1u16.to_le_bytes());
    summary[2..6].copy_from_slice(&4u32.to_le_bytes());
    summary[7..11].copy_from_slice(&4u32.to_le_bytes());
    summary[11..15].copy_from_slice(&4097u32.to_le_bytes());
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let mut small = [0; 13];
    fs.read_at(4, 0, &mut small, &mut cx).unwrap();
    assert_eq!(&small, b"hello F2FS!\r\n");
}

#[test]
fn f2fs_superblock_checksums_and_backup_copy() {
    let mut bytes = synthetic();
    for offset in [1024, 5120] {
        let sb = &mut bytes[offset..offset + 3072];
        sb[0x20..0x24].copy_from_slice(&3068u32.to_le_bytes());
        sb[0x884..0x888].copy_from_slice(&0x968u32.to_le_bytes());
        let crc = crc32_raw(0xf2f52010, &sb[..3068]);
        sb[3068..].copy_from_slice(&crc.to_le_bytes());
    }
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    assert!(F2fs::open(SliceSource::new(&bytes), &mut cx).is_ok());
    bytes[1024..1028].fill(0);
    assert!(F2fs::probe(&mut SliceSource::new(&bytes)).unwrap());
    assert!(F2fs::open(SliceSource::new(&bytes), &mut cx).is_ok());
    bytes[5120 + 0x100] ^= 1;
    assert!(F2fs::open(SliceSource::new(&bytes), &mut cx).is_err());
}

#[test]
fn f2fs_corruption_and_unsupported_content_fail_explicitly() {
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut bytes = synthetic();
    bytes[4097 * 4096 + 388] ^= 1;
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(fs.metadata(4, &mut cx).is_err());
    let mut bytes = synthetic();
    bytes[4098 * 4096 + 384..4098 * 4096 + 388].copy_from_slice(&8193u32.to_le_bytes());
    checksum_inode(&mut bytes, 5);
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    assert!(fs.read_at(5, 0, &mut [0; 1], &mut cx).is_err());
    for (offset, value) in [(2, 4), (80, 4)] {
        let mut bytes = synthetic();
        bytes[4097 * 4096 + offset] |= value;
        checksum_inode(&mut bytes, 4);
        let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
        assert!(fs.metadata(4, &mut cx).is_ok());
        assert!(matches!(
            fs.read_at(4, 0, &mut [0; 1], &mut cx),
            Err(FoldError::Unsupported(_))
        ));
    }
    let bytes = synthetic();
    assert!(F2fs::open(SliceSource::new(&bytes[..bytes.len() - 1]), &mut cx).is_err());
}

#[test]
#[ignore = "requires F2FS_IMAGE"]
fn f2fs_external_checkpoint_tree() {
    let bytes = std::fs::read(std::env::var_os("F2FS_IMAGE").expect("F2FS_IMAGE")).unwrap();
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let root = fs.root();
    let entries = fs.read_dir(root, &mut cx).unwrap();
    for entry in entries {
        println!(
            "{} {:?} {:?}",
            entry.name,
            entry.kind,
            fs.metadata(entry.node, &mut cx).unwrap()
        );
    }
}

#[test]
fn f2fs_all_indirect_levels_use_owner_checked_nodes() {
    let mut bytes = synthetic();
    let double_start = 913 + 2 * 1018 + 2 * 1018u64.pow(2);
    let base = 4098 * 4096;
    bytes[base + 16..base + 24].copy_from_slice(&((double_start + 1) * 4096).to_le_bytes());
    bytes[base + 4060..base + 4064].copy_from_slice(&10u32.to_le_bytes());
    bytes[base + 4068..base + 4072].copy_from_slice(&12u32.to_le_bytes());
    for (nid, value) in [(10u32, 11u32), (11, 6000), (12, 13), (13, 14), (14, 6001)] {
        let block = 4096 + nid as usize - 3;
        let node = &mut bytes[block * 4096..(block + 1) * 4096];
        node.fill(0);
        node[..4].copy_from_slice(&value.to_le_bytes());
        node[4072..4076].copy_from_slice(&nid.to_le_bytes());
        node[4076..4080].copy_from_slice(&5u32.to_le_bytes());
        let offset = 2560 * 4096 + nid as usize * 9;
        bytes[offset + 1..offset + 5].copy_from_slice(&5u32.to_le_bytes());
        bytes[offset + 5..offset + 9].copy_from_slice(&(block as u32).to_le_bytes());
    }
    bytes[6000 * 4096] = 42;
    bytes[6001 * 4096] = 73;
    checksum_inode(&mut bytes, 5);
    let mut cache = BlockCache::new(0);
    let mut cx = context(&mut cache);
    let mut fs = F2fs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let mut buffer = [0; 1];
    fs.read_at(5, (913 + 2 * 1018) * 4096, &mut buffer, &mut cx)
        .unwrap();
    assert_eq!(buffer, [42]);
    fs.read_at(5, double_start * 4096, &mut buffer, &mut cx)
        .unwrap();
    assert_eq!(buffer, [73]);
}
