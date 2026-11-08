use crate::{inode::Inode, stream::Stream, Header, Inum};
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SubstrateCtx};
use lamfold::{FileKind, SliceSource};
use std::io::Read;

fn put32(raw: &mut [u8], at: usize, value: u32) {
    raw[at..at + 4].copy_from_slice(&value.to_be_bytes());
}
fn put64(raw: &mut [u8], at: usize, value: u64) {
    raw[at..at + 8].copy_from_slice(&value.to_be_bytes());
}
fn prefix() -> Vec<u8> {
    let mut raw = vec![0; 272];
    for (at, value) in [
        (0, 0x01161970),
        (4, 1),
        (16, 100),
        (24, 1802),
        (28, 1900),
        (36, 4096),
        (40, 12),
    ] {
        put32(&mut raw, at, value);
    }
    put64(&mut raw, 48, 2);
    put64(&mut raw, 56, 32);
    put64(&mut raw, 80, 1);
    put64(&mut raw, 88, 33);
    raw
}
#[test]
fn superblock_geometry_distinguishes_gfs1_and_both_gfs2_formats() {
    let mut raw = prefix();
    for version in [1801, 1802] {
        put32(&mut raw, 24, version);
        let header = Header::inspect_prefix(&raw, 1024 * 1024).unwrap();
        assert_eq!(header.format, version);
        assert_eq!(
            header.root,
            Inum {
                formal: 1,
                address: 33
            }
        );
    }
    for (at, value) in [(24, 1309), (24, 1803), (28, 1), (40, 10), (36, 768)] {
        let mut bad = raw.clone();
        put32(&mut bad, at, value);
        assert!(Header::inspect_prefix(&bad, 1024 * 1024).is_err());
    }
    assert!(Header::inspect_prefix(&raw, 65536).is_err());
}
#[test]
fn inode_offsets_stuffed_bytes_and_timestamp_roles_are_independent() {
    let header = Header::inspect_prefix(&prefix(), 1024 * 1024).unwrap();
    let mut image = vec![0; 1024 * 1024];
    let raw = &mut image[33 * 4096..34 * 4096];
    for (at, value) in [
        (0, 0x01161970),
        (4, 4),
        (16, 400),
        (40, 0x81a4),
        (52, 1),
        (176, 123),
        (180, 456),
        (184, 789),
    ] {
        put32(raw, at, value);
    }
    for (at, value) in [
        (24, 1),
        (32, 33),
        (56, 5),
        (64, 1),
        (72, (-315619200i64) as u64),
        (80, 1700000000),
        (88, 1700000001),
    ] {
        put64(raw, at, value);
    }
    raw[232..237].copy_from_slice(b"hello");
    let mut source = SliceSource::new(&image);
    let inode = Inode::read(&mut source, &header, 33).unwrap();
    assert_eq!(inode.kind(), FileKind::Regular);
    assert_eq!(inode.times.accessed, Some((-315619200, 123)));
    assert_eq!(inode.times.modified, Some((1700000000, 456)));
    assert_eq!(inode.times.changed, Some((1700000001, 789)));
    assert!(inode.times.created.is_none());
    let stream = Stream::parse(&mut source, &header, &inode).unwrap();
    let mut bytes = [0; 10];
    assert_eq!(stream.read(&mut source, &header, 2, &mut bytes).unwrap(), 3);
    assert_eq!(&bytes[..3], b"llo");
}
fn invented() -> Vec<u8> {
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(
        include_bytes!("../tests/fixtures/invented-4096.zlib").as_slice(),
    )
    .read_to_end(&mut raw)
    .unwrap();
    raw
}
#[test]
fn a_source_error_is_never_reported_as_a_negative_signature() {
    struct Broken;
    impl lamfold::BlockSource for Broken {
        fn len(&self) -> u64 {
            1024 * 1024
        }
        fn read_at(&mut self, _: u64, _: &mut [u8]) -> lamfold::Result<()> {
            Err(lamfold::FoldError::Io("injected source failure"))
        }
    }
    assert!(matches!(
        crate::Gfs2::probe(&mut Broken),
        Err(lamfold::FoldError::Io(_))
    ));
    assert!(!crate::Gfs2::probe(&mut SliceSource::new(&[0; 65536])).unwrap());
}
#[test]
fn independent_volume_covers_clean_journal_allocation_aliases_holes_links_and_attributes() {
    let raw = invented();
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = crate::Gfs2::open(SliceSource::new(&raw), &mut context).unwrap();
    let root = reader.root();
    let entries = reader.read_dir(root, &mut context).unwrap();
    assert_eq!(entries.len(), 6);
    assert_eq!(
        reader.lookup(root, "hello", &mut context).unwrap(),
        reader.lookup(root, "hardlink", &mut context).unwrap()
    );
    let mut value = vec![0; 64];
    let count = reader.read_at(36, 0, &mut value, &mut context).unwrap();
    assert_eq!(&value[..count], b"hello native-style GFS2\n");
    assert_eq!(
        reader.read_link_text(38, &mut context).unwrap().as_deref(),
        Some("sub/nested")
    );
    assert!(reader.read_at(39, 0, &mut value, &mut context).is_err());
    let attributes = reader.attributes(36).unwrap();
    assert_eq!(attributes.len(), 2);
    assert_eq!(attributes[0].name, "user.inline");
    assert_eq!(attributes[0].bytes, b"\0\xfffixture");
    assert!(attributes[0].resident);
    assert_eq!(attributes[1].bytes, (0..100u8).collect::<Vec<_>>());
    assert!(!attributes[1].resident);
    let mut sparse = vec![0; 3 * 4096];
    let count = reader.read_at(41, 0, &mut sparse, &mut context).unwrap();
    assert_eq!(count, 3 * 4096 - 7);
    assert!(sparse[..4096].iter().all(|byte| *byte == 0));
    assert!(sparse[8192..count].iter().all(|byte| *byte == 0));
    for index in 0..4096 {
        assert_eq!(sparse[4096 + index], (index * 17 + 5) as u8);
    }
}
#[test]
fn dirty_corrupt_short_and_stale_metadata_fail_explicitly() {
    let raw = invented();
    for change in [0, 1, 2, 3, 4, 5, 6] {
        let mut bad = raw.clone();
        match change {
            0 => {
                put32(&mut bad, 512 * 4096 + 32, 0);
                put32(&mut bad, 512 * 4096 + 44, 0);
                let crc = crate::disk_hash(&bad[512 * 4096..512 * 4096 + 48]);
                put32(&mut bad, 512 * 4096 + 44, crc);
            }
            1 => bad[512 * 4096 + 44] ^= 1,
            2 => bad.truncate(65536 + 272),
            3 => {
                bad[17 * 4096 + 128 + (33 - 18) / 4] = 0;
            }
            4 => put64(&mut bad, 100 * 4096 + 24, 100),
            5 => put64(&mut bad, 32 * 4096 + 232, 100),
            6 => {
                // Keep the old clean header, add a later dirty transaction.
                let start = 513 * 4096;
                let clean = bad[512 * 4096..512 * 4096 + 48].to_vec();
                bad[start..start + 48].copy_from_slice(&clean);
                put64(&mut bad, start + 24, 8);
                put32(&mut bad, start + 32, 0);
                put32(&mut bad, start + 40, 1);
                put32(&mut bad, start + 44, 0);
                let crc = crate::disk_hash(&bad[start..start + 48]);
                put32(&mut bad, start + 44, crc);
            }
            _ => unreachable!(),
        }
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        assert!(
            crate::Gfs2::open(SliceSource::new(&bad), &mut context).is_err(),
            "mutation {change}"
        );
    }
}
#[test]
fn exhash_repeated_buckets_collision_chains_and_cross_references_are_checked() {
    fn leaf(raw: &mut [u8], next: u64, values: &[(&str, u64, u64, u16)]) {
        raw.fill(0);
        for (at, value) in [(0, 0x01161970), (4, 6), (16, 600), (28, 1200)] {
            put32(raw, at, value);
        }
        raw[24..26].copy_from_slice(&1u16.to_be_bytes());
        raw[26..28].copy_from_slice(&(values.len() as u16).to_be_bytes());
        put64(raw, 32, next);
        put64(raw, 40, 37);
        let mut at = 104;
        for (index, (name, formal, address, kind)) in values.iter().enumerate() {
            let length = if index + 1 == values.len() {
                raw.len() - at
            } else {
                (40 + name.len()).next_multiple_of(8)
            };
            put64(raw, at, *formal);
            put64(raw, at + 8, *address);
            put32(raw, at + 16, crate::disk_hash(name.as_bytes()));
            raw[at + 20..at + 22].copy_from_slice(&(length as u16).to_be_bytes());
            raw[at + 22..at + 24].copy_from_slice(&(name.len() as u16).to_be_bytes());
            raw[at + 24..at + 26].copy_from_slice(&kind.to_be_bytes());
            raw[at + 40..at + 40 + name.len()].copy_from_slice(name.as_bytes());
            at += length;
        }
    }
    let mut raw = invented();
    let collision = (0..100)
        .map(|index| format!("collision-{index}"))
        .find(|name| crate::disk_hash(name.as_bytes()) < 0x80000000)
        .unwrap();
    assert!(crate::disk_hash(b".") < 0x80000000);
    assert!(crate::disk_hash(b"..") >= 0x80000000);
    assert!(crate::disk_hash(b"nested") >= 0x80000000);
    let start = 37 * 4096;
    put64(&mut raw, start + 56, 32);
    put64(&mut raw, start + 64, 4);
    put32(&mut raw, start + 128, 2);
    put32(&mut raw, start + 148, 4);
    raw[start + 146..start + 148].copy_from_slice(&2u16.to_be_bytes());
    for (index, address) in [400, 400, 402, 402].into_iter().enumerate() {
        put64(&mut raw, start + 232 + index * 8, address);
    }
    leaf(&mut raw[400 * 4096..401 * 4096], 401, &[(".", 7, 37, 4)]);
    leaf(
        &mut raw[401 * 4096..402 * 4096],
        0,
        &[(&collision, 6, 36, 8)],
    );
    leaf(
        &mut raw[402 * 4096..403 * 4096],
        0,
        &[("..", 1, 33, 4), ("nested", 6, 36, 8)],
    );
    let bitmap = 17 * 4096 + 128;
    for address in [400, 401, 402] {
        let relative = address - 18;
        raw[bitmap + relative / 4] |= 1 << ((relative % 4) * 2);
    }
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = crate::Gfs2::open(SliceSource::new(&raw), &mut context).unwrap();
    assert_eq!(reader.read_dir(37, &mut context).unwrap().len(), 2);
    assert_eq!(
        reader.lookup(37, &collision, &mut context).unwrap(),
        Some(36)
    );
    for change in 0..6 {
        let mut bad = raw.clone();
        match change {
            0 => {
                put64(&mut bad, start + 232 + 8, 402);
                put64(&mut bad, start + 232 + 16, 400);
            }
            1 => put64(&mut bad, 400 * 4096 + 32, 400),
            2 => bad[400 * 4096 + 25] = 3,
            3 => bad[402 * 4096 + 104 + 16] ^= 1,
            4 => bad[400 * 4096 + 27] = 2,
            5 => put64(&mut bad, 402 * 4096 + 104, 999),
            _ => unreachable!(),
        }
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut reader = crate::Gfs2::open(SliceSource::new(&bad), &mut context).unwrap();
        assert!(
            reader.read_dir(37, &mut context).is_err(),
            "exhash mutation {change}"
        );
    }
}
