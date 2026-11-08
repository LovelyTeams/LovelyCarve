//! Unit tests for the parent module, split out of `lib.rs` for size.

use super::*;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

fn header(compression_type: u32, size: u64) -> Vec<u8> {
    let mut v = MAGIC.to_le_bytes().to_vec();
    v.extend_from_slice(&compression_type.to_le_bytes());
    v.extend_from_slice(&size.to_le_bytes());
    v
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Payload that zlib will not shrink, so a real writer would store it.
fn incompressible(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(37) ^ (i >> 3)) as u8)
        .collect()
}

#[test]
fn classifies_xattrs() {
    // The header attribute is machinery whether or not the caller has
    // established that the file is compressed.
    assert_eq!(classify_xattr(XATTR_NAME, true), XattrKind::Compression);
    assert_eq!(classify_xattr(XATTR_NAME, false), XattrKind::Compression);

    // The resource fork is machinery only on a compressed file; on an
    // uncompressed one it holds user data.
    assert_eq!(
        classify_xattr(RESOURCE_FORK_XATTR_NAME, true),
        XattrKind::Compression
    );
    assert_eq!(
        classify_xattr(RESOURCE_FORK_XATTR_NAME, false),
        XattrKind::User
    );

    for name in ["com.apple.quarantine", "com.apple.FinderInfo", ""] {
        assert_eq!(classify_xattr(name, true), XattrKind::User, "{name}");
        assert_eq!(classify_xattr(name, false), XattrKind::User, "{name}");
    }
}

#[test]
fn rejects_foreign_attribute() {
    let attr = vec![0u8; HEADER_SIZE];
    assert!(matches!(
        Header::parse(&attr),
        Err(CmpfsError::InvalidMagic(0))
    ));
}

#[test]
fn rejects_short_attribute() {
    let attr = header(3, 10);
    assert!(matches!(
        Header::parse(&attr[..12]),
        Err(CmpfsError::Truncated { .. })
    ));
}

#[test]
fn parses_header() {
    let h = Header::parse(&header(4, 1 << 20)).unwrap();
    assert_eq!(h.compression_type, 4);
    assert_eq!(h.uncompressed_size, 1 << 20);
    assert_eq!(h.storage(), Storage::ResourceFork);
}

#[test]
fn classifies_storage() {
    let storage = |t| Header::parse(&header(t, 0)).unwrap().storage();
    for t in [1, 3, 7, 9, 11, 13] {
        assert_eq!(storage(t), Storage::Xattr, "type {t}");
    }
    for t in [4, 8, 10, 12, 14] {
        assert_eq!(storage(t), Storage::ResourceFork, "type {t}");
    }
    assert_eq!(storage(0x8000_0001), Storage::Dataless);
    assert_eq!(storage(0x8000_0002), Storage::Dataless);
    assert_eq!(storage(2), Storage::Unknown);
    assert_eq!(storage(99), Storage::Unknown);
}

#[test]
fn type_1_is_verbatim() {
    let mut attr = header(1, 5);
    attr.extend_from_slice(b"hello");
    assert_eq!(decompress(&attr, None).unwrap(), b"hello");
}

#[test]
fn type_3_inline_zlib() {
    let data = b"the quick brown fox".repeat(20);
    let mut attr = header(3, data.len() as u64);
    attr.extend_from_slice(&zlib(&data));
    assert_eq!(decompress(&attr, None).unwrap(), data);
}

#[test]
fn type_3_inline_stored() {
    let data = incompressible(64);
    let mut attr = header(3, data.len() as u64);
    attr.push(0xff);
    attr.extend_from_slice(&data);
    assert_eq!(decompress(&attr, None).unwrap(), data);
}

#[test]
fn type_7_inline_stored() {
    let data = incompressible(32);
    let mut attr = header(7, data.len() as u64);
    attr.push(0x06);
    attr.extend_from_slice(&data);
    assert_eq!(decompress(&attr, None).unwrap(), data);
}

/// Exercises the compressed LZVN branch, not just the stored marker. No
/// Rust LZVN encoder exists, so the stream is hand-built: opcode `0xe5`
/// emits five literals, `0x06` ends the stream, and the trailing zeros
/// stand in for the padding macOS leaves after every block.
#[test]
fn type_7_inline_lzvn() {
    let mut attr = header(7, 5);
    attr.extend_from_slice(&[
        0xe5, b'h', b'e', b'l', b'l', b'o', 0x06, 0, 0, 0, 0, 0, 0, 0,
    ]);
    assert_eq!(decompress(&attr, None).unwrap(), b"hello");
}

