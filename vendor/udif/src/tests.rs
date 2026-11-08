//! Unit tests for the parent module, split out of `lib.rs` for size.

use super::*;
use std::io::Cursor;

#[test]
fn test_block_type_conversion() {
    assert_eq!(
        BlockType::try_from(0x00000000).unwrap(),
        BlockType::ZeroFill
    );
    assert_eq!(BlockType::try_from(0x80000005).unwrap(), BlockType::Zlib);
    assert_eq!(BlockType::try_from(0x80000006).unwrap(), BlockType::Bzip2);
    assert_eq!(BlockType::try_from(0x80000007).unwrap(), BlockType::Lzfse);
    assert_eq!(BlockType::try_from(0x80000008).unwrap(), BlockType::Xz);
    assert_eq!(BlockType::try_from(0xFFFFFFFF).unwrap(), BlockType::End);
    assert_eq!(BlockType::try_from(0x7FFFFFFE).unwrap(), BlockType::Comment);

    // Unknown block type should error
    assert!(BlockType::try_from(0x12345678).is_err());
}

#[test]
fn test_compression_method() {
    assert_eq!(CompressionMethod::default(), CompressionMethod::Zlib);
}

// =========================================================================
// TRICKY PIECE #1: Koly header must be exactly 512 bytes at -512 from EOF
// =========================================================================
#[test]
fn test_koly_header_size_is_512() {
    use crate::format::{KOLY_MAGIC, KOLY_SIZE, KolyHeader};

    assert_eq!(KOLY_SIZE, 512, "KOLY_SIZE constant must be 512");

    // Create a koly header and serialize it
    let koly = KolyHeader {
        magic: *KOLY_MAGIC,
        version: 4,
        header_size: 512,
        flags: 1,
        running_data_fork_offset: 0,
        data_fork_offset: 0,
        data_fork_length: 1000,
        rsrc_fork_offset: 0,
        rsrc_fork_length: 0,
        segment_number: 1,
        segment_count: 1,
        segment_id: [0u8; 16],
        data_checksum_type: 2,
        data_checksum_size: 32,
        data_checksum: [0u8; 128],
        plist_offset: 1000,
        plist_length: 500,
        reserved: [0u8; 64],
        master_checksum_type: 2,
        master_checksum_size: 32,
        master_checksum: [0u8; 128],
        image_variant: 1,
        sector_count: 100,
    };

    let mut buf = Vec::new();
    koly.write(&mut buf).unwrap();

    assert_eq!(
        buf.len(),
        512,
        "Koly header serialization must be exactly 512 bytes"
    );
}

#[test]
fn test_koly_magic_position() {
    use crate::format::{KOLY_MAGIC, KolyHeader};

    // Create a minimal DMG-like structure
    let mut dmg_data = vec![0u8; 1024]; // Some data

    // Append plist placeholder
    let plist = b"<?xml version=\"1.0\"?><plist></plist>";
    let plist_offset = dmg_data.len() as u64;
    dmg_data.extend_from_slice(plist);
    let plist_length = plist.len() as u64;

    // Create and append koly header
    let koly = KolyHeader {
        magic: *KOLY_MAGIC,
        version: 4,
        header_size: 512,
        flags: 1,
        running_data_fork_offset: 0,
        data_fork_offset: 0,
        data_fork_length: plist_offset,
        rsrc_fork_offset: 0,
        rsrc_fork_length: 0,
        segment_number: 1,
        segment_count: 1,
        segment_id: [0u8; 16],
        data_checksum_type: 2,
        data_checksum_size: 32,
        data_checksum: [0u8; 128],
        plist_offset,
        plist_length,
        reserved: [0u8; 64],
        master_checksum_type: 2,
        master_checksum_size: 32,
        master_checksum: [0u8; 128],
        image_variant: 1,
        sector_count: 2,
    };
    koly.write(&mut dmg_data).unwrap();

    // Verify koly magic is at exactly -512 from end
    let total_len = dmg_data.len();
    let koly_start = total_len - 512;
    assert_eq!(&dmg_data[koly_start..koly_start + 4], b"koly");
}

