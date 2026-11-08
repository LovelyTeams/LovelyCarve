//! Unit tests for the parent module, split out of `testutil.rs` for size.

use super::*;
use crate::{EntryKind, HfsVolume, XattrEntry, XattrKind};
use std::io::Cursor;

fn make_test_image() -> Vec<u8> {
    let mut builder = HfsPlusImageBuilder::new();
    builder
        .add_file("hello.txt", b"Hello, World!\n", 0o644)
        .add_file("test.pkg", b"FAKE_PKG_DATA", 0o644);
    builder.build()
}

#[test]
fn test_synthetic_volume_header() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let vol = HfsVolume::open(cursor).unwrap();
    let hdr = vol.volume_header();

    assert!(hdr.is_hfsx);
    assert_eq!(hdr.signature, 0x4858);
    assert_eq!(hdr.version, 5);
    assert_eq!(hdr.block_size, 4096);
    assert_eq!(hdr.file_count, 2);
    assert_eq!(hdr.folder_count, 1);
}

#[test]
fn test_synthetic_list_root() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let entries = vol.list_directory("/").unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["hello.txt", "test.pkg"]);

    assert_eq!(entries[0].kind, EntryKind::File);
    assert_eq!(entries[0].size, 14);
    assert_eq!(entries[1].kind, EntryKind::File);
    assert_eq!(entries[1].size, 13);
}

#[test]
fn test_synthetic_read_file() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let data = vol.read_file("/hello.txt").unwrap();
    assert_eq!(data, b"Hello, World!\n");

    let data = vol.read_file("/test.pkg").unwrap();
    assert_eq!(data, b"FAKE_PKG_DATA");
}

#[test]
fn test_read_file_rejects_a_fork_that_ends_early() {
    // The fork claims 1 MiB but only one block is allocated, as happens
    // when an extent chain is damaged or its overflow records are lost.
    // Returning the short buffer would be indistinguishable from a
    // complete 1 MiB file that happens to end in zeros.
    let mut builder = HfsPlusImageBuilder::new();
    builder.add_file_with_declared_size("short.bin", b"only this much", 0o644, 1024 * 1024);
    let cursor = Cursor::new(builder.build());
    let mut vol = HfsVolume::open(cursor).unwrap();

    let err = vol.read_file("/short.bin").unwrap_err();
    assert!(
        matches!(err, crate::HfsPlusError::CorruptedData(_)),
        "expected CorruptedData, got {err:?}"
    );
    let message = err.to_string();
    assert!(message.contains("1048576"), "got {message:?}");
    assert!(message.contains("allocation"), "got {message:?}");
}

#[test]
fn streaming_rejects_missing_allocations_before_writing() {
    let image = HfsPlusImageBuilder::new()
        .add_file_with_declared_size("short.bin", b"partial", 0o644, 1024 * 1024)
        .build();
    let mut volume = HfsVolume::open(Cursor::new(image)).unwrap();
    let mut output = Vec::new();
    assert!(volume.read_file_to("/short.bin", &mut output).is_err());
    assert!(output.is_empty());
    assert!(volume.open_file("/short.bin").is_err());
}

#[test]
fn test_synthetic_walk() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let entries = vol.walk().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "/hello.txt");
    assert_eq!(entries[1].path, "/test.pkg");
}

#[test]
fn test_synthetic_stat() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let stat = vol.stat("/hello.txt").unwrap();
    assert_eq!(stat.kind, EntryKind::File);
    assert_eq!(stat.size, 14);
    assert_eq!(stat.permissions.mode, 0o100644);
    assert_eq!(stat.data_fork_extents, 1);
    assert_eq!(stat.resource_fork_size, 0);

    let root_stat = vol.stat("/").unwrap();
    assert_eq!(root_stat.kind, EntryKind::Directory);
}

#[test]
fn test_synthetic_exists() {
    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    assert!(vol.exists("/hello.txt").unwrap());
    assert!(vol.exists("/test.pkg").unwrap());
    assert!(!vol.exists("/nonexistent").unwrap());
}

#[test]
fn test_synthetic_empty_file() {
    let mut builder = HfsPlusImageBuilder::new();
    builder.add_file("empty.txt", b"", 0o644);
    let image = builder.build();

    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let entries = vol.list_directory("/").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "empty.txt");
    assert_eq!(entries[0].size, 0);

    let data = vol.read_file("/empty.txt").unwrap();
    assert!(data.is_empty());
}

