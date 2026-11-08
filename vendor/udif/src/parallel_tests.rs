//! Unit tests for the parent module, split out of `lib.rs` for size.

use super::*;
use std::io::Cursor;

#[test]
fn test_parallel_matches_sequential() {
    let original = b"Test data for parallel decompression. ".repeat(100);

    for method in [
        CompressionMethod::Raw,
        CompressionMethod::Zlib,
        CompressionMethod::Bzip2,
        CompressionMethod::Lzfse,
    ] {
        let mut dmg_buf = Vec::new();
        {
            let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf)).compression(method);
            writer.add_partition("test", &original).unwrap();
            writer.finish().unwrap();
        }

        // Sequential decompress
        let mut reader1 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
        let sequential = reader1.decompress_partition(0).unwrap();

        // Parallel decompress
        let mut reader2 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
        let parallel = reader2.decompress_partition_parallel(0).unwrap();

        assert_eq!(sequential, parallel, "Parallel mismatch for {:?}", method);
    }
}

#[test]
fn test_parallel_to_writer() {
    let original = b"Test data for parallel writer. ".repeat(100);

    let mut dmg_buf = Vec::new();
    {
        let mut writer =
            DmgWriter::new(Cursor::new(&mut dmg_buf)).compression(CompressionMethod::Zlib);
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Sequential decompress
    let mut reader1 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let sequential = reader1.decompress_partition(0).unwrap();

    // Parallel via decompress_partition_to_parallel
    let mut reader2 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let mut output = Vec::new();
    reader2
        .decompress_partition_to_parallel(0, &mut output)
        .unwrap();

    assert_eq!(sequential, output);
}

#[test]
fn test_parallel_empty_partition() {
    let original: Vec<u8> = vec![];

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("empty", &original).unwrap();
        writer.finish().unwrap();
    }

    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let result = reader.decompress_partition_parallel(0).unwrap();
    assert!(result.iter().all(|&b| b == 0));
}

#[test]
fn test_parallel_zeros() {
    let original = vec![0u8; 2048]; // 4 sectors of zeros

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("zeros", &original).unwrap();
        writer.finish().unwrap();
    }

    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let result = reader.decompress_partition_parallel(0).unwrap();

    assert_eq!(result.len(), original.len());
    assert!(result.iter().all(|&b| b == 0));
}

#[test]
fn test_auto_selects_parallel() {
    let original = b"Auto-select test data. ".repeat(50);

    let mut dmg_buf = Vec::new();
    {
        let mut writer =
            DmgWriter::new(Cursor::new(&mut dmg_buf)).compression(CompressionMethod::Zlib);
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // decompress_partition_auto should use parallel when feature is enabled
    let mut reader1 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let auto_result = reader1.decompress_partition_auto(0).unwrap();

    let mut reader2 = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let parallel_result = reader2.decompress_partition_parallel(0).unwrap();

    assert_eq!(auto_result, parallel_result);
}
