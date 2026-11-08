//! Unit tests for the parent module, split out of `attributes.rs` for size.

use super::*;

/// Build an attribute key record: `keyLength | pad | fileID | startBlock |
/// attrNameLen | attrName`.
fn key(file_id: u32, name: &str, start_block: u32) -> Vec<u8> {
    let utf16 = unicode::string_to_utf16(name);
    // keyLength counts everything after the keyLength field itself:
    // `kHFSPlusAttrKeyMinimumLength` is 12, plus two bytes per character.
    let key_length = (12 + utf16.len() * 2) as u16;
    let mut record = key_length.to_be_bytes().to_vec();
    record.extend_from_slice(&0u16.to_be_bytes());
    record.extend_from_slice(&file_id.to_be_bytes());
    record.extend_from_slice(&start_block.to_be_bytes());
    record.extend_from_slice(&(utf16.len() as u16).to_be_bytes());
    for unit in utf16 {
        record.extend_from_slice(&unit.to_be_bytes());
    }
    record
}

fn inline_record(file_id: u32, name: &str, data: &[u8]) -> Vec<u8> {
    let mut record = key(file_id, name, 0);
    record.extend_from_slice(&RECORD_TYPE_INLINE_DATA.to_be_bytes());
    record.extend_from_slice(&[0u8; 8]);
    record.extend_from_slice(&(data.len() as u32).to_be_bytes());
    record.extend_from_slice(data);
    record
}

#[test]
fn parses_inline_value() {
    let record = inline_record(42, "com.apple.decmpfs", b"payload");
    assert_eq!(
        parse_value(&record).unwrap(),
        AttrValue::Inline(b"payload".to_vec())
    );
}

#[test]
fn parses_fork_value() {
    let mut record = key(42, "com.apple.ResourceFork", 0);
    record.extend_from_slice(&RECORD_TYPE_FORK_DATA.to_be_bytes());
    record.extend_from_slice(&0u32.to_be_bytes());
    record.extend_from_slice(&70_000u64.to_be_bytes());
    record.extend_from_slice(&0u32.to_be_bytes());
    record.extend_from_slice(&18u32.to_be_bytes());
    record.extend_from_slice(&100u32.to_be_bytes());
    record.extend_from_slice(&18u32.to_be_bytes());
    record.extend_from_slice(&[0u8; 56]);

    let AttrValue::Fork(fork) = parse_value(&record).unwrap() else {
        panic!("expected a fork value");
    };
    assert_eq!(fork.logical_size, 70_000);
    assert_eq!(fork.total_blocks, 18);
    assert_eq!(fork.extents[0].start_block, 100);
    assert_eq!(fork.extents[0].block_count, 18);
    assert_eq!(fork.extents[1].block_count, 0);
}

#[test]
fn rejects_inline_value_shorter_than_declared() {
    let mut record = key(42, "x", 0);
    record.extend_from_slice(&RECORD_TYPE_INLINE_DATA.to_be_bytes());
    record.extend_from_slice(&[0u8; 8]);
    record.extend_from_slice(&100u32.to_be_bytes());
    record.extend_from_slice(b"short");
    assert!(matches!(
        parse_value(&record),
        Err(HfsPlusError::CorruptedData(_))
    ));
}

#[test]
fn rejects_unknown_record_type() {
    let mut record = key(42, "x", 0);
    record.extend_from_slice(&0x99u32.to_be_bytes());
    record.extend_from_slice(&[0u8; 16]);
    assert!(matches!(
        parse_value(&record),
        Err(HfsPlusError::CorruptedData(_))
    ));
}

/// Ordering must match `hfs_attrkeycompare`: file ID, then a 16-bit binary
/// name comparison, then start block.
#[test]
fn orders_by_file_then_name_then_start_block() {
    use std::cmp::Ordering;
    let name = unicode::string_to_utf16("bbb");

    let cmp = |record: &[u8]| compare_key(record, 42, &name, 0).expect("decodable key");
    assert_eq!(cmp(&key(41, "bbb", 0)), Ordering::Less);
    assert_eq!(cmp(&key(43, "bbb", 0)), Ordering::Greater);
    assert_eq!(cmp(&key(42, "aaa", 0)), Ordering::Less);
    assert_eq!(cmp(&key(42, "ccc", 0)), Ordering::Greater);
    // A prefix sorts before the longer name it prefixes.
    assert_eq!(cmp(&key(42, "bb", 0)), Ordering::Less);
    assert_eq!(cmp(&key(42, "bbbb", 0)), Ordering::Greater);
    assert_eq!(cmp(&key(42, "bbb", 0)), Ordering::Equal);
    assert_eq!(cmp(&key(42, "bbb", 8)), Ordering::Greater);
}

#[test]
fn undecodable_key_is_an_error_not_a_miss() {
    // Too short to hold an attribute key. Ordering it (the old behaviour was
    // `Less`) would steer the descent past the damage and report an attribute
    // that exists as absent.
    let name = unicode::string_to_utf16("bbb");
    assert!(compare_key(&[0u8; 3], 42, &name, 0).is_err());
}

#[test]
fn names_a_record() {
    let record = inline_record(7, "com.apple.quarantine", b"");
    assert_eq!(
        AttrKey::parse(&record).unwrap().to_name().unwrap(),
        "com.apple.quarantine"
    );
}
