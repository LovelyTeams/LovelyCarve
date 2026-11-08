use super::*;
use std::io::{Cursor, Read, Seek, SeekFrom};

const BASIC: &[u8] = include_bytes!("../tests/data/basic.apfs");
const CASE_SENSITIVE: &[u8] = include_bytes!("../tests/data/case-sensitive.apfs");

#[test]
fn checkpoint_transaction_and_volume_slots_are_preserved() {
    let info = ApfsVolume::container_info(Cursor::new(BASIC)).unwrap();
    assert_eq!(info.transaction_id, 10);
    assert_eq!(info.size, 65536);
    assert_eq!(info.volume_slots, [(0, 100), (2, 101)]);
    let volumes = ApfsVolume::list_volumes(Cursor::new(BASIC)).unwrap();
    assert_eq!(volumes.len(), 2);
    assert_eq!(volumes[1].index, 2);
    assert!(volumes[1].encrypted);
    assert_eq!(volumes[0].role, 64);
    assert_eq!(
        volumes[0].uuid,
        core::array::from_fn(|index| 16 + index as u8)
    );
    assert!(matches!(
        ApfsVolume::open_index(Cursor::new(BASIC), 2),
        Err(ApfsError::Unsupported(_))
    ));
    assert!(matches!(
        ApfsVolume::open_index(Cursor::new(BASIC), 1),
        Err(ApfsError::NoVolume)
    ));
}

#[test]
fn full_streamed_and_seekable_reads_agree_for_clones_sparse_and_compressed_files() {
    let mut volume = ApfsVolume::open(Cursor::new(BASIC)).unwrap();
    let entries = volume.list_directory("/").unwrap();
    assert_eq!(entries.len(), 5);
    assert!(entries.iter().any(|entry| entry.name == "plain.bin"));
    assert!(
        !entries
            .iter()
            .any(|entry| entry.name == "old.bin" || entry.name == "future.bin")
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap()
            .link_target
            .as_deref(),
        Some("../plain.bin")
    );
    for name in [
        "plain.bin",
        "clone.bin",
        "hole.bin",
        "compressed.txt",
        "link",
    ] {
        let path = format!("/{name}");
        let complete = volume.read_file(&path).unwrap();
        let stat = volume.stat(&path).unwrap();
        assert_eq!(stat.size, complete.len() as u64);
        let mut streamed = Vec::new();
        assert_eq!(
            volume.read_file_to(&path, &mut streamed).unwrap(),
            stat.size
        );
        assert_eq!(streamed, complete);
        for offset in [0, 1, 8, 4095, 4096, 4999, 5000, 8192, u64::MAX] {
            let actual = volume.read_file_range(&path, offset, 30).unwrap();
            let expected = complete.get(offset as usize..).unwrap_or_default();
            assert_eq!(
                actual,
                expected[..expected.len().min(30)],
                "{path} offset={offset}"
            );
        }
        let mut by_id = Vec::new();
        volume
            .open_file_by_id(stat.oid)
            .unwrap()
            .read_to_end(&mut by_id)
            .unwrap();
        assert_eq!(by_id, complete);
        assert_eq!(volume.stat_by_id(stat.oid).unwrap().size, stat.size);
    }
    assert_eq!(
        volume.read_file("/clone.bin").unwrap(),
        volume.read_file("/plain.bin").unwrap()
    );
    assert_eq!(
        volume.read_file("/compressed.txt").unwrap(),
        b"hello compressed"
    );
    assert_eq!(volume.read_file("/hole.bin").unwrap(), vec![0; 8192]);
    let stat = volume.stat("/plain.bin").unwrap();
    assert_eq!(
        (stat.parent_oid, stat.private_id, stat.uid, stat.gid),
        (2, 70, 501, 20)
    );
    assert_eq!(stat.create_time, -1);
    assert!(volume.open_file("/").is_err());
}

#[test]
fn attributes_are_independent_seekable_streams() {
    let mut volume = ApfsVolume::open(Cursor::new(BASIC)).unwrap();
    assert_eq!(
        volume
            .get_xattr("/plain.bin", "user.small")
            .unwrap()
            .unwrap(),
        b"value"
    );
    assert_eq!(
        volume
            .get_xattr("/plain.bin", "user.stream")
            .unwrap()
            .unwrap(),
        b"ATTR!!"
    );
    assert!(volume.get_xattr("/plain.bin", "absent").unwrap().is_none());
    let info = volume
        .xattr_info("/plain.bin", "user.stream")
        .unwrap()
        .unwrap();
    assert_eq!(info.stream_id, Some(80));
    assert_eq!(info.size, 6);
    assert_eq!(info.extents[0].value.phys_block_num, 14);
    let mut attribute = volume
        .open_xattr("/plain.bin", "user.stream")
        .unwrap()
        .unwrap();
    attribute.seek(SeekFrom::Start(2)).unwrap();
    let mut bytes = Vec::new();
    attribute.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"TR!!");
}

