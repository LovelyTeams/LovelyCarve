//! Unit tests for the parent module, split out of `catalog.rs` for size.

use super::*;

#[test]
fn catalog_keys_compare_equal_only_when_oid_and_type_match() {
    // The scan predicates depend on this: a record is in range exactly when
    // the comparison is Equal, so they need no separate oid/type equality
    // check alongside the ordering.
    for oid_a in 0..4u64 {
        for type_a in 0..4u8 {
            for oid_b in 0..4u64 {
                for type_b in 0..4u8 {
                    let equal = compare_catalog_keys(oid_a, type_a, oid_b, type_b)
                        == std::cmp::Ordering::Equal;
                    assert_eq!(
                        equal,
                        oid_a == oid_b && type_a == type_b,
                        "({oid_a},{type_a}) vs ({oid_b},{type_b})"
                    );
                }
            }
        }
    }
}

#[test]
fn parses_xattr_value_header() {
    // flags=0x0006 (embedded), data_len=0x000e, data = "../README.txt\0"
    let value = [
        0x06, 0x00, 0x0e, 0x00, b'.', b'.', b'/', b'R', b'E', b'A', b'D', b'M', b'E', b'.', b't',
        b'x', b't', b'\0',
    ];
    assert_eq!(
        parse_xattr_value(&value).unwrap(),
        XattrValue::Embedded(b"../README.txt\0".to_vec())
    );
    let short = [0x06, 0x00, 0xff, 0x00, b'a', b'b'];
    assert!(parse_xattr_value(&short).is_err());
    // value shorter than the header: rejected rather than passed through
    assert!(parse_xattr_value(b"abc").is_err());
    // data-stream reference shorter than a j_xattr_dstream_t
    let dstream = [0x01, 0x00, 0x10, 0x00, 0xAA, 0xBB, 0xCC, 0xDD];
    assert!(parse_xattr_value(&dstream).is_err());
}

#[test]
fn parses_xattr_data_stream_reference() {
    // flags=XATTR_DATA_STREAM, then j_xattr_dstream_t: xattr_obj_id then a
    // j_dstream_t whose first field is the size.
    let mut value = vec![0x01, 0x00, 0x30, 0x00];
    value.extend_from_slice(&0x1234_5678_u64.to_le_bytes());
    value.extend_from_slice(&99_u64.to_le_bytes());
    value.extend_from_slice(&[0u8; 32]);

    assert_eq!(
        parse_xattr_value(&value).unwrap(),
        XattrValue::DataStream {
            obj_id: 0x1234_5678,
            size: 99,
        }
    );
}

/// Build an on-disk catalog key: `obj_id_and_type | name_len u16 | name | NUL`.
fn xattr_key(oid: u64, j_type: u8, name: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(8 + 2 + name.len() + 1);
    let obj_id_and_type = (oid & 0x0FFFFFFFFFFFFFFF) | ((j_type as u64) << 60);
    key.extend_from_slice(&obj_id_and_type.to_le_bytes());
    key.extend_from_slice(&((name.len() + 1) as u16).to_le_bytes());
    key.extend_from_slice(name.as_bytes());
    key.push(0);
    key
}

#[test]
fn xattr_key_layout_matches_on_disk_format() {
    let key = xattr_key(25, J_TYPE_XATTR, SYMLINK_XATTR_NAME);
    assert_eq!(key.len(), 31);
    assert_eq!(key[0], 0x19); // oid 25, low byte
    assert_eq!(key[7], 0x40); // J_TYPE_XATTR nibble
    assert_eq!(&key[8..10], &[0x15, 0x00]); // name_len 21, incl. NUL
    assert_eq!(&key[XATTR_KEY_NAME_OFFSET..], b"com.apple.fs.symlink\0");
}