// =========================================================================
// TRICKY PIECE #2: Mish header actual_block_count is at offset 200, not 36
// =========================================================================
#[test]
fn test_mish_block_count_at_offset_200() {
    use crate::format::{MISH_MAGIC, MishHeader};
    use byteorder::{BigEndian, WriteBytesExt};

    // Create a mish header manually with known values
    let mut mish_data = Vec::new();

    // Header (204 bytes)
    mish_data.extend_from_slice(MISH_MAGIC); // 0-3: magic
    mish_data.write_u32::<BigEndian>(1).unwrap(); // 4-7: version
    mish_data.write_u64::<BigEndian>(0).unwrap(); // 8-15: first_sector
    mish_data.write_u64::<BigEndian>(10).unwrap(); // 16-23: sector_count
    mish_data.write_u64::<BigEndian>(0).unwrap(); // 24-31: data_offset
    mish_data.write_u32::<BigEndian>(0).unwrap(); // 32-35: buffers_needed
    mish_data.write_u32::<BigEndian>(999).unwrap(); // 36-39: WRONG block count (should be ignored)
    mish_data.extend_from_slice(&[0u8; 24]); // 40-63: reserved
    mish_data.write_u32::<BigEndian>(2).unwrap(); // 64-67: checksum_type
    mish_data.write_u32::<BigEndian>(32).unwrap(); // 68-71: checksum_size
    mish_data.extend_from_slice(&[0u8; 128]); // 72-199: checksum
    mish_data.write_u32::<BigEndian>(2).unwrap(); // 200-203: ACTUAL block count

    // Add 2 block runs (40 bytes each)
    // Block 0: ZeroFill
    mish_data.write_u32::<BigEndian>(0x00000000).unwrap(); // type
    mish_data.write_u32::<BigEndian>(0).unwrap(); // comment
    mish_data.write_u64::<BigEndian>(0).unwrap(); // sector_number
    mish_data.write_u64::<BigEndian>(10).unwrap(); // sector_count
    mish_data.write_u64::<BigEndian>(0).unwrap(); // compressed_offset
    mish_data.write_u64::<BigEndian>(0).unwrap(); // compressed_length

    // Block 1: End marker
    mish_data.write_u32::<BigEndian>(0xFFFFFFFF).unwrap(); // type
    mish_data.write_u32::<BigEndian>(0).unwrap();
    mish_data.write_u64::<BigEndian>(10).unwrap();
    mish_data.write_u64::<BigEndian>(0).unwrap();
    mish_data.write_u64::<BigEndian>(0).unwrap();
    mish_data.write_u64::<BigEndian>(0).unwrap();

    // Parse
    let mish = MishHeader::from_bytes(&mish_data).unwrap();

    // Should have 2 block runs (from offset 200), not 999 (from offset 36)
    assert_eq!(mish.block_runs.len(), 2);
    assert_eq!(mish.actual_block_count, 2);
    assert_eq!(mish.block_descriptor_count, 999); // This field is at 36 but not used for counting
}

#[test]
fn test_mish_header_size_is_204() {
    use crate::format::{MISH_MAGIC, MishHeader};
    use byteorder::{BigEndian, WriteBytesExt};

    // Build minimal mish header
    let mut mish_data = Vec::new();
    mish_data.extend_from_slice(MISH_MAGIC);
    mish_data.write_u32::<BigEndian>(1).unwrap();
    mish_data.write_u64::<BigEndian>(0).unwrap();
    mish_data.write_u64::<BigEndian>(1).unwrap();
    mish_data.write_u64::<BigEndian>(0).unwrap();
    mish_data.write_u32::<BigEndian>(0).unwrap();
    mish_data.write_u32::<BigEndian>(0).unwrap();
    mish_data.extend_from_slice(&[0u8; 24]);
    mish_data.write_u32::<BigEndian>(2).unwrap();
    mish_data.write_u32::<BigEndian>(32).unwrap();
    mish_data.extend_from_slice(&[0u8; 128]);
    mish_data.write_u32::<BigEndian>(0).unwrap(); // actual_block_count = 0

    // Header should be exactly 204 bytes
    assert_eq!(mish_data.len(), 204);

    // Should parse successfully with 0 block runs
    let mish = MishHeader::from_bytes(&mish_data).unwrap();
    assert_eq!(mish.block_runs.len(), 0);
}