#[test]
fn both_directory_key_layouts_and_volume_name_policies_work() {
    let mut insensitive = ApfsVolume::open(Cursor::new(BASIC)).unwrap();
    assert_eq!(insensitive.read_file("/PLAIN.BIN").unwrap().len(), 5000);
    let mut sensitive = ApfsVolume::open(Cursor::new(CASE_SENSITIVE)).unwrap();
    assert_eq!(sensitive.list_directory("/").unwrap().len(), 5);
    assert_eq!(sensitive.read_file("/plain.bin").unwrap().len(), 5000);
    assert!(sensitive.read_file("/PLAIN.BIN").is_err());
}

#[test]
fn omap_cache_and_lookup_obey_the_same_transaction_and_tombstone_rules() {
    let mut source = Cursor::new(BASIC.to_vec());
    for (xid, expected) in [(5, 9), (10, 10), (11, 11)] {
        let tree = omap::OmapTree { root: 8, xid };
        assert_eq!(
            omap::omap_lookup(&mut source, tree, 4096, 200).unwrap(),
            expected
        );
        assert_eq!(
            omap::load_omap(&mut source, tree, 4096).unwrap().get(&200),
            Some(&expected)
        );
    }
    // The first value grows down from the end; xid=11 is the third value.
    let block = &mut source.get_mut()[8 * 4096..9 * 4096];
    let start = 4096 - 40 - 3 * 16;
    block[start..start + 4].copy_from_slice(&1u32.to_le_bytes());
    let sum = fletcher::fletcher64(&block[8..]);
    block[..8].copy_from_slice(&sum.to_le_bytes());
    let tree = omap::OmapTree { root: 8, xid: 11 };
    assert!(omap::omap_lookup(&mut source, tree, 4096, 200).is_err());
    assert!(
        !omap::load_omap(&mut source, tree, 4096)
            .unwrap()
            .contains_key(&200)
    );
}

#[test]
fn geometry_checksum_and_checkpoint_identity_are_validated() {
    for (offset, value) in [(36, 0u32), (36, u32::MAX), (180, 101)] {
        let mut bytes = BASIC.to_vec();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let checksum = fletcher::fletcher64(&bytes[8..4096]);
        bytes[..8].copy_from_slice(&checksum.to_le_bytes());
        assert!(superblock::read_nxsb(&mut Cursor::new(bytes)).is_err());
    }
    assert!(ApfsVolume::open(Cursor::new(&BASIC[..BASIC.len() - 1])).is_err());
    let mut bytes = BASIC.to_vec();
    bytes[4096 + 72] ^= 1;
    let checksum = fletcher::fletcher64(&bytes[4096 + 8..8192]);
    bytes[4096..4104].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        ApfsVolume::container_info(Cursor::new(bytes))
            .unwrap()
            .transaction_id,
        5
    );

    let mut bytes = BASIC.to_vec();
    bytes[36..40].copy_from_slice(&8192u32.to_le_bytes());
    bytes[40..48].copy_from_slice(&8u64.to_le_bytes());
    bytes[4096..8192].fill(0);
    let checksum = fletcher::fletcher64(&bytes[8..8192]);
    bytes[..8].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        superblock::read_nxsb(&mut Cursor::new(bytes))
            .unwrap()
            .block_size,
        8192
    );
}

#[test]
fn malformed_btree_offsets_and_counts_return_errors_without_panics() {
    let valid = &BASIC[10 * 4096..11 * 4096];
    for (offset, payload) in [
        (36, u32::MAX.to_le_bytes().to_vec()),
        (42, u16::MAX.to_le_bytes().to_vec()),
        (60, u16::MAX.to_le_bytes().to_vec()),
    ] {
        let mut block = valid.to_vec();
        block[offset..offset + payload.len()].copy_from_slice(&payload);
        if let Ok(node) = btree::BTreeNode::parse(&block) {
            assert!(node.value(0, 0).is_err());
        }
    }
}

