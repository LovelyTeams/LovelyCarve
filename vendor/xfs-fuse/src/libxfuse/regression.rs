//! Independently assembled minimal disk structures; no third-party image data.
use super::{
    compat::{EINVAL, EIO},
    da_btree::hashname,
    volume::Volume,
};
use crc::{Crc, CRC_32_ISCSI};
use std::io::Cursor;

const BLOCK: usize = 4096;
fn put16(raw: &mut [u8], offset: usize, value: u16) {
    raw[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
fn put32(raw: &mut [u8], offset: usize, value: u32) {
    raw[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn put64(raw: &mut [u8], offset: usize, value: u64) {
    raw[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
fn checksum(raw: &mut [u8], offset: usize) {
    raw[offset..offset + 4].fill(0);
    let value = Crc::<u32>::new(&CRC_32_ISCSI).checksum(raw);
    raw[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn extent(start: u64, physical: u64, count: u64, unwritten: bool) -> [u8; 16] {
    ((u128::from(unwritten) << 127)
        | (u128::from(start) << 73)
        | (u128::from(physical) << 21)
        | u128::from(count))
    .to_be_bytes()
}
fn inode(version: u16, number: u64, mode: u16, format: u8, size: u64, count: u32) -> Vec<u8> {
    let mut raw = vec![0; 512];
    put16(&mut raw, 0, 0x494e);
    put16(&mut raw, 2, mode);
    raw[4] = if version == 5 {
        3
    } else if version == 1 {
        1
    } else {
        2
    };
    raw[5] = format;
    put16(&mut raw, 6, 3);
    put32(&mut raw, 8, 1000);
    put32(&mut raw, 12, 1001);
    put32(&mut raw, 16, 3);
    put32(&mut raw, 32, (-2i32) as u32);
    put32(&mut raw, 36, 123);
    put64(&mut raw, 56, size);
    put64(&mut raw, 64, 3);
    put32(&mut raw, 76, count);
    raw[83] = 2;
    if version == 5 {
        put64(&mut raw, 152, number);
        raw[160..176].fill(0x11);
    }
    raw
}
fn store(image: &mut [u8], number: u64, mut raw: Vec<u8>) {
    if raw[4] == 3 {
        checksum(&mut raw, 100);
    }
    let offset = number as usize * 512;
    image[offset..offset + 512].copy_from_slice(&raw);
}
fn core(version: u16) -> usize {
    if version == 5 {
        176
    } else {
        100
    }
}
fn bmap(image: &mut [u8], version: u16, block: usize, owner: u64, records: &[[u8; 16]]) {
    let raw = &mut image[block * BLOCK..(block + 1) * BLOCK];
    put32(raw, 0, if version == 5 { 0x424d4133 } else { 0x424d4150 });
    put16(raw, 6, records.len() as u16);
    put64(raw, 8, u64::MAX);
    put64(raw, 16, u64::MAX);
    let header = if version == 5 {
        put64(raw, 24, (block * BLOCK / 512) as u64);
        raw[40..56].fill(0x11);
        put64(raw, 56, owner);
        72
    } else {
        24
    };
    for (index, record) in records.iter().enumerate() {
        raw[header + index * 16..header + (index + 1) * 16].copy_from_slice(record);
    }
    if version == 5 {
        checksum(raw, 64);
    }
}

pub(crate) fn fixture(version: u16) -> Vec<u8> {
    let mut image = vec![0; 32 * BLOCK];
    let sb = &mut image[..512];
    put32(sb, 0, 0x58465342);
    put32(sb, 4, BLOCK as u32);
    put64(sb, 8, 32);
    sb[32..48].fill(0x11);
    put64(sb, 56, 16);
    put32(sb, 84, 32);
    put32(sb, 88, 1);
    put16(sb, 100, version | if version >= 4 { 0x2000 } else { 0 });
    put16(sb, 102, 512);
    put16(sb, 104, 512);
    put16(sb, 106, 8);
    sb[108..112].copy_from_slice(b"test");
    sb[120] = 12;
    sb[121] = 9;
    sb[122] = 9;
    sb[123] = 3;
    sb[124] = 5;
    if version == 5 {
        put32(sb, 200, 0x100);
        put32(sb, 216, 1);
        checksum(sb, 224);
    }
    let base = core(version);
    let names: Vec<(&[u8], u64)> = vec![
        (b"data", 17),
        (b"link", 18),
        (b"tree", 19),
        (b"attrs", 20),
        (b"device", 21),
        (&[0xff], 17),
    ];
    let mut directory = if version < 4 {
        [16u64.to_be_bytes().as_slice(), &[names.len() as u8]].concat()
    } else {
        vec![names.len() as u8, 0, 0, 0, 0, 16]
    };
    for (index, (name, number)) in names.iter().enumerate() {
        if version < 4 {
            directory.extend_from_slice(&number.to_be_bytes());
            directory.push(name.len() as u8);
            directory.extend_from_slice(name);
        } else {
            directory.push(name.len() as u8);
            directory.extend_from_slice(&(32 + index as u16 * 32).to_be_bytes());
            directory.extend_from_slice(name);
            if version == 5 {
                directory.push(if *number == 18 { 7 } else { 1 });
            }
            directory.extend_from_slice(&(*number as u32).to_be_bytes());
        }
    }
    let mut root = inode(version, 16, 0x41ed, 1, directory.len() as u64, 0);
    root[base..base + directory.len()].copy_from_slice(&directory);
    store(&mut image, 16, root);
    let mut data = inode(version, 17, 0x81a4, 2, (5 * BLOCK - 7) as u64, 3);
    for (index, record) in [
        extent(0, 8, 1, false),
        extent(2, 9, 1, true),
        extent(4, 10, 1, false),
    ]
    .iter()
    .enumerate()
    {
        data[base + index * 16..base + (index + 1) * 16].copy_from_slice(record);
    }
    store(&mut image, 17, data);
    image[8 * BLOCK..9 * BLOCK].fill(b'A');
    image[9 * BLOCK..10 * BLOCK].fill(b'S');
    image[10 * BLOCK..11 * BLOCK].fill(b'Z');
    let mut link = inode(version, 18, 0xa1ff, 1, 4, 0);
    link[base..base + 4].copy_from_slice(b"data");
    store(&mut image, 18, link);
    let mut tree = inode(version, 19, 0x81a4, 3, (5 * BLOCK - 7) as u64, 2);
    put16(&mut tree, base, 1);
    put16(&mut tree, base + 2, 2);
    put64(&mut tree, base + 4, 0);
    put64(&mut tree, base + 12, 4);
    let pointers = base + 4 + ((512 - base - 4) / 16) * 8;
    put64(&mut tree, pointers, 11);
    put64(&mut tree, pointers + 8, 12);
    store(&mut image, 19, tree);
    bmap(&mut image, version, 11, 19, &[extent(0, 8, 1, false)]);
    bmap(&mut image, version, 12, 19, &[extent(4, 10, 1, false)]);
    let mut attrs = inode(version, 20, 0x81a4, 2, 0, 0);
    attrs[82] = 8;
    attrs[83] = 1;
    let fork = base + 64;
    let content = [
        b"same".as_slice(),
        &[0x00, 0xff],
        b"same",
        b"root",
        b"selinux",
        b"label",
    ]
    .concat();
    let mut values = vec![0, 0, 3, 0];
    values.extend_from_slice(&[4, 2, 0]);
    values.extend_from_slice(&content[..6]);
    values.extend_from_slice(&[4, 4, 2]);
    values.extend_from_slice(b"sameroot");
    values.extend_from_slice(&[7, 5, 4]);
    values.extend_from_slice(b"selinuxlabel");
    let size = values.len() as u16;
    put16(&mut values, 0, size);
    attrs[fork..fork + values.len()].copy_from_slice(&values);
    store(&mut image, 20, attrs);
    let mut device = inode(version, 21, 0x21a4, 0, 0, 0);
    put32(&mut device, base, 0x801);
    store(&mut image, 21, device);
    image
}

#[test]
fn versions_one_to_five_paths_metadata_and_reads() {
    for version in 1..=5 {
        let mut volume = Volume::open(Cursor::new(fixture(version))).unwrap();
        assert_eq!(volume.superblock().version(), version);
        assert_eq!(&volume.superblock().label[..4], b"test");
        assert_eq!(volume.lookup_path_bytes(b"/\xff").unwrap(), 17);
        assert!(volume
            .readdir(16)
            .unwrap()
            .iter()
            .any(|entry| entry.name == [0xff]));
        let attributes = volume.getattr(17).unwrap();
        assert_eq!(
            (
                attributes.uid,
                attributes.gid,
                attributes.nlink,
                attributes.mode
            ),
            (1000, 1001, 3, 0x81a4)
        );
        assert_eq!(attributes.crtime.is_some(), version == 5);
        assert_eq!(
            attributes
                .atime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_err()
                .duration(),
            std::time::Duration::new(1, 999_999_900)
        );
        assert_eq!(attributes.raw_atime, (-2, 123));
        assert_eq!(volume.getattr(21).unwrap().rdev, 0x801);
        assert_eq!(volume.readlink(18).unwrap(), b"data");
        let data = volume.read(17, 0, u32::MAX).unwrap();
        assert_eq!(data.len(), 5 * BLOCK - 7);
        assert!(data[..BLOCK].iter().all(|b| *b == b'A'));
        assert!(data[BLOCK..4 * BLOCK].iter().all(|b| *b == 0));
        assert!(data[4 * BLOCK..].iter().all(|b| *b == b'Z'));
        assert_eq!(volume.read(19, 0, u32::MAX).unwrap(), data);
        for (offset, size) in [
            (4093, 8),
            (8190, 9000),
            (16380, 1000),
            (20470, 100),
            (30000, 10),
        ] {
            let start = (offset as usize).min(data.len());
            let end = (start + size as usize).min(data.len());
            assert_eq!(volume.read(17, offset, size).unwrap(), data[start..end]);
        }
        assert_eq!(volume.read(17, -1, 1), Err(EINVAL));
        let extents = volume.extents(17).unwrap();
        assert_eq!(extents.len(), 3);
        assert!(extents[1].unwritten);
        assert_eq!(
            volume.get_extended_attribute(20, b"user.same").unwrap(),
            [0, 255]
        );
        assert_eq!(
            volume.get_extended_attribute(20, b"trusted.same").unwrap(),
            b"root"
        );
        assert_eq!(
            volume
                .get_extended_attribute(20, b"security.selinux")
                .unwrap(),
            b"label"
        );
    }
}

#[test]
fn malformed_metadata_and_truncated_data_fail() {
    for version in [4, 5] {
        let mut image = fixture(version);
        image.truncate(8 * BLOCK + 20);
        let mut volume = Volume::open(Cursor::new(image)).unwrap();
        assert_eq!(volume.read(17, 0, 40), Err(EIO));
        let mut image = fixture(version);
        image[17 * 512 + core(version)..17 * 512 + core(version) + 16]
            .copy_from_slice(&extent(0, 31, 2, false));
        if version == 5 {
            checksum(&mut image[17 * 512..18 * 512], 100);
        }
        let mut volume = Volume::open(Cursor::new(image)).unwrap();
        assert_eq!(volume.read(17, 0, 40), Err(EIO));
    }
    for offset in [100, 120, 122, 124, 4] {
        let mut image = fixture(4);
        image[offset] = 0xff;
        assert!(Volume::open(Cursor::new(image)).is_err());
    }
    assert!(Volume::open(Cursor::new(vec![0; 20])).is_err());
    let mut image = fixture(5);
    image[17 * 512 + 20] ^= 1;
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.getattr(17).is_err());
    let mut image = fixture(5);
    image[11 * BLOCK + 72] ^= 1;
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.read(19, 0, 1).is_err());
}

#[test]
fn independent_volume_context_and_metadata_uuid() {
    let mut first = Volume::open(Cursor::new(fixture(4))).unwrap();
    let mut image = fixture(5);
    image[32..48].fill(0x22);
    put32(&mut image, 216, 5);
    image[248..264].fill(0x11);
    checksum(&mut image[..512], 224);
    let mut second = Volume::open(Cursor::new(image)).unwrap();
    assert_eq!(first.read(19, 0, 4).unwrap(), b"AAAA");
    assert_eq!(second.read(19, 0, 4).unwrap(), b"AAAA");
    assert_eq!(first.readdir(16).unwrap().len(), 8);
    assert_eq!(second.readdir(16).unwrap().len(), 8);
}

#[test]
fn namespace_collision_names_are_distinct() {
    // A real XFS hash collision, unlike comparing only same names in different namespaces.
    assert_eq!(hashname(b"aA"), hashname(b"`\xc1"));
}

fn da_header(raw: &mut [u8], version: u16, physical: usize, magic: u16, owner: u64) -> usize {
    put16(raw, 8, magic);
    if version == 5 {
        put64(raw, 16, (physical * BLOCK / 512) as u64);
        raw[32..48].fill(0x11);
        put64(raw, 48, owner);
        56
    } else {
        12
    }
}
fn attribute_image(version: u16, branch: bool, btree: bool) -> Vec<u8> {
    let mut image = fixture(version);
    let base = core(version);
    let mut attrs = inode(version, 20, 0x81a4, 2, 0, 0);
    attrs[82] = 8;
    attrs[83] = if btree { 3 } else { 2 };
    put16(&mut attrs, 80, 1);
    let fork = base + 64;
    if btree {
        put16(&mut attrs, fork, 1);
        put16(&mut attrs, fork + 2, 1);
        put64(&mut attrs, fork + 4, 0);
        let pointer = fork + 4 + ((512 - fork - 4) / 16) * 8;
        put64(&mut attrs, pointer, 17);
        bmap(&mut image, version, 17, 20, &[extent(0, 13, 4, false)]);
    } else {
        attrs[fork..fork + 16].copy_from_slice(&extent(0, 13, 4, false));
    }
    store(&mut image, 20, attrs);
    if branch {
        let raw = &mut image[13 * BLOCK..14 * BLOCK];
        let header = da_header(
            raw,
            version,
            13,
            if version == 5 { 0x3ebe } else { 0xfebe },
            20,
        );
        put16(raw, header, 1);
        put16(raw, header + 2, 1);
        let entries = if version == 5 { 64 } else { 16 };
        put32(raw, entries, u32::MAX);
        put32(raw, entries + 4, 1);
        if version == 5 {
            checksum(raw, 12);
        }
    }
    let physical = if branch { 14 } else { 13 };
    let raw = &mut image[physical * BLOCK..(physical + 1) * BLOCK];
    let header = da_header(
        raw,
        version,
        physical,
        if version == 5 { 0x3bee } else { 0xfbee },
        20,
    );
    put16(raw, header, 4);
    let entries = if version == 5 { 80 } else { 32 };
    let mut offset = 512;
    for (index, (name, value, flags)) in [
        (b"aA".as_slice(), b"first".as_slice(), 1),
        (b"`\xc1", b"second", 1),
        (b"aA", b"secure", 5),
    ]
    .iter()
    .enumerate()
    {
        let entry = entries + index * 8;
        put32(raw, entry, hashname(name));
        put16(raw, entry + 4, offset as u16);
        raw[entry + 6] = *flags;
        put16(raw, offset, value.len() as u16);
        raw[offset + 2] = name.len() as u8;
        raw[offset + 3..offset + 3 + name.len()].copy_from_slice(name);
        raw[offset + 3 + name.len()..offset + 3 + name.len() + value.len()].copy_from_slice(value);
        offset += 64;
    }
    let entry = entries + 24;
    put32(raw, entry, hashname(b"remote"));
    put16(raw, entry + 4, offset as u16);
    put32(raw, offset, 2);
    put32(raw, offset + 4, 6000);
    raw[offset + 8] = 6;
    raw[offset + 9..offset + 15].copy_from_slice(b"remote");
    if version == 5 {
        checksum(raw, 12);
    }
    let mut logical_offset = 0;
    for physical in [15, 16] {
        let raw = &mut image[physical * BLOCK..(physical + 1) * BLOCK];
        let header = if version == 5 { 56 } else { 0 };
        let count = (BLOCK - header).min(6000 - logical_offset);
        if version == 5 {
            put32(raw, 0, 0x5841524d);
            put32(raw, 4, logical_offset as u32);
            put32(raw, 8, count as u32);
            raw[16..32].fill(0x11);
            put64(raw, 32, 20);
            put64(raw, 40, (physical * BLOCK / 512) as u64);
        }
        for index in 0..count {
            raw[header + index] = ((logical_offset + index) % 251) as u8;
        }
        logical_offset += count;
        if version == 5 {
            checksum(raw, 12);
        }
    }
    image
}

#[test]
fn leaf_node_btree_remote_attributes_collisions_and_provenance() {
    for version in [4, 5] {
        for branch in [false, true] {
            for btree in [false, true] {
                let mut volume =
                    Volume::open(Cursor::new(attribute_image(version, branch, btree))).unwrap();
                assert_eq!(
                    volume.get_extended_attribute(20, b"user.aA").unwrap(),
                    b"first"
                );
                assert_eq!(
                    volume.get_extended_attribute(20, b"user.`\xc1").unwrap(),
                    b"second"
                );
                assert_eq!(
                    volume.get_extended_attribute(20, b"security.aA").unwrap(),
                    b"secure"
                );
                let expected: Vec<_> = (0..6000).map(|i| (i % 251) as u8).collect();
                assert_eq!(
                    volume.get_extended_attribute(20, b"user.remote").unwrap(),
                    expected
                );
                assert_eq!(
                    volume
                        .read_extended_attribute(20, b"user.remote", 4038, 12)
                        .unwrap(),
                    expected[4038..4050]
                );
                let attributes = volume.extended_attributes(20).unwrap();
                let remote = attributes
                    .iter()
                    .find(|a| a.name == b"user.remote")
                    .unwrap();
                assert_eq!(remote.extents.len(), 2);
                assert_eq!(remote.extents.iter().map(|r| r.length).sum::<u64>(), 6000);
                assert_eq!(
                    remote.extents[0].physical_offset,
                    15 * BLOCK as u64 + if version == 5 { 56 } else { 0 }
                );
            }
        }
    }
    let mut image = attribute_image(5, true, false);
    image[15 * BLOCK + 56] ^= 1;
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.extended_attributes(20).is_err());
    let mut image = attribute_image(4, true, false);
    put32(&mut image, 13 * BLOCK + 20, 0);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.extended_attributes(20).is_err());
}

#[test]
fn external_symlink_length_headers_and_offsets() {
    for version in [4, 5] {
        let mut image = fixture(version);
        let length = if version == 5 { 600 } else { 4 };
        let base = core(version);
        let mut link = inode(version, 18, 0xa1ff, 2, length, 1);
        link[base..base + 16].copy_from_slice(&extent(0, 13, 1, false));
        store(&mut image, 18, link);
        let raw = &mut image[13 * BLOCK..14 * BLOCK];
        let header = if version == 5 { 56 } else { 0 };
        if version == 5 {
            put32(raw, 0, 0x58534c4d);
            put32(raw, 8, length as u32);
            raw[16..32].fill(0x11);
            put64(raw, 32, 18);
            put64(raw, 40, (13 * BLOCK / 512) as u64);
        }
        raw[header..header + length as usize].fill(b'x');
        if version == 5 {
            checksum(raw, 12);
        }
        let mut volume = Volume::open(Cursor::new(image.clone())).unwrap();
        assert_eq!(volume.readlink(18).unwrap(), vec![b'x'; length as usize]);
        if version == 5 {
            put32(&mut image, 13 * BLOCK + 4, 10);
            checksum(&mut image[13 * BLOCK..14 * BLOCK], 12);
            let mut volume = Volume::open(Cursor::new(image)).unwrap();
            assert!(volume.readlink(18).is_err());
        }
    }
}

#[test]
fn legacy_leaf_node_directories_and_cycles() {
    for version in 1..=3 {
        for branch in [false, true] {
            let mut image = fixture(version);
            let mut root = inode(version, 16, 0x41ed, 2, BLOCK as u64, 1);
            root[100..116].copy_from_slice(&extent(0, 13, 2, false));
            store(&mut image, 16, root);
            if branch {
                let raw = &mut image[13 * BLOCK..14 * BLOCK];
                put16(raw, 8, 0xfebe);
                put16(raw, 12, 1);
                put16(raw, 14, 1);
                put32(raw, 20, 1);
            }
            let physical = if branch { 14 } else { 13 };
            let raw = &mut image[physical * BLOCK..(physical + 1) * BLOCK];
            put16(raw, 8, 0xfeeb);
            put16(raw, 12, 1);
            put32(raw, 32, hashname(b"data"));
            put16(raw, 36, 512);
            raw[38] = 4;
            put64(raw, 512, 17);
            raw[520..524].copy_from_slice(b"data");
            let mut volume = Volume::open(Cursor::new(image.clone())).unwrap();
            assert_eq!(volume.lookup_path("/data").unwrap(), 17);
            if branch {
                put32(&mut image, 13 * BLOCK + 20, 0);
                let mut volume = Volume::open(Cursor::new(image)).unwrap();
                assert!(volume.readdir(16).is_err());
            }
        }
    }
}

#[test]
fn v2_v3_directory_data_and_block_tail_validate() {
    for version in [4, 5] {
        for block_format in [false, true] {
            let mut image = fixture(version);
            let base = core(version);
            let mut root = inode(version, 16, 0x41ed, 2, BLOCK as u64, 1);
            root[base..base + 16].copy_from_slice(&extent(0, 13, 1, false));
            store(&mut image, 16, root);
            let raw = &mut image[13 * BLOCK..14 * BLOCK];
            let header = if version == 5 {
                put32(raw, 0, if block_format { 0x58444233 } else { 0x58444433 });
                put64(raw, 8, (13 * BLOCK / 512) as u64);
                raw[24..40].fill(0x11);
                put64(raw, 40, 16);
                64
            } else {
                put32(raw, 0, if block_format { 0x58443242 } else { 0x58443244 });
                16
            };
            put64(raw, header, 17);
            raw[header + 8] = 4;
            raw[header + 9..header + 13].copy_from_slice(b"data");
            if version == 5 {
                raw[header + 13] = 1;
            }
            put16(raw, header + 14, header as u16);
            let end = if block_format { BLOCK - 16 } else { BLOCK };
            let free = header + 16;
            put16(raw, free, 0xffff);
            put16(raw, free + 2, (end - free) as u16);
            put16(raw, end - 2, free as u16);
            if block_format {
                put32(raw, BLOCK - 8, 1);
            }
            if version == 5 {
                checksum(raw, 4);
            }
            let mut volume = Volume::open(Cursor::new(image.clone())).unwrap();
            assert_eq!(volume.lookup_path("/data").unwrap(), 17);
            image[13 * BLOCK + header + 14] ^= 1;
            if version == 5 {
                checksum(&mut image[13 * BLOCK..14 * BLOCK], 4);
            }
            let mut volume = Volume::open(Cursor::new(image)).unwrap();
            assert!(volume.readdir(16).is_err());
        }
    }
}

#[test]
fn large_counters_bigtime_feature_flags_and_realtime_failures() {
    let mut image = fixture(5);
    put32(&mut image, 216, 1 | 8 | 32);
    checksum(&mut image[..512], 224);
    let raw = &mut image[17 * 512..18 * 512];
    put64(raw, 24, 3);
    put64(raw, 120, 8 | 16);
    let time = (2_147_483_648u64 + 5_000_000_000) * 1_000_000_000 + 123_456_789;
    put64(raw, 32, time);
    checksum(raw, 100);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert_eq!(
        volume.getattr(17).unwrap().raw_atime,
        (5_000_000_000, 123_456_789)
    );
    assert_eq!(volume.read(17, 0, 3).unwrap(), b"AAA");
    for flags in [0x10, 0x100, 0x200, 0x400, 0x8000] {
        let mut image = fixture(5);
        put32(&mut image, 216, flags);
        checksum(&mut image[..512], 224);
        assert!(Volume::open(Cursor::new(image)).is_err());
    }
    let mut image = fixture(4);
    put16(&mut image, 17 * 512 + 90, 1);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.read(17, 0, 1).is_err());
    let mut image = fixture(4);
    put16(&mut image, 100, 0xa004);
    put32(&mut image, 200, 0x200);
    // MOREBITS is 0x8000, not 0x4000; FTYPE must be visible on V4.
    let volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.sb.has_ftype());
}

#[test]
fn identity_checks_do_not_depend_only_on_crc() {
    for offset in [152, 160] {
        let mut image = fixture(5);
        image[17 * 512 + offset] ^= 1;
        checksum(&mut image[17 * 512..18 * 512], 100);
        let mut volume = Volume::open(Cursor::new(image)).unwrap();
        assert!(volume.getattr(17).is_err());
    }
    for offset in [24, 40, 56] {
        let mut image = fixture(5);
        image[11 * BLOCK + offset] ^= 1;
        checksum(&mut image[11 * BLOCK..12 * BLOCK], 64);
        let mut volume = Volume::open(Cursor::new(image)).unwrap();
        assert!(volume.read(19, 0, 1).is_err());
    }
    let mut image = fixture(4);
    put32(&mut image, 17 * 512 + 36, 1_000_000_000);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.getattr(17).is_err());
    let mut image = fixture(4);
    put16(&mut image, 100, 0x6004);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.lookup_path("/DATA").unwrap(), 17);
}