// =========================================================================
// FORMER TRICKY PIECE #3: the C LZFSE decoder needed a destination larger
// than the output, so decode_lzfse_exact decoded into double-sized scratch
// and copied back. lzfse_rust decodes into a Vec that grows as needed, so
// the headroom rule is gone; this pins the behaviour that replaced it.
// =========================================================================
#[test]
fn test_lzfse_decodes_to_exactly_the_original_length() {
    let original = b"Hello, World! This is a test of LZFSE compression. ".repeat(10);

    let mut compressed = Vec::new();
    lzfse_rust::encode_bytes(&original, &mut compressed).unwrap();

    let mut decoded = Vec::new();
    let n = lzfse_rust::decode_bytes(&compressed, &mut decoded).unwrap();

    assert_eq!(n as usize, original.len());
    assert_eq!(decoded, original);
}

// =========================================================================
// TRICKY PIECE #4: Zlib decompressed size might be less than sector_count * 512
// =========================================================================
#[test]
fn test_zlib_partial_sector() {
    use flate2::Compression;
    use flate2::read::ZlibDecoder;
    use flate2::write::ZlibEncoder;
    use std::io::{Read, Write};

    // Create data that's not sector-aligned (100 bytes, not multiple of 512)
    let original = b"This is test data that is not aligned to 512-byte sectors!";

    // Compress it
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(original).unwrap();
    let compressed = encoder.finish().unwrap();

    // Try to decompress into a full sector buffer (512 bytes)
    let mut sector_buf = vec![0u8; 512];
    let mut decoder = ZlibDecoder::new(&compressed[..]);

    // read() should return actual bytes, not error
    let bytes_read = decoder.read(&mut sector_buf).unwrap();

    assert_eq!(bytes_read, original.len());
    assert_eq!(&sector_buf[..bytes_read], &original[..]);

    // Rest of buffer should be zeros
    assert!(sector_buf[bytes_read..].iter().all(|&b| b == 0));
}

// =========================================================================
// TRICKY PIECE #5: DMG roundtrip with various data sizes
// =========================================================================
#[test]
fn test_roundtrip_sector_aligned() {
    // Test with sector-aligned data (1024 bytes = 2 sectors)
    let original = vec![0x42u8; 1024];

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Read back
    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    // Should match (might be padded to sector boundary)
    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

#[test]
fn test_roundtrip_partition_name_needing_xml_escaping() {
    // Names arrive unfiltered from add_partition. Before they were escaped,
    // `&` and `<` produced a plist this reader could not parse.
    for name in ["Foo & Bar", "<disk>", "a > b", "plain"] {
        let original = vec![0x42u8; 1024];

        let mut dmg_buf = Vec::new();
        {
            let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
            writer.add_partition(name, &original).unwrap();
            writer.finish().unwrap();
        }

        let mut reader = DmgReader::new(Cursor::new(&dmg_buf))
            .unwrap_or_else(|e| panic!("partition name {name:?} produced an unreadable DMG: {e}"));
        let extracted = reader.decompress_partition(0).unwrap();
        assert_eq!(&extracted[..original.len()], &original[..]);
    }
}

#[test]
fn test_roundtrip_non_sector_aligned() {
    // Test with non-sector-aligned data (100 bytes)
    let original = b"Short test data that is not sector aligned".to_vec();

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Read back
    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    // First bytes should match original
    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

#[test]
fn test_writer_pads_final_chunk_to_its_declared_sector_count() {
    use std::io::{Read, Seek, SeekFrom};

    // Shorter than a sector, but the block run still declares a whole one.
    // The stored stream has to decode to the full 512 bytes: a reader that
    // trusts the block map would otherwise leave the tail of the sector as
    // fabricated zeroes it cannot distinguish from recovered data.
    let original = b"not sector aligned".to_vec();

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    let reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let data_fork_offset = reader.koly().data_fork_offset;
    let run = reader.partitions()[0]
        .block_map
        .block_runs
        .iter()
        .find(|r| r.block_type == BlockType::Zlib)
        .expect("expected a compressed run")
        .clone();

    let mut cursor = Cursor::new(&dmg_buf);
    cursor
        .seek(SeekFrom::Start(data_fork_offset + run.compressed_offset))
        .unwrap();
    let mut compressed = vec![0u8; run.compressed_length as usize];
    cursor.read_exact(&mut compressed).unwrap();

    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut decoded)
        .unwrap();

    assert_eq!(decoded.len() as u64, run.sector_count * 512);
    assert_eq!(&decoded[..original.len()], &original[..]);
    assert!(decoded[original.len()..].iter().all(|&b| b == 0));
}

#[test]
fn test_roundtrip_empty_data() {
    // Edge case: empty data
    let original: Vec<u8> = vec![];

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("empty", &original).unwrap();
        writer.finish().unwrap();
    }

    let reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let partitions = reader.partitions();
    assert_eq!(partitions.len(), 1);
}