#[test]
fn unicode_lookup_policy_preserves_case_and_canonical_equivalence() {
    let context = catalog::CatalogContext {
        omap: 0.into(),
        incompatible_features: 9,
    };
    assert_eq!(context.name_key("CAFÉ"), context.name_key("cafe\u{301}"));
    assert_eq!(context.name_key("Straße"), context.name_key("STRASSE"));
    let sensitive = catalog::CatalogContext {
        incompatible_features: 8,
        ..context
    };
    assert_ne!(
        sensitive.name_key("CAFÉ"),
        sensitive.name_key("cafe\u{301}")
    );
}

#[test]
fn snapshot_metadata_keeps_identity_times_and_superblock_provenance() {
    let mut volume = ApfsVolume::open(Cursor::new(BASIC)).unwrap();
    let snapshots = volume.list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].name, "snap-one");
    assert_eq!(snapshots[0].transaction_id, 5);
    assert_eq!(snapshots[0].superblock_block, 4);
    assert_eq!(snapshots[0].create_time, -1);
    assert_eq!(snapshots[0].change_time, 123456789);
}

#[test]
fn file_extent_cannot_use_bytes_after_the_declared_container() {
    let mut source = BASIC.to_vec();
    source.extend_from_slice(&[b'X'; 8192]);
    let mut volume = ApfsVolume::open(Cursor::new(source)).unwrap();
    let records = vec![catalog::FileExtentRecord {
        logical_addr: 0,
        value: catalog::FileExtentVal {
            flags_and_length: 4096,
            phys_block_num: 16,
            crypto_id: 0,
        },
    }];
    assert!(extents::ApfsForkReader::new(&mut volume.reader, 4096, records, 4096).is_err());
}

#[test]
fn omap_predecessor_follows_the_correct_leaf_across_a_version_boundary() {
    fn node(level: u16, records: &[(u64, u64, u64)], root: bool) -> Vec<u8> {
        let mut block = vec![0u8; 4096];
        block[24..28].copy_from_slice(&object::OBJECT_TYPE_BTREE.to_le_bytes());
        let flags = btree::BTNODE_FIXED_KV_SIZE
            | if level == 0 { btree::BTNODE_LEAF } else { 0 }
            | if root { btree::BTNODE_ROOT } else { 0 };
        block[32..34].copy_from_slice(&flags.to_le_bytes());
        block[34..36].copy_from_slice(&level.to_le_bytes());
        block[36..40].copy_from_slice(&(records.len() as u32).to_le_bytes());
        block[42..44].copy_from_slice(&((records.len() * 4) as u16).to_le_bytes());
        let value_end = if root { 4056 } else { 4096 };
        let key_base = 56 + records.len() * 4;
        let value_size = if level == 0 { 16 } else { 8 };
        for (index, &(oid, xid, address)) in records.iter().enumerate() {
            block[56 + index * 4..58 + index * 4]
                .copy_from_slice(&((index * 16) as u16).to_le_bytes());
            block[58 + index * 4..60 + index * 4]
                .copy_from_slice(&(((index + 1) * value_size) as u16).to_le_bytes());
            let key = key_base + index * 16;
            block[key..key + 8].copy_from_slice(&oid.to_le_bytes());
            block[key + 8..key + 16].copy_from_slice(&xid.to_le_bytes());
            let value = value_end - (index + 1) * value_size;
            if level == 0 {
                block[value + 4..value + 8].copy_from_slice(&4096u32.to_le_bytes());
                block[value + 8..value + 16].copy_from_slice(&address.to_le_bytes());
            } else {
                block[value..value + 8].copy_from_slice(&address.to_le_bytes());
            }
        }
        if root {
            block[4060..4064].copy_from_slice(&4096u32.to_le_bytes());
            block[4064..4068].copy_from_slice(&16u32.to_le_bytes());
            block[4068..4072].copy_from_slice(&16u32.to_le_bytes());
        }
        let checksum = fletcher::fletcher64(&block[8..]);
        block[..8].copy_from_slice(&checksum.to_le_bytes());
        block
    }
    let mut source = Cursor::new(
        [
            node(1, &[(200, 5, 1), (200, 10, 2)], true),
            node(0, &[(200, 5, 9)], false),
            node(0, &[(200, 10, 10), (200, 11, 11)], false),
        ]
        .concat(),
    );
    for (xid, address) in [(5, 9), (9, 9), (10, 10), (11, 11)] {
        assert_eq!(
            omap::omap_lookup(&mut source, omap::OmapTree { root: 0, xid }, 4096, 200).unwrap(),
            address
        );
    }
}
