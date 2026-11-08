//! Unit tests for the parent module, split out of `btree.rs` for size.

use super::*;
use crate::fletcher;
use std::io::Cursor;

const BLOCK_SIZE: u32 = 4096;

/// Build a single-entry root leaf node in a checksummed 4 KiB block.
///
/// Layout: object header (32) | node header (24) | TOC (8) | key area |
/// ... | value | BTreeInfo (40, root only). One variable-size entry:
/// key = 8-byte LE u64, value = 8-byte LE u64.
fn synthetic_root_leaf(key: u64, value: u64) -> Vec<u8> {
    let mut block = vec![0u8; BLOCK_SIZE as usize];

    // Object header: checksum filled last; oid 1, xid 1, type BTREE, subtype 0.
    block[8..16].copy_from_slice(&1u64.to_le_bytes());
    block[16..24].copy_from_slice(&1u64.to_le_bytes());
    block[24..28].copy_from_slice(&object::OBJECT_TYPE_BTREE.to_le_bytes());

    // Node header at 32: root+leaf, level 0, one key, TOC space 8 bytes.
    let nh = ObjectHeader::SIZE;
    block[nh..nh + 2].copy_from_slice(&(BTNODE_ROOT | BTNODE_LEAF).to_le_bytes());
    block[nh + 4..nh + 8].copy_from_slice(&1u32.to_le_bytes());
    block[nh + 10..nh + 12].copy_from_slice(&8u16.to_le_bytes());

    // TOC at 56: key_off 0, key_len 8, val_off 8, val_len 8.
    let toc = nh + BTreeNodeHeader::SIZE;
    block[toc..toc + 2].copy_from_slice(&0u16.to_le_bytes());
    block[toc + 2..toc + 4].copy_from_slice(&8u16.to_le_bytes());
    block[toc + 4..toc + 6].copy_from_slice(&8u16.to_le_bytes());
    block[toc + 6..toc + 8].copy_from_slice(&8u16.to_le_bytes());

    // Key area at 64.
    let key_area = toc + 8;
    block[key_area..key_area + 8].copy_from_slice(&key.to_le_bytes());

    // Value grows down from val_area_end = block end minus BTreeInfo.
    let val_area_end = BLOCK_SIZE as usize - BTreeInfo::SIZE;
    block[val_area_end - 8..val_area_end].copy_from_slice(&value.to_le_bytes());

    // BTreeInfo: node_size, one key, one node; sizes 0 = variable.
    block[val_area_end + 4..val_area_end + 8].copy_from_slice(&BLOCK_SIZE.to_le_bytes());
    block[val_area_end + 24..val_area_end + 32].copy_from_slice(&1u64.to_le_bytes());
    block[val_area_end + 32..val_area_end + 40].copy_from_slice(&1u64.to_le_bytes());

    let checksum = fletcher::fletcher64(&block[8..]);
    block[..8].copy_from_slice(&checksum.to_le_bytes());
    block
}

fn compare_to(search: u64) -> impl Fn(&[u8]) -> Result<std::cmp::Ordering> {
    move |key: &[u8]| {
        let k = u64::from_le_bytes(key.try_into().expect("8-byte key"));
        Ok(k.cmp(&search))
    }
}

#[test]
fn lookup_reads_a_checksummed_node() {
    let block = synthetic_root_leaf(42, 7);
    let mut reader = Cursor::new(block);
    let found = btree_lookup(&mut reader, 0, BLOCK_SIZE, 0, 0, &compare_to(42), None)
        .expect("lookup on a valid node");
    assert_eq!(found.as_deref(), Some(&7u64.to_le_bytes()[..]));
}

#[test]
fn lookup_rejects_a_corrupt_node() {
    let mut block = synthetic_root_leaf(42, 7);
    block[100] ^= 0xFF;
    let mut reader = Cursor::new(block);
    let err = btree_lookup(&mut reader, 0, BLOCK_SIZE, 0, 0, &compare_to(42), None)
        .expect_err("corrupt node must not be traversed");
    assert!(matches!(err, ApfsError::InvalidChecksum), "{err:?}");
}

#[test]
fn comparator_errors_propagate_instead_of_reading_as_a_miss() {
    let block = synthetic_root_leaf(42, 7);
    let mut reader = Cursor::new(block);
    let failing = |_key: &[u8]| -> Result<std::cmp::Ordering> {
        Err(ApfsError::CorruptedData("undecodable key".into()))
    };
    let err = btree_lookup(&mut reader, 0, BLOCK_SIZE, 0, 0, &failing, None)
        .expect_err("an undecodable key must fail the lookup, not report a miss");
    assert!(matches!(err, ApfsError::CorruptedData(_)), "{err:?}");
}

#[test]
fn scan_rejects_a_corrupt_node() {
    let mut block = synthetic_root_leaf(42, 7);
    block[100] ^= 0xFF;
    let mut reader = Cursor::new(block);
    let err = btree_scan(&mut reader, 0, BLOCK_SIZE, 0, 0, &compare_to(42), None)
        .expect_err("corrupt node must not be scanned");
    assert!(matches!(err, ApfsError::InvalidChecksum), "{err:?}");
}

#[test]
fn bulk_walk_preserves_cancellation_and_rejects_corrupt_nodes() {
    let block = synthetic_root_leaf(42, 7);
    let mut reader = Cursor::new(block.clone());
    let mut records = Vec::new();
    let completed = btree_walk_leaves(&mut reader, 0, BLOCK_SIZE, None, &mut |key, value| {
        records.push((key.to_vec(), value.to_vec()));
        false
    })
    .unwrap();
    assert!(!completed);
    assert_eq!(
        records,
        vec![(42u64.to_le_bytes().to_vec(), 7u64.to_le_bytes().to_vec())]
    );

    let mut corrupt = block;
    corrupt[100] ^= 0xff;
    let mut reader = Cursor::new(corrupt);
    let result = btree_walk_leaves(&mut reader, 0, BLOCK_SIZE, None, &mut |_, _| {
        panic!("a corrupt node must fail before publishing catalog records")
    });
    assert!(matches!(result, Err(ApfsError::InvalidChecksum)));
}