#[test]
fn type_9_requires_its_marker() {
    let mut attr = header(9, 3);
    attr.push(0x00);
    attr.extend_from_slice(b"abc");
    assert!(matches!(
        decompress(&attr, None),
        Err(CmpfsError::CorruptedData(_))
    ));

    let mut attr = header(9, 3);
    attr.push(0xCC);
    attr.extend_from_slice(b"abc");
    assert_eq!(decompress(&attr, None).unwrap(), b"abc");
}

#[test]
fn type_11_inline_lzfse() {
    let data = b"lzfse round trip ".repeat(500);
    let mut encoded = Vec::new();
    lzfse_rust::encode_bytes(&data, &mut encoded).unwrap();
    let mut attr = header(11, data.len() as u64);
    attr.extend_from_slice(&encoded);
    assert_eq!(decompress(&attr, None).unwrap(), data);
}

#[test]
fn declared_size_must_match() {
    let data = b"exactly nineteen ch";
    let mut attr = header(3, data.len() as u64 + 1);
    attr.extend_from_slice(&zlib(data));
    assert!(matches!(
        decompress(&attr, None),
        Err(CmpfsError::CorruptedData(_))
    ));
}

#[test]
fn reports_unsupported_and_dataless_apart() {
    assert!(matches!(
        decompress(&header(13, 0), None),
        Err(CmpfsError::Unsupported(13))
    ));
    assert!(matches!(
        decompress(&header(0x8000_0002, 0), None),
        Err(CmpfsError::Dataless(_))
    ));
}

#[test]
fn resource_fork_must_be_supplied() {
    assert!(matches!(
        decompress(&header(4, 10), None),
        Err(CmpfsError::MissingResourceFork(4))
    ));
}

/// Build a type 4 fork the way libarchive's `hfs_write_resource_fork_header`
/// does: 256-byte header, big-endian resource length, then the block table.
fn zlib_fork(blocks: &[Vec<u8>]) -> Vec<u8> {
    let table_len = 4 + blocks.len() * 8;
    let mut table = (blocks.len() as u32).to_le_bytes().to_vec();
    let mut body = Vec::new();
    for block in blocks {
        table.extend_from_slice(&((table_len + body.len()) as u32).to_le_bytes());
        table.extend_from_slice(&(block.len() as u32).to_le_bytes());
        body.extend_from_slice(block);
    }

    let mut fork = 256u32.to_be_bytes().to_vec();
    fork.resize(256, 0);
    fork.extend_from_slice(&((table_len + body.len()) as u32).to_be_bytes());
    fork.extend_from_slice(&table);
    fork.extend_from_slice(&body);
    fork
}

#[test]
fn type_4_resource_fork_zlib() {
    let data: Vec<u8> = (0..BLOCK_SIZE + 1000).map(|i| (i % 251) as u8).collect();
    let blocks: Vec<Vec<u8>> = data.chunks(BLOCK_SIZE).map(zlib).collect();
    assert_eq!(blocks.len(), 2);

    let attr = header(4, data.len() as u64);
    let fork = zlib_fork(&blocks);
    assert_eq!(decompress(&attr, Some(&fork)).unwrap(), data);
}

#[test]
fn type_4_mixes_stored_and_compressed_blocks() {
    let mut data = vec![0u8; BLOCK_SIZE];
    data.extend_from_slice(&incompressible(500));

    let mut stored = vec![0xff];
    stored.extend_from_slice(&data[BLOCK_SIZE..]);
    let blocks = vec![zlib(&data[..BLOCK_SIZE]), stored];

    let attr = header(4, data.len() as u64);
    assert_eq!(decompress(&attr, Some(&zlib_fork(&blocks))).unwrap(), data);
}

#[test]
fn type_4_block_count_must_match_size() {
    let data = vec![7u8; 100];
    let attr = header(4, (data.len() + BLOCK_SIZE) as u64);
    let fork = zlib_fork(&[zlib(&data)]);
    assert!(matches!(
        decompress(&attr, Some(&fork)),
        Err(CmpfsError::CorruptedData(_))
    ));
}

/// Build a type 8/10/12 fork: a table of little-endian offsets at the
/// start, each block running to the next offset.
fn offset_fork(blocks: &[Vec<u8>]) -> Vec<u8> {
    let table_len = (blocks.len() + 1) * 4;
    let mut table = Vec::new();
    let mut body = Vec::new();
    for block in blocks {
        table.extend_from_slice(&((table_len + body.len()) as u32).to_le_bytes());
        body.extend_from_slice(block);
    }
    table.extend_from_slice(&((table_len + body.len()) as u32).to_le_bytes());
    table.extend_from_slice(&body);
    table
}