#[test]
fn compares_xattr_keys_by_oid_then_type_then_name() {
    use std::cmp::Ordering;
    // The comparator matches against the NUL-terminated attribute name.
    let mut want = SYMLINK_XATTR_NAME.as_bytes().to_vec();
    want.push(0);

    let cmp = |key: &[u8], oid: u64| compare_xattr_key(key, oid, &want).expect("decodable key");

    // Exact match.
    assert_eq!(
        cmp(&xattr_key(25, J_TYPE_XATTR, SYMLINK_XATTR_NAME), 25),
        Ordering::Equal
    );

    // OID dominates, and is compared numerically -- not as little-endian
    // bytes, which would order 0x100 before 0x02.
    assert_eq!(
        cmp(&xattr_key(0x100, J_TYPE_XATTR, SYMLINK_XATTR_NAME), 0x02),
        Ordering::Greater
    );
    assert_eq!(
        cmp(&xattr_key(0x02, J_TYPE_XATTR, SYMLINK_XATTR_NAME), 0x100),
        Ordering::Less
    );

    // Same OID: the record type breaks the tie, even though it is packed
    // into the high nibble of obj_id_and_type.
    assert_eq!(cmp(&xattr_key(25, J_TYPE_INODE, ""), 25), Ordering::Less);
    assert_eq!(
        cmp(&xattr_key(25, J_TYPE_DIR_REC, ""), 25),
        Ordering::Greater
    );

    // Same OID and type: names order by bytes, ignoring the name_len field
    // that precedes them -- a longer name can still sort first.
    assert_eq!(
        cmp(
            &xattr_key(25, J_TYPE_XATTR, "com.apple.diskimages.recentcksum"),
            25
        ),
        Ordering::Less
    );
    assert_eq!(
        cmp(&xattr_key(25, J_TYPE_XATTR, "com.apple.quarantine"), 25),
        Ordering::Greater
    );

    // An undecodable key is an error, not an ordering: any guess would
    // steer the descent past the damage and report a record that exists
    // as absent.
    assert!(compare_xattr_key(b"short", 25, &want).is_err());
}

#[test]
fn test_drec_val_parse() {
    // Construct DrecVal bytes: file_id(u64) + date_added(i64) + flags(u16)
    let mut data = Vec::new();
    data.extend_from_slice(&42u64.to_le_bytes()); // file_id = 42
    data.extend_from_slice(&1000i64.to_le_bytes()); // date_added = 1000
    data.extend_from_slice(&DT_DIR.to_le_bytes()); // flags = DT_DIR (4)

    let drec = DrecVal::parse(&data).unwrap();
    assert_eq!(drec.file_id, 42);
    assert_eq!(drec.date_added, 1000);
    assert_eq!(drec.file_type(), DT_DIR);
}

#[test]
fn test_file_extent_val_parse() {
    // Construct FileExtentVal bytes: flags_and_length(u64) + phys_block_num(u64) + crypto_id(u64)
    // length() masks with lower 56 bits (0x00FFFFFFFFFFFFFF)
    let flags_and_length: u64 = 0xAB00_0000_0000_1000; // upper byte = flags 0xAB, lower 56 = 0x1000
    let mut data = Vec::new();
    data.extend_from_slice(&flags_and_length.to_le_bytes());
    data.extend_from_slice(&100u64.to_le_bytes()); // phys_block_num = 100
    data.extend_from_slice(&0u64.to_le_bytes()); // crypto_id = 0

    let extent = FileExtentVal::parse(&data).unwrap();
    assert_eq!(extent.length(), 0x1000);
    assert_eq!(extent.phys_block_num, 100);
    assert_eq!(extent.crypto_id, 0);
}

#[test]
fn test_file_extent_key_logical_address() {
    // j_file_extent_key_t is a j_key_t (one u64 of packed OID and type)
    // followed by the little-endian logical address, so the address
    // starts at byte 8.
    let mut key = Vec::new();
    let obj_id_and_type = (u64::from(J_TYPE_FILE_EXTENT) << 60) | 42;
    key.extend_from_slice(&obj_id_and_type.to_le_bytes());
    key.extend_from_slice(&8192u64.to_le_bytes());

    assert_eq!(parse_file_extent_logical_addr(&key).unwrap(), 8192);
}

#[test]
fn test_file_extent_key_too_short_is_rejected() {
    // A key with the j_key_t but no address must not silently read as 0,
    // which would stack every extent at the start of the file.
    let key = [0u8; 8];
    assert!(matches!(
        parse_file_extent_logical_addr(&key),
        Err(ApfsError::CorruptedData(_))
    ));
}