#[test]
fn test_roundtrip_zeros() {
    // Test that zero-filled blocks are handled correctly
    let original = vec![0u8; 2048]; // 4 sectors of zeros

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("zeros", &original).unwrap();
        writer.finish().unwrap();
    }

    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    assert_eq!(extracted.len(), original.len());
    assert!(extracted.iter().all(|&b| b == 0));
}

// =========================================================================
// TRICKY PIECE #6: Block run structure is exactly 40 bytes
// =========================================================================
#[test]
fn test_block_run_size() {
    use crate::format::{BlockRun, BlockType};

    let block_run = BlockRun {
        block_type: BlockType::Zlib,
        comment: 0,
        sector_number: 100,
        sector_count: 50,
        compressed_offset: 1000,
        compressed_length: 500,
    };

    let bytes = block_run.to_bytes();
    assert_eq!(bytes.len(), 40, "Block run must be exactly 40 bytes");

    // Verify round-trip
    let parsed = BlockRun::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.block_type, BlockType::Zlib);
    assert_eq!(parsed.sector_number, 100);
    assert_eq!(parsed.sector_count, 50);
    assert_eq!(parsed.compressed_offset, 1000);
    assert_eq!(parsed.compressed_length, 500);
}

// =========================================================================
// TRICKY PIECE #7: is_dmg checks magic at -512, not -4
// =========================================================================
#[test]
fn test_is_dmg_checks_correct_offset() {
    use crate::format::is_dmg;

    // Create a file that has "koly" at -4 but not at -512 (should NOT be valid)
    let mut fake_dmg = vec![0u8; 600];
    // Put "koly" at the wrong place (-4 from end)
    let len = fake_dmg.len();
    fake_dmg[len - 4..].copy_from_slice(b"koly");

    let mut cursor = Cursor::new(&fake_dmg);
    assert!(
        !is_dmg(&mut cursor),
        "Should not detect koly at wrong offset"
    );

    // Create a file that has "koly" at -512 (should be valid)
    let mut real_dmg = vec![0u8; 600];
    let len = real_dmg.len();
    real_dmg[len - 512..len - 508].copy_from_slice(b"koly");

    let mut cursor = Cursor::new(&real_dmg);
    assert!(is_dmg(&mut cursor), "Should detect koly at correct offset");
}

// =========================================================================
// TRICKY PIECE #8: Different compression methods
// =========================================================================
#[test]
fn test_compression_methods() {
    let original = b"Test data for compression testing. ".repeat(100);

    for method in [
        CompressionMethod::Raw,
        CompressionMethod::Zlib,
        CompressionMethod::Bzip2,
        // LZFSE tested separately due to buffer quirks
    ] {
        let mut dmg_buf = Vec::new();
        {
            let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf)).compression(method);
            writer.add_partition("test", &original).unwrap();
            writer.finish().unwrap();
        }

        let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
        let extracted = reader.decompress_partition(0).unwrap();

        assert!(
            extracted.len() >= original.len(),
            "Extracted should be at least as large as original for {:?}",
            method
        );
        assert_eq!(
            &extracted[..original.len()],
            &original[..],
            "Data mismatch for {:?}",
            method
        );
    }
}