#[test]
fn type_8_resource_fork_stored() {
    let data: Vec<u8> = (0..BLOCK_SIZE + 7).map(|i| (i % 97) as u8).collect();
    let blocks: Vec<Vec<u8>> = data
        .chunks(BLOCK_SIZE)
        .map(|c| {
            let mut b = vec![0x06];
            b.extend_from_slice(c);
            b
        })
        .collect();

    let attr = header(8, data.len() as u64);
    assert_eq!(
        decompress(&attr, Some(&offset_fork(&blocks))).unwrap(),
        data
    );
}

#[test]
fn type_10_resource_fork_requires_raw_markers_and_preserves_multiple_blocks() {
    let data = vec![b'R'; BLOCK_SIZE + 17];
    let blocks: Vec<Vec<u8>> = data
        .chunks(BLOCK_SIZE)
        .map(|chunk| {
            let mut block = vec![0xcc];
            block.extend_from_slice(chunk);
            block
        })
        .collect();
    let attribute = header(10, data.len() as u64);
    let mut fork = offset_fork(&blocks);
    assert_eq!(decompress(&attribute, Some(&fork)).unwrap(), data);
    let first = u32::from_le_bytes(fork[..4].try_into().unwrap()) as usize;
    fork[first] = 0;
    assert!(decompress(&attribute, Some(&fork)).is_err());
}

#[test]
fn resource_fork_payload_cannot_point_into_the_offset_table() {
    let mut fork = offset_fork(&[vec![0xcc, b'A']]);
    fork[..4].copy_from_slice(&0u32.to_le_bytes());
    assert!(decompress(&header(10, 1), Some(&fork)).is_err());
}

#[test]
fn type_12_resource_fork_lzfse() {
    let data: Vec<u8> = b"lzfse in a resource fork ".repeat(4000);
    let blocks: Vec<Vec<u8>> = data
        .chunks(BLOCK_SIZE)
        .map(|c| {
            let mut out = Vec::new();
            lzfse_rust::encode_bytes(c, &mut out).unwrap();
            out
        })
        .collect();
    assert!(blocks.len() > 1);

    let attr = header(12, data.len() as u64);
    assert_eq!(
        decompress(&attr, Some(&offset_fork(&blocks))).unwrap(),
        data
    );
}

#[test]
fn lzfse_inline_and_resource_fork_allow_the_stored_marker() {
    let mut attribute = header(11, 5);
    attribute.extend_from_slice(b"\xffhello");
    assert_eq!(decompress(&attribute, None).unwrap(), b"hello");
    let fork = offset_fork(&[b"\xffhello".to_vec()]);
    assert_eq!(decompress(&header(12, 5), Some(&fork)).unwrap(), b"hello");
}

#[test]
fn type_8_resource_fork_decodes_compressed_lzvn() {
    let block = vec![
        0xe5, b'h', b'e', b'l', b'l', b'o', 0x06, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(
        decompress(&header(8, 5), Some(&offset_fork(&[block]))).unwrap(),
        b"hello"
    );
}

#[test]
fn rejects_backwards_offsets() {
    let mut fork = 8u32.to_le_bytes().to_vec();
    fork.extend_from_slice(&4u32.to_le_bytes());
    fork.extend_from_slice(&[0u8; 16]);
    assert!(matches!(
        decompress(&header(8, 10), Some(&fork)),
        Err(CmpfsError::CorruptedData(_))
    ));
}

#[test]
fn rejects_oversized_block() {
    let mut fork = 8u32.to_le_bytes().to_vec();
    fork.extend_from_slice(&((8 + BLOCK_SIZE + 2) as u32).to_le_bytes());
    fork.resize(8 + BLOCK_SIZE + 2, 0);
    assert!(matches!(
        decompress(&header(8, 10), Some(&fork)),
        Err(CmpfsError::CorruptedData(_))
    ));
}

#[test]
fn rejects_block_past_end_of_fork() {
    let fork = zlib_fork(&[vec![0xff, 1, 2, 3]]);
    let attr = header(4, 1000);
    assert!(matches!(
        decompress(&attr, Some(&fork[..fork.len() - 2])),
        Err(CmpfsError::TruncatedResourceFork { .. } | CmpfsError::CorruptedData(_))
    ));
}

#[test]
fn oversized_declared_output_is_rejected_before_allocation() {
    let mut attribute = 0x636d7066u32.to_le_bytes().to_vec();
    attribute.extend_from_slice(&11u32.to_le_bytes());
    attribute.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(decompress(&attribute, None).is_err());
}
