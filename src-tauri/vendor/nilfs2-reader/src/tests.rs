use crate::{
    logs::{Logs, Payload},
    Header, Nilfs2,
};
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

pub(crate) fn inflate(packed: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(packed)
        .read_to_end(&mut raw)
        .unwrap();
    raw
}
fn fixture(block: usize) -> Vec<u8> {
    inflate(match block {
        1024 => include_bytes!("../tests/fixtures/invented-1024.zlib"),
        4096 => include_bytes!("../tests/fixtures/invented-4096.zlib"),
        65536 => include_bytes!("../tests/fixtures/invented-65536.zlib"),
        _ => panic!("invented geometry"),
    })
}
fn open(raw: &[u8]) -> Nilfs2<SliceSource<'_>> {
    let mut cache = BlockCache::new(0);
    let mut context = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    Nilfs2::open(SliceSource::new(raw), &mut context).unwrap()
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
fn super_crc(raw: &mut [u8], at: usize) {
    put32(raw, at + 16, 0);
    let seed = crate::le32(raw, at + 12).unwrap();
    let checksum = crate::crc(seed, &raw[at..at + 280]);
    put32(raw, at + 16, checksum);
}
fn log_crc(raw: &mut [u8]) {
    let header = Header::inspect(&mut SliceSource::new(raw)).unwrap();
    let block = header.block_size as usize;
    let start = header.last_partial_segment as usize * block;
    let count = crate::le32(raw, start + 40).unwrap() as usize;
    let summary = crate::le32(raw, start + 48).unwrap() as usize;
    let root = start + (count - 1) * block;
    let checksum = crate::crc(header.crc_seed, &raw[root + 4..root + 400]);
    put32(raw, root, checksum);
    let checksum = crate::crc(header.crc_seed, &raw[start + 8..start + summary]);
    put32(raw, start + 4, checksum);
    let checksum = crate::crc(header.crc_seed, &raw[start + 4..start + count * block]);
    put32(raw, start, checksum);
}
fn payload(raw: &[u8], matches: impl Fn(Payload) -> bool) -> (usize, Payload) {
    let mut source = SliceSource::new(raw);
    let header = Header::inspect(&mut source).unwrap();
    let mut logs = Logs::new();
    let root = logs.latest_root(&mut source, &header).unwrap();
    for block in header.last_partial_segment..root {
        if let Ok(identity) = logs.identity(&mut source, &header, block) {
            if matches(identity) {
                return (block as usize * header.block_size as usize, identity);
            }
        }
    }
    panic!("missing invented payload")
}
#[test]
fn independent_geometries_cover_direct_tree_sparse_allocators_names_and_checkpoint_views() {
    for block in [1024, 4096, 65536] {
        let raw = fixture(block);
        let mut reader = open(&raw);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let root = reader.root();
        let entries = reader.read_dir(root, &mut context).unwrap();
        assert_eq!(entries.len(), 9);
        assert_eq!(
            reader.lookup(root, "hello", &mut context).unwrap(),
            reader.lookup(root, "alias", &mut context).unwrap()
        );
        assert_eq!(
            reader.lookup(root, "far", &mut context).unwrap(),
            Some(block as u64 * 8 + 11)
        );
        let mut bytes = vec![0; 8 * block];
        let count = reader.read_at(11, 0, &mut bytes, &mut context).unwrap();
        assert_eq!(&bytes[..count], b"hello NILFS2 evidence\n");
        let count = reader.read_at(12, 0, &mut bytes, &mut context).unwrap();
        assert_eq!(count, block * 4 - 7);
        assert!(bytes[..block].iter().all(|byte| *byte == 0));
        assert!(bytes[2 * block..3 * block].iter().all(|byte| *byte == 0));
        assert_eq!(
            &bytes[block..2 * block],
            (0..block)
                .map(|index| (index * 23 + 7) as u8)
                .collect::<Vec<_>>()
        );
        let count = reader.read_at(17, 0, &mut bytes, &mut context).unwrap();
        assert_eq!(count, block * 7 - 13);
        assert_eq!(
            &bytes[..count],
            (0..count)
                .map(|index| (index * 17 + 5) as u8)
                .collect::<Vec<_>>()
        );
        let mut range = vec![0; 77];
        assert_eq!(
            reader
                .read_at(17, (block * 4 - 9) as u64, &mut range, &mut context)
                .unwrap(),
            77
        );
        assert_eq!(range, bytes[block * 4 - 9..block * 4 + 68]);
        assert_eq!(
            reader.read_link(13, &mut context).unwrap().unwrap(),
            b"sub/nested"
        );
        assert!(reader.read_at(15, 0, &mut bytes, &mut context).is_err());
        let times = reader.timestamps(11, &mut context).unwrap();
        assert!(times.accessed.is_none() && times.created.is_none());
        assert_eq!(times.modified, Some((-315619200, 456)));
        assert_eq!(times.changed, Some((1700000001, 123)));
        assert_eq!(
            reader
                .snapshots()
                .unwrap()
                .iter()
                .map(|checkpoint| checkpoint.number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(reader.checkpoint().blocks, (block != 65536).then_some(42));
        for (number, expected) in [
            (1, b"snapshot-one".as_slice()),
            (2, b"snapshot-two".as_slice()),
            (3, b"current".as_slice()),
        ] {
            let mut history = Nilfs2::open_checkpoint(SliceSource::new(&raw), number).unwrap();
            let count = history.read_at(16, 0, &mut bytes, &mut context).unwrap();
            assert_eq!(&bytes[..count], expected);
        }
    }
}
#[test]
fn superblock_selection_never_hides_a_newer_dirty_or_unsupported_view() {
    // Early valid volumes can fill the device with segments and have no spare.
    let mut primary_only = fixture(1024);
    primary_only.truncate(primary_only.len() - 4096);
    let length = primary_only.len() as u64;
    put64(&mut primary_only, 1024 + 32, length);
    super_crc(&mut primary_only, 1024);
    assert_eq!(open(&primary_only).header().selected_super_offset, 1024);
    let mut raw = fixture(1024);
    raw[1024 + 16] ^= 1;
    let reader = open(&raw);
    assert_eq!(
        reader.header().selected_super_offset,
        (raw.len() - 4096) as u64
    );
    for mutation in 0..5 {
        let mut raw = fixture(1024);
        let backup = raw.len() - 4096;
        match mutation {
            0 => {
                put16(&mut raw, 1024 + 116, 0);
                put64(&mut raw, backup + 56, 2);
                super_crc(&mut raw, 1024);
                super_crc(&mut raw, backup);
            }
            1 => {
                for at in [1024, backup] {
                    put64(&mut raw, at + 272, 1);
                    super_crc(&mut raw, at);
                }
            }
            2 => raw.truncate(raw.len() - 1),
            3 => {
                raw[1024 + 16] ^= 1;
                raw[backup + 16] ^= 1;
            }
            4 => {
                let at = 4 * 1024 + 4;
                raw[at] ^= 1;
            }
            _ => unreachable!(),
        }
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        assert!(
            Nilfs2::open(SliceSource::new(&raw), &mut context).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn valid_crc_does_not_override_allocation_ownership_tree_or_snapshot_consistency() {
    for mutation in 0..6 {
        let mut raw = fixture(1024);
        let (node, _) = payload(&raw, |identity| {
            matches!(identity, Payload::Node { owner: 17, .. })
        });
        let (_, hello) = payload(&raw, |identity| {
            matches!(
                identity,
                Payload::Data {
                    owner: 11,
                    logical: 0,
                    ..
                }
            )
        });
        let Payload::Data {
            virtual_block: hello_virtual,
            ..
        } = hello
        else {
            unreachable!()
        };
        match mutation {
            0 => put64(&mut raw, node + 16 + 63 * 8, hello_virtual),
            1 => put64(&mut raw, node + 16 + 8, 0),
            2 => {
                let (bitmap, _) = payload(&raw, |identity| {
                    identity == (Payload::DatData { logical: 1 })
                });
                let (descriptor, _) = payload(&raw, |identity| {
                    identity == (Payload::DatData { logical: 0 })
                });
                raw[bitmap + hello_virtual as usize / 8] &= !(1 << (hello_virtual % 8));
                let free = crate::le32(&raw, descriptor).unwrap();
                put32(&mut raw, descriptor, free + 1);
            }
            3 => {
                let (bitmap, _) = payload(&raw, |identity| {
                    matches!(
                        identity,
                        Payload::Data {
                            owner: 6,
                            logical: 1,
                            ..
                        }
                    )
                });
                let (descriptor, _) = payload(&raw, |identity| {
                    matches!(
                        identity,
                        Payload::Data {
                            owner: 6,
                            logical: 0,
                            ..
                        }
                    )
                });
                raw[bitmap + 11 / 8] &= !(1 << (11 % 8));
                let free = crate::le32(&raw, descriptor).unwrap();
                put32(&mut raw, descriptor, free + 1);
            }
            4 => {
                let (usage, _) = payload(&raw, |identity| {
                    matches!(
                        identity,
                        Payload::Data {
                            owner: 5,
                            logical: 0,
                            ..
                        }
                    )
                });
                put32(&mut raw, usage + 44, 0);
            }
            5 => {
                let (cp, _) = payload(&raw, |identity| {
                    matches!(
                        identity,
                        Payload::Data {
                            owner: 4,
                            logical: 0,
                            ..
                        }
                    )
                });
                put64(&mut raw, cp + 192 + 8, 1);
            }
            _ => unreachable!(),
        }
        log_crc(&mut raw);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let reader = Nilfs2::open(SliceSource::new(&raw), &mut context);
        if mutation == 4 {
            assert!(reader.is_err());
            continue;
        }
        let mut reader = reader.unwrap();
        match mutation {
            0 | 1 => assert!(reader.read_at(17, 0, &mut [0; 1], &mut context).is_err()),
            2 => assert!(reader.read_at(11, 0, &mut [0; 1], &mut context).is_err()),
            3 => assert!(reader.read_dir(2, &mut context).is_err()),
            5 => assert!(reader.snapshots().is_err()),
            4 => unreachable!(),
            _ => unreachable!(),
        }
    }
}