#[test]
fn test_lzfse_compression_roundtrip() {
    let original = b"LZFSE compression test data. ".repeat(100);

    let mut dmg_buf = Vec::new();
    {
        let mut writer =
            DmgWriter::new(Cursor::new(&mut dmg_buf)).compression(CompressionMethod::Lzfse);
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

#[test]
fn test_xz_roundtrip() {
    use lzma_rust2::{XzOptions, XzReader, XzWriter};
    use std::io::Write;

    let original = b"XZ compression roundtrip test data. ".repeat(100);

    // Compress
    let mut encoder = XzWriter::new(Vec::new(), XzOptions::with_preset(6)).unwrap();
    encoder.write_all(&original).unwrap();
    let compressed = encoder.finish().unwrap();

    // Verify XZ magic bytes
    assert_eq!(&compressed[..6], &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);

    // Decompress
    let mut decoder = XzReader::new(&compressed[..], false);
    let mut decompressed = vec![0u8; original.len()];
    let n = crate::reader::read_full(&mut decoder, &mut decompressed).unwrap();

    assert_eq!(n, original.len());
    assert_eq!(&decompressed[..], &original[..]);
}

// =========================================================================
// Integration test with real DMG file (requires fixture)
// =========================================================================

// =========================================================================
// TRICKY PIECE #9: Checksum verification
// =========================================================================
#[test]
fn test_checksum_verification_disabled() {
    // Create a DMG and verify we can read it with checksums disabled
    let original = b"Test data for checksum verification".repeat(20);

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Read with checksums disabled
    let options = reader::DmgReaderOptions {
        verify_checksums: false,
    };
    let mut reader = DmgReader::with_options(Cursor::new(&dmg_buf), options).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

#[test]
fn test_checksum_verification_enabled_with_zero_checksums() {
    // Create a DMG (writer currently writes zero checksums)
    // This should still work because zero checksums are skipped
    let original = b"Test data for checksum verification".repeat(20);

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Read with checksums enabled (default)
    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

#[test]
fn test_dmg_archive_with_options() {
    // Test that DmgArchive::open_with_options works
    let original = b"Test data".repeat(10);

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Write to temp file
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path().join("test.dmg");
    std::fs::write(&temp_path, &dmg_buf).unwrap();

    // Open with custom options
    let options = reader::DmgReaderOptions {
        verify_checksums: false,
    };
    let mut archive = DmgArchive::open_with_options(&temp_path, options).unwrap();
    let extracted = archive.extract_partition(0).unwrap();

    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);
}

// =========================================================================
// TRICKY PIECE #10: Checksum roundtrip verification
// =========================================================================
#[test]
fn test_checksum_roundtrip_with_verification() {
    // Test that checksums are written and verified correctly
    let original = b"Test data for checksum roundtrip verification. ".repeat(50);

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Read with checksum verification enabled (default)
    // This will fail if checksums don't match
    let mut reader = DmgReader::new(Cursor::new(&dmg_buf)).unwrap();
    let extracted = reader.decompress_partition(0).unwrap();

    assert!(extracted.len() >= original.len());
    assert_eq!(&extracted[..original.len()], &original[..]);

    // Verify checksums are non-zero in koly header
    let koly = reader.koly();
    assert_eq!(koly.data_checksum_type, 2); // CRC32
    assert_ne!(&koly.data_checksum[..4], &[0u8; 4]); // Non-zero checksum
    assert_eq!(koly.master_checksum_type, 2); // CRC32
    assert_ne!(&koly.master_checksum[..4], &[0u8; 4]); // Non-zero checksum
}

#[test]
fn test_checksum_detection_corrupted_data() {
    // Test that corrupted data fork is detected
    let original = b"Test data for corruption detection".repeat(20);

    let mut dmg_buf = Vec::new();
    {
        let mut writer = DmgWriter::new(Cursor::new(&mut dmg_buf));
        writer.add_partition("test", &original).unwrap();
        writer.finish().unwrap();
    }

    // Corrupt the data fork (first 100 bytes)
    for byte in dmg_buf.iter_mut().take(100) {
        *byte ^= 0xFF;
    }

    // Try to read with checksum verification - should fail
    let result = DmgReader::new(Cursor::new(&dmg_buf));
    assert!(result.is_err());
    if let Err(DppError::ChecksumMismatch { expected, actual }) = result {
        assert_ne!(expected, actual); // Checksums should differ
    } else {
        panic!("Expected ChecksumMismatch error");
    }
}

#[test]
fn test_checksum_all_compression_methods() {
    // Test checksum verification with all compression methods
    let original = b"Testing checksums with all compressions! ".repeat(100);

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

        // Read with checksum verification
        let mut reader = DmgReader::new(Cursor::new(&dmg_buf))
            .unwrap_or_else(|e| panic!("Failed to open DMG with {:?}: {:?}", method, e));

        let extracted = reader
            .decompress_partition(0)
            .unwrap_or_else(|e| panic!("Failed to decompress with {:?}: {:?}", method, e));

        assert!(
            extracted.len() >= original.len(),
            "Extracted size mismatch for {:?}",
            method
        );
        assert_eq!(
            &extracted[..original.len()],
            &original[..],
            "Data mismatch for {:?}",
            method
        );
    }
}