#[test]
fn parent_attribute_records_and_incomplete_values_remain_distinct() {
    let mut image = fixture(5);
    let base = core(5);
    let mut attrs = inode(5, 20, 0x81a4, 2, 0, 0);
    attrs[82] = 8;
    attrs[83] = 1;
    let mut content = vec![0, 0, 3, 0];
    for parent in [10u64, 11] {
        content.extend_from_slice(&[4, 12, 8]);
        content.extend_from_slice(b"name");
        content.extend_from_slice(&parent.to_be_bytes());
        content.extend_from_slice(&1u32.to_be_bytes());
    }
    content.extend_from_slice(&[4, 1, 0x80]);
    content.extend_from_slice(b"tempX");
    let length = content.len() as u16;
    put16(&mut content, 0, length);
    attrs[base + 64..base + 64 + content.len()].copy_from_slice(&content);
    store(&mut image, 20, attrs);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    let attributes = volume.extended_attributes(20).unwrap();
    assert_eq!(attributes.len(), 2);
    assert_eq!(attributes[0].name, b"parent.name");
    assert_eq!(attributes[0].flags, 8);
    assert_ne!(attributes[0].value, attributes[1].value);
    assert!(volume.get_extended_attribute(20, b"parent.name").is_err());
}

#[test]
fn small_block_multiblock_symlink_offsets_are_logical() {
    let mut image = fixture(5);
    put32(&mut image, 4, 512);
    put64(&mut image, 8, 256);
    put32(&mut image, 84, 256);
    put16(&mut image, 106, 1);
    image[120] = 9;
    image[123] = 0;
    image[124] = 8;
    checksum(&mut image[..512], 224);
    let mut link = inode(5, 18, 0xa1ff, 2, 1024, 1);
    link[176..192].copy_from_slice(&extent(0, 13, 3, false));
    store(&mut image, 18, link);
    let expected: Vec<_> = (0..1024).map(|i| b'a' + (i % 26) as u8).collect();
    let mut offset = 0;
    for block in 13..=15 {
        let raw = &mut image[block * 512..(block + 1) * 512];
        let count = 456.min(1024 - offset);
        put32(raw, 0, 0x58534c4d);
        put32(raw, 4, offset as u32);
        put32(raw, 8, count as u32);
        raw[16..32].fill(0x11);
        put64(raw, 32, 18);
        put64(raw, 40, block as u64);
        raw[56..56 + count].copy_from_slice(&expected[offset..offset + count]);
        offset += count;
        checksum(raw, 12);
    }
    let mut volume = Volume::open(Cursor::new(image.clone())).unwrap();
    assert_eq!(volume.readlink(18).unwrap(), expected);
    put32(&mut image, 14 * 512 + 4, 0);
    checksum(&mut image[14 * 512..15 * 512], 12);
    let mut volume = Volume::open(Cursor::new(image)).unwrap();
    assert!(volume.readlink(18).is_err());
}