#[test]
fn test_synthetic_large_file() {
    // File spanning 2 blocks
    let content = vec![0xAB; 5000];
    let mut builder = HfsPlusImageBuilder::new();
    builder.add_file("large.bin", &content, 0o755);
    let image = builder.build();

    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let data = vol.read_file("/large.bin").unwrap();
    assert_eq!(data.len(), 5000);
    assert_eq!(data, content);

    let stat = vol.stat("/large.bin").unwrap();
    assert_eq!(stat.permissions.mode, 0o100755);
}

// -----------------------------------------------------------------
// decmpfs
//
// These use decmpfs's stored-block form (the `0xff` / `0x06` markers)
// rather than a real codec: what they exercise is the HFS+ plumbing —
// attribute lookup, resource-fork read, block table — while `cmpfs`
// covers the codecs against synthetic streams of its own. It also keeps
// hfsplus free of a compressor dependency.
// -----------------------------------------------------------------

/// A `com.apple.decmpfs` attribute: magic, type, uncompressed size.
fn decmpfs_attr(compression_type: u32, size: u64, payload: &[u8]) -> Vec<u8> {
    let mut attr = 0x636d_7066u32.to_le_bytes().to_vec();
    attr.extend_from_slice(&compression_type.to_le_bytes());
    attr.extend_from_slice(&size.to_le_bytes());
    attr.extend_from_slice(payload);
    attr
}

/// A type 4 resource fork: 256-byte header holding the offset to the
/// resource data, its big-endian length, then the block table.
fn decmpfs_resource_fork(blocks: &[Vec<u8>]) -> Vec<u8> {
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
fn test_decmpfs_inline_is_decompressed() {
    let content = b"contents of a compressed file";
    let mut payload = vec![0xff];
    payload.extend_from_slice(content);

    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "small.txt",
            b"",
            0o644,
            &[(
                "com.apple.decmpfs",
                &decmpfs_attr(3, content.len() as u64, &payload),
            )],
            b"",
        )
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(vol.read_file("/small.txt").unwrap(), content);

    let mut streamed = Vec::new();
    assert_eq!(
        vol.read_file_to("/small.txt", &mut streamed).unwrap(),
        content.len() as u64
    );
    assert_eq!(streamed, content);
}

#[test]
fn test_decmpfs_resource_fork_is_decompressed() {
    // Two blocks, so the block table is actually walked.
    let content: Vec<u8> = (0..70_000).map(|i| (i % 251) as u8).collect();
    let blocks: Vec<Vec<u8>> = content
        .chunks(0x1_0000)
        .map(|chunk| {
            let mut block = vec![0xff];
            block.extend_from_slice(chunk);
            block
        })
        .collect();
    assert_eq!(blocks.len(), 2);

    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "big.bin",
            b"",
            0o644,
            &[(
                "com.apple.decmpfs",
                &decmpfs_attr(4, content.len() as u64, &[]),
            )],
            &decmpfs_resource_fork(&blocks),
        )
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(vol.read_file("/big.bin").unwrap(), content);
}

#[test]
fn test_stat_reports_the_decompressed_size() {
    let content = b"nineteen characters";
    let mut payload = vec![0xff];
    payload.extend_from_slice(content);

    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "c.txt",
            b"",
            0o644,
            &[(
                "com.apple.decmpfs",
                &decmpfs_attr(3, content.len() as u64, &payload),
            )],
            b"",
        )
        .add_file("plain.txt", b"plain", 0o644)
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();

    // The data fork is empty, so an uncompressed read would report 0.
    let stat = vol.stat("/c.txt").unwrap();
    assert_eq!(stat.size, content.len() as u64);
    let compression = stat.compression.expect("file is compressed");
    assert_eq!(compression.compression_type, 3);
    assert_eq!(compression.uncompressed_size, content.len() as u64);

    let stat = vol.stat("/plain.txt").unwrap();
    assert_eq!(stat.size, 5);
    assert!(stat.compression.is_none());
}

