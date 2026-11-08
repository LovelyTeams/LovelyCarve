use crate::Ocfs2;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let packed: &[u8] = match name {
        "plain" => include_bytes!("../tests/fixtures/plain.zlib"),
        "ecc" => include_bytes!("../tests/fixtures/ecc.zlib"),
        "discontiguous-ecc" => include_bytes!("../tests/fixtures/discontiguous-ecc.zlib"),
        _ => panic!("unknown invented fixture"),
    };
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(packed)
        .read_to_end(&mut raw)
        .unwrap();
    raw
}
fn put16(raw: &mut [u8], at: usize, value: u16) {
    raw[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(raw: &mut [u8], at: usize, value: u32) {
    raw[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(raw: &mut [u8], at: usize, value: u64) {
    raw[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn adjust16(raw: &mut [u8], at: usize, delta: i32) {
    let value = crate::le16(raw, at).unwrap() as i32 + delta;
    put16(raw, at, value.try_into().unwrap());
}
fn adjust32(raw: &mut [u8], at: usize, delta: i64) {
    let value = crate::le32(raw, at).unwrap() as i64 + delta;
    put32(raw, at, value.try_into().unwrap());
}
#[test]
fn independent_images_cover_allocation_units_ecc_holes_links_attributes_and_discontiguous_groups() {
    for name in ["plain", "ecc", "discontiguous-ecc"] {
        let raw = fixture(name);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        assert!(Ocfs2::probe(&mut SliceSource::new(&raw)).unwrap());
        let mut reader = Ocfs2::open(SliceSource::new(&raw), &mut context)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let root = reader.root();
        let children = reader.read_dir(root, &mut context).unwrap();
        assert_eq!(
            children.len(),
            if name == "discontiguous-ecc" { 8 } else { 7 }
        );
        assert_eq!(
            reader.lookup(root, "hello", &mut context).unwrap(),
            reader.lookup(root, "alias", &mut context).unwrap()
        );
        let mut bytes = vec![0; 16384];
        let count = reader.read_at(513, 0, &mut bytes, &mut context).unwrap();
        assert_eq!(&bytes[..count], b"hello OCFS2 evidence\n");
        let count = reader.read_at(514, 0, &mut bytes, &mut context).unwrap();
        assert_eq!(count, 12281);
        assert!(bytes[..4096].iter().all(|byte| *byte == 0));
        assert!(bytes[8192..count].iter().all(|byte| *byte == 0));
        assert_eq!(
            &bytes[4096..8192],
            (0..4096)
                .map(|index| (index * 17 + 5) as u8)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reader.read_at(518, 0, &mut bytes, &mut context).unwrap(),
            8192
        );
        assert!(bytes[..8192].iter().all(|byte| *byte == 0));
        assert_eq!(
            reader.read_link_text(516, &mut context).unwrap().as_deref(),
            Some("sub/nested")
        );
        assert!(reader.read_at(517, 0, &mut bytes, &mut context).is_err());
        let attributes = reader.attributes(513).unwrap();
        assert_eq!(attributes.len(), 2);
        assert_eq!(attributes[0].name, "user.inline");
        assert_eq!(attributes[0].bytes, b"\0\xfffixture");
        assert!(attributes[0].resident);
        assert_eq!(attributes[1].bytes, (0..100u8).collect::<Vec<_>>());
        assert!(!attributes[1].resident);
        let times = reader.timestamps(513, &mut context).unwrap();
        assert_eq!(times.accessed, Some((-315619200, 123)));
        assert_eq!(times.modified, Some((1700000000, 456)));
        assert_eq!(times.changed, Some((1700000001, 789)));
        assert!(times.created.is_none());
        if name == "discontiguous-ecc" {
            let count = reader.read_at(10241, 0, &mut bytes, &mut context).unwrap();
            assert_eq!(&bytes[..count], b"discontiguous inode allocation");
        }
    }
}
#[test]
fn dirty_active_and_truncated_sources_are_never_opened_as_clean() {
    for mutation in 0..7 {
        let mut raw = fixture("plain");
        match mutation {
            0 => put32(&mut raw, 21 * 1024 + 184, 1),
            1 => raw[1024 * 1024 + 31] = 1,
            2 => raw[256 * 1024] = 1,
            3 => put16(&mut raw, 23 * 1024 + 194, 1),
            4 => raw.truncate(8192),
            5 => put32(&mut raw, 2 * 1024 + 224, 0x35c),
            6 => put32(&mut raw, 10 * 1024 + 96, 99),
            _ => unreachable!(),
        }
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        assert!(
            Ocfs2::open(SliceSource::new(&raw), &mut context).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn free_metadata_and_allocated_content_faults_fail_at_the_correct_operation() {
    for mutation in 0..5 {
        let mut raw = fixture("plain");
        match mutation {
            0 => {
                raw[8 * 1024 + 64] &= !2;
                adjust16(&mut raw, 8 * 1024 + 12, 1);
                adjust32(&mut raw, 12 * 1024 + 208, 1);
                adjust32(&mut raw, 12 * 1024 + 184, -1);
            }
            1 => {
                raw[4 * 1024 + 64 + 2304 / 8] &= !(1 << (2304 % 8));
                adjust16(&mut raw, 4 * 1024 + 12, 1);
                adjust32(&mut raw, 15 * 1024 + 208, 1);
                adjust32(&mut raw, 15 * 1024 + 184, -1);
            }
            2 => put64(&mut raw, 514 * 1024 + 216, 16384),
            3 => put64(&mut raw, 514 * 1024 + 216, 512),
            4 => put32(&mut raw, 2 * 1024 + 228, 0),
            _ => unreachable!(),
        }
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let reader = Ocfs2::open(SliceSource::new(&raw), &mut context);
        if mutation == 0 {
            assert!(reader.is_err());
        } else {
            let mut reader = reader.unwrap();
            assert!(
                reader
                    .read_at(
                        if mutation == 4 { 518 } else { 514 },
                        0,
                        &mut [0; 1],
                        &mut context
                    )
                    .is_err(),
                "mutation {mutation}"
            );
        }
    }
}
#[test]
fn non_dot_directory_self_reference_is_rejected() {
    let mut raw = fixture("plain");
    // Replace the ordinary `sub` entry with root itself; its valid `..` is
    // also root, so checking only the child's parent would miss this cycle.
    let at = 9 * 1024 + 200 + 32 + 3 * 20;
    put64(&mut raw, at, 9);
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Ocfs2::open(SliceSource::new(&raw), &mut context).unwrap();
    let root = reader.root();
    assert!(reader.read_dir(root, &mut context).is_err());
}

#[test]
fn metadata_crc_attribute_bounds_and_source_errors_are_not_silenced() {
    let mut raw = fixture("ecc");
    raw[513 * 1024 + 200] ^= 1;
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut reader = Ocfs2::open(SliceSource::new(&raw), &mut context).unwrap();
    assert!(reader.read_at(513, 0, &mut [0; 10], &mut context).is_err());
    for offset in [769 * 1024 + 68, 769 * 1024 + 208] {
        let mut raw = fixture("plain");
        put64(&mut raw, offset, 0);
        let mut reader = Ocfs2::open(SliceSource::new(&raw), &mut context).unwrap();
        assert!(reader.attributes(513).is_err());
    }
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
        Ocfs2::probe(&mut Broken),
        Err(lamfold::FoldError::Io(_))
    ));
}