#[test]
fn test_list_directory_and_walk_agree_with_stat() {
    // A compressed file's data fork is empty, so a listing that reports the
    // raw fork would say 0 where stat says the decompressed size.
    let content = b"nineteen characters";
    let mut payload = vec![0xff];
    payload.extend_from_slice(content);

    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "c.txt",
            b"",
            0o644,
            &[(
                "com.apple.decmpfs",
                &decmpfs_attr(3, content.len() as u64, &payload),
            )],
            b"",
        )
        .add_file("plain.txt", b"plain", 0o644)
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();

    for entry in vol.list_directory("/").unwrap() {
        let stat = vol.stat(&format!("/{}", entry.name)).unwrap();
        assert_eq!(entry.size, stat.size, "{}", entry.name);
        assert_eq!(entry.kind, stat.kind, "{}", entry.name);
    }
    for walked in vol.walk().unwrap() {
        let stat = vol.stat(&walked.path).unwrap();
        assert_eq!(walked.entry.size, stat.size, "{}", walked.path);
        assert_eq!(walked.entry.kind, stat.kind, "{}", walked.path);
    }
}

#[test]
fn test_compression_attributes_are_classified_apart() {
    let content = b"compressed body";
    let mut payload = vec![0xff];
    payload.extend_from_slice(content);

    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "c.txt",
            b"",
            0o644,
            &[
                (
                    "com.apple.decmpfs",
                    &decmpfs_attr(3, content.len() as u64, &payload),
                ),
                ("com.apple.quarantine", b"0081;deadbeef;Safari;"),
            ],
            b"",
        )
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();
    let attrs = vol.list_xattrs("/c.txt").unwrap();

    // Nothing is hidden: both are still reported, but only one is
    // machinery, and the user attribute is unaffected by the file being
    // compressed.
    assert_eq!(attrs.len(), 2);
    let kind = |name: &str| {
        attrs
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .kind
    };
    assert_eq!(kind("com.apple.decmpfs"), XattrKind::Compression);
    assert_eq!(kind("com.apple.quarantine"), XattrKind::User);
}

#[test]
fn compressed_stream_and_seek_match_full_read() {
    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "c.txt",
            b"",
            0o644,
            &[(
                "com.apple.decmpfs",
                &decmpfs_attr(3, 2, &[0xff, b'h', b'i']),
            )],
            b"",
        )
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();
    use std::io::{Read, Seek, SeekFrom};
    let mut reader = vol.open_file("/c.txt").unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"hi");
    reader.seek(SeekFrom::Start(1)).unwrap();
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, b"i");
    drop(reader);
    assert_eq!(vol.read_file("/c.txt").unwrap(), output);
    let mut streamed = Vec::new();
    assert_eq!(vol.read_file_to("/c.txt", &mut streamed).unwrap(), 2);
    assert_eq!(streamed, output);
}

#[test]
fn test_read_and_list_extended_attributes() {
    let image = HfsPlusImageBuilder::new()
        .add_file_with_xattrs(
            "tagged.txt",
            b"body",
            0o644,
            &[
                ("com.apple.quarantine", b"0081;deadbeef;Safari;"),
                ("com.apple.metadata:kMDItemWhereFroms", b"bplist00"),
            ],
            b"",
        )
        .add_file("bare.txt", b"body", 0o644)
        .build();

    let mut vol = HfsVolume::open(Cursor::new(image)).unwrap();

    assert_eq!(
        vol.get_xattr("/tagged.txt", "com.apple.quarantine")
            .unwrap(),
        Some(b"0081;deadbeef;Safari;".to_vec())
    );
    assert_eq!(
        vol.get_xattr("/tagged.txt", "com.apple.absent").unwrap(),
        None
    );

    // Sorted by the 16-bit binary name comparison, so "metadata:" first.
    // Neither attribute is compression machinery on an uncompressed file.
    assert_eq!(
        vol.list_xattrs("/tagged.txt").unwrap(),
        vec![
            XattrEntry {
                name: "com.apple.metadata:kMDItemWhereFroms".to_string(),
                kind: XattrKind::User,
            },
            XattrEntry {
                name: "com.apple.quarantine".to_string(),
                kind: XattrKind::User,
            },
        ]
    );
    assert!(vol.list_xattrs("/bare.txt").unwrap().is_empty());

    // An uncompressed file is unaffected by any of this.
    assert_eq!(vol.read_file("/tagged.txt").unwrap(), b"body");
}

#[test]
fn test_synthetic_open_file_streaming() {
    use std::io::Read;

    let image = make_test_image();
    let cursor = Cursor::new(image);
    let mut vol = HfsVolume::open(cursor).unwrap();

    let mut reader = vol.open_file("/hello.txt").unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"Hello, World!\n");
}
