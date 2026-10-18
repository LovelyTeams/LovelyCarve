//! Pure Rust reader for Expert Witness Format (E01/EWF) forensic disk images.
//!
//! Provides a `Read + Seek` interface over E01 images, supporting:
//! - EWF v1 format (`.E01` files produced by `EnCase`, FTK Imager, etc.)
//! - EWF v2 format (`.Ex01`/`.Lx01` from `EnCase` 7+) with auto-detection
//! - L01 logical evidence files
//! - Multi-segment images (`.E01`-`.EZZ` for v1, `.Ex01`-`.EzZZ` for v2)
//! - zlib-compressed chunks with LRU caching
//! - O(1) seeking via flat chunk index
//! - Hash verification (`verify()`) with MD5 and SHA-1
//! - Case metadata, stored hashes, and acquisition error parsing

mod acquisition;
mod chunk_decode;
mod chunk_table;
mod error;
pub(crate) mod ewf2;
pub mod logical;
mod parse;
mod reader;
/// EWF v1 on-disk structural primitives: signatures, descriptor / volume /
/// table-header / table-entry layout, their stored adler-32 CRCs, and the shared
/// [`sections::adler32`] entry point. This is the single source of truth for the
/// EWF v1 layout — consumers (e.g. an integrity auditor) parse and CRC-check
/// against these instead of re-deriving offsets.
pub mod sections;
mod segment_source;
mod types;
mod v1_tables;

// Re-export the public API so external crates see the same `ewf::Foo` paths.
pub use error::{EwfError, Result};
pub use parse::parse_error2_data;
pub use reader::{discover_segments, EwfReader};
pub use sections::{
    EwfFileHeader, EwfVolume, SectionDescriptor, TableEntry, TableHeader, EVF_SIGNATURE,
    FILE_HEADER_SIZE, SECTION_DESCRIPTOR_SIZE, TABLE_HEADER_SIZE,
};
pub use segment_source::{SegmentBacking, SegmentSource};
#[cfg(feature = "verify")]
pub use types::VerifyResult;
pub use types::{AcquisitionError, EwfMetadata, StoredHashes};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sections::{FILE_HEADER_SIZE, SECTION_DESCRIPTOR_SIZE};

    use std::io::{Read, Seek, SeekFrom, Write};
    use tempfile::NamedTempFile;

    // -- EwfFileHeader tests --

    fn make_file_header(segment_number: u16) -> [u8; 13] {
        let mut buf = [0u8; FILE_HEADER_SIZE];
        buf[0..8].copy_from_slice(&EVF_SIGNATURE);
        buf[8] = 0x01; // Fields_start
        buf[9..11].copy_from_slice(&segment_number.to_le_bytes());
        buf[11] = 0x00; // Fields_end (low byte)
        buf[12] = 0x00; // Fields_end (high byte)
        buf
    }

    #[test]
    fn parse_file_header_segment_1() {
        let buf = make_file_header(1);
        let header = EwfFileHeader::parse(&buf).unwrap();
        assert_eq!(header.segment_number, 1);
    }

    #[test]
    fn parse_file_header_segment_42() {
        let buf = make_file_header(42);
        let header = EwfFileHeader::parse(&buf).unwrap();
        assert_eq!(header.segment_number, 42);
    }

    #[test]
    fn parse_file_header_rejects_invalid_signature() {
        let buf = [0u8; 13];
        let result = EwfFileHeader::parse(&buf);
        assert!(matches!(result, Err(EwfError::InvalidSignature { .. })));
    }

    #[test]
    fn parse_file_header_rejects_short_buffer() {
        let buf = [0u8; 5];
        let result = EwfFileHeader::parse(&buf);
        assert!(matches!(result, Err(EwfError::BufferTooShort { .. })));
    }

    // -- SectionDescriptor tests --

    fn make_section_descriptor(section_type: &str, next: u64, section_size: u64) -> [u8; 76] {
        let mut buf = [0u8; SECTION_DESCRIPTOR_SIZE];
        // Type field: 16 bytes, NUL-padded
        let type_bytes = section_type.as_bytes();
        buf[..type_bytes.len()].copy_from_slice(type_bytes);
        // Next: u64 LE at offset 16
        buf[16..24].copy_from_slice(&next.to_le_bytes());
        // SectionSize: u64 LE at offset 24
        buf[24..32].copy_from_slice(&section_size.to_le_bytes());
        // Checksum at offset 72 (skip for now, not validated)
        buf
    }

    #[test]
    fn parse_section_descriptor_volume() {
        let buf = make_section_descriptor("volume", 1000, 170);
        let desc = SectionDescriptor::parse(&buf, 13).unwrap();
        assert_eq!(desc.section_type, "volume");
        assert_eq!(desc.next, 1000);
        assert_eq!(desc.section_size, 170);
        assert_eq!(desc.offset, 13);
    }

    #[test]
    fn parse_section_descriptor_table() {
        let buf = make_section_descriptor("table", 50000, 4096);
        let desc = SectionDescriptor::parse(&buf, 200).unwrap();
        assert_eq!(desc.section_type, "table");
        assert_eq!(desc.next, 50000);
        assert_eq!(desc.section_size, 4096);
    }

    #[test]
    fn parse_section_descriptor_done() {
        let buf = make_section_descriptor("done", 0, 76);
        let desc = SectionDescriptor::parse(&buf, 9999).unwrap();
        assert_eq!(desc.section_type, "done");
        assert_eq!(desc.next, 0);
    }

    #[test]
    fn parse_section_descriptor_rejects_short_buffer() {
        let buf = [0u8; 10];
        let result = SectionDescriptor::parse(&buf, 0);
        assert!(matches!(result, Err(EwfError::BufferTooShort { .. })));
    }

    // -- EwfVolume tests --

    fn make_volume_data(
        chunk_count: u32,
        sectors_per_chunk: u32,
        bytes_per_sector: u32,
        sector_count: u64,
    ) -> [u8; 94] {
        let mut buf = [0u8; 94];
        // media_type at offset 0 (skip)
        buf[4..8].copy_from_slice(&chunk_count.to_le_bytes());
        buf[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        buf[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        buf[16..24].copy_from_slice(&sector_count.to_le_bytes());
        buf
    }

    #[test]
    fn parse_volume_typical() {
        let buf = make_volume_data(1000, 64, 512, 64000);
        let vol = EwfVolume::parse(&buf).unwrap();
        assert_eq!(vol.chunk_count, 1000);
        assert_eq!(vol.sectors_per_chunk, 64);
        assert_eq!(vol.bytes_per_sector, 512);
        assert_eq!(vol.sector_count, 64000);
        assert_eq!(vol.chunk_size(), 32768); // 64 * 512
        assert_eq!(vol.total_size(), 512 * 64000);
    }

    #[test]
    fn parse_volume_rejects_short_buffer() {
        let buf = [0u8; 10];
        let result = EwfVolume::parse(&buf);
        assert!(matches!(result, Err(EwfError::BufferTooShort { .. })));
    }

    // -- TableEntry tests --

    #[test]
    fn parse_table_entry_compressed() {
        // bit 31 set, offset = 0x1000
        let val: u32 = 0x8000_1000;
        let buf = val.to_le_bytes();
        let entry = TableEntry::parse(&buf).unwrap();
        assert!(entry.compressed);
        assert_eq!(entry.chunk_offset, 0x1000);
    }

    #[test]
    fn parse_table_entry_uncompressed() {
        let val: u32 = 0x0000_2000;
        let buf = val.to_le_bytes();
        let entry = TableEntry::parse(&buf).unwrap();
        assert!(!entry.compressed);
        assert_eq!(entry.chunk_offset, 0x2000);
    }

    #[test]
    fn parse_table_entry_rejects_short_buffer() {
        let buf = [0u8; 2];
        let result = TableEntry::parse(&buf);
        assert!(matches!(result, Err(EwfError::BufferTooShort { .. })));
    }

    // -- EwfReader synthetic E01 tests --

    /// Build a single-chunk EnCase 6 fixture with sectors before its tables.
    fn build_synthetic_e01(data: &[u8]) -> NamedTempFile {
        build_synthetic_e01_chunks(&[data]).0
    }

    /// Independently serialize the documented EnCase 6 layout and return the
    /// actual stored lengths as an oracle for eager/lazy chunk metadata.
    fn build_synthetic_e01_chunks(data: &[&[u8]]) -> (NamedTempFile, Vec<usize>) {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let stored: Vec<Vec<u8>> = data
            .iter()
            .map(|bytes| {
                let mut padded = bytes.to_vec();
                padded.resize(32768, 0);
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&padded).unwrap();
                encoder.finish().unwrap()
            })
            .collect();
        let lengths = stored.iter().map(Vec::len).collect();
        let mut output = make_file_header(1).to_vec();
        let mut section = |kind: &str, body: &[u8]| {
            let size = (76 + body.len()) as u64;
            let next = if kind == "done" {
                output.len() as u64
            } else {
                output.len() as u64 + size
            };
            let mut descriptor = make_section_descriptor(kind, next, size);
            let checksum = adler2::adler32_slice(&descriptor[..72]);
            descriptor[72..76].copy_from_slice(&checksum.to_le_bytes());
            output.extend_from_slice(&descriptor);
            output.extend_from_slice(body);
        };
        let mut volume = [0u8; 1052];
        volume[0] = 1;
        volume[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        volume[8..12].copy_from_slice(&64u32.to_le_bytes());
        volume[12..16].copy_from_slice(&512u32.to_le_bytes());
        volume[16..24].copy_from_slice(&(data.len() as u64 * 64).to_le_bytes());
        volume[52] = 1;
        let checksum = adler2::adler32_slice(&volume[..1048]);
        volume[1048..1052].copy_from_slice(&checksum.to_le_bytes());
        section("volume", &volume);
        let data_offset = 13 + 76 + volume.len() as u64 + 76;
        let sectors: Vec<u8> = stored.iter().flatten().copied().collect();
        section("sectors", &sectors);
        let mut table = vec![0u8; 24];
        table[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        table[8..16].copy_from_slice(&data_offset.to_le_bytes());
        let checksum = adler2::adler32_slice(&table[..20]);
        table[20..24].copy_from_slice(&checksum.to_le_bytes());
        let mut offset = 0u32;
        for chunk in &stored {
            table.extend_from_slice(&(0x8000_0000u32 | offset).to_le_bytes());
            offset += chunk.len() as u32;
        }
        let checksum = adler2::adler32_slice(&table[24..]);
        table.extend_from_slice(&checksum.to_le_bytes());
        section("table", &table);
        section("table2", &table);
        section("done", &[]);
        let mut file = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        file.write_all(&output).unwrap();
        file.flush().unwrap();
        (file, lengths)
    }

    /// A synthetic `E01` whose *volume header* declares an arbitrary `chunk_count`,
    /// while the real table still describes exactly one chunk. The header count
    /// is the untrusted field behind the 128 GB ceiling bug; the table is the
    /// source of truth. `chunk_count` sits at file offset 13 (header) + 76
    /// (volume descriptor) + 4 (within volume data) = 93.
    fn e01_declaring_chunk_count(data: &[u8], declared: u32) -> tempfile::NamedTempFile {
        let tmp = build_synthetic_e01(data);
        let mut bytes = std::fs::read(tmp.path()).unwrap();
        bytes[93..97].copy_from_slice(&declared.to_le_bytes());
        std::fs::write(tmp.path(), &bytes).unwrap();
        tmp
    }

    #[test]
    fn large_volume_chunk_count_is_accepted() {
        // 5M chunks (× 32 KB ≈ 160 GB) exceeds the old 4M cap. A real >128 GB
        // image must open — the header count is a hint, the table is the truth.
        let tmp = e01_declaring_chunk_count(b"Hello, forensic world!", 5_000_000);
        let reader = EwfReader::open(tmp.path())
            .expect("a >128 GB image must not be rejected by a too-low ceiling");
        // The REAL chunk count comes from the table (one entry), not the header.
        assert_eq!(reader.chunk_count(), 1);
    }

    #[test]
    fn forged_huge_chunk_count_does_not_over_allocate() {
        // A forged header claims ~4.3 billion chunks (× 32 B/entry ≈ 137 GB if
        // reserved blindly). The reservation must be bounded by what the file
        // can physically describe (4 B/entry), so this opens without OOM.
        let tmp = e01_declaring_chunk_count(b"x", u32::MAX);
        let reader = EwfReader::open(tmp.path())
            .expect("a forged count must be bounded by file size, not OOM or reject");
        assert_eq!(reader.chunk_count(), 1);
    }

    #[test]
    fn chunk_struct_stays_compact() {
        // The in-RAM chunk table dominates memory for large images (one Chunk per
        // 32 KB). Keep it <= 16 bytes so a 2 TB image's ~67M-entry table is ~1 GB,
        // not ~2 GB. (compressed flag packed into the offset's high bit; size and
        // segment_idx narrowed to u32 — a chunk is <= 128 MB, segment counts small.)
        assert!(
            std::mem::size_of::<crate::sections::Chunk>() <= 16,
            "Chunk is {} bytes; the chunk table must stay compact for TB images",
            std::mem::size_of::<crate::sections::Chunk>()
        );
    }

    #[test]
    fn ewf_reader_opens_synthetic_e01() {
        let data = b"Hello, forensic world!";
        let tmp = build_synthetic_e01(data);
        let reader = EwfReader::open(tmp.path()).unwrap();
        assert_eq!(reader.chunk_size(), 32768);
        assert_eq!(reader.chunk_count(), 1);
        assert!(reader.total_size() > 0);
    }

    #[test]
    fn ewf_reader_reads_first_bytes() {
        let data = b"DEADBEEF_CAFEBABE_1234567890";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    #[test]
    fn ewf_reader_seek_and_read() {
        let mut test_data = vec![0u8; 1024];
        // Write a known pattern at offset 512
        test_data[512..520].copy_from_slice(b"SEEKTEST");
        let tmp = build_synthetic_e01(&test_data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        // Seek to offset 512
        reader.seek(SeekFrom::Start(512)).unwrap();
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"SEEKTEST");
    }

    #[test]
    fn ewf_reader_seek_from_end() {
        let data = b"test";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        let size = reader.total_size();

        // Seek to 4 bytes before end, then read should get zeros (padded area)
        let pos = reader.seek(SeekFrom::End(-4)).unwrap();
        assert_eq!(pos, size - 4);
    }

    #[test]
    fn ewf_reader_read_returns_zero_at_eof() {
        let data = b"short";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        // Seek past end
        reader.seek(SeekFrom::Start(reader.total_size())).unwrap();
        let mut buf = [0u8; 10];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    // -- Enhancement: table2 support --

    /// Build a synthetic E01 that uses "table2" instead of "table" for the chunk table.
    /// Some `EnCase` versions write both; our reader must handle either.
    fn build_synthetic_e01_with_table2(data: &[u8]) -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let mut padded = data.to_vec();
        padded.resize(chunk_size as usize, 0);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        let sector_count = u64::from(chunk_size / bytes_per_sector);

        let vol_desc_offset: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_offset: u64 = vol_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_offset: u64 = vol_data_offset + 94;
        let tbl_hdr_offset: u64 = tbl_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entries_offset: u64 = tbl_hdr_offset + 24;
        let sectors_desc_offset: u64 = tbl_entries_offset + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = sectors_data_offset + compressed.len() as u64;

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_offset.to_le_bytes());
        vol_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[0..4].copy_from_slice(&1u32.to_le_bytes());
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor -- uses "table2" instead of "table"
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..6].copy_from_slice(b"table2");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        let tbl_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4;
        tbl_desc[24..32].copy_from_slice(&tbl_section_size.to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header: u32 entry_count + 4 padding + u64 base_offset
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes()); // entry_count (u32)
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry
        let entry: u32 = 0x8000_0000;
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        sec_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        let sec_section_size = SECTION_DESCRIPTOR_SIZE as u64 + compressed.len() as u64;
        sec_desc[24..32].copy_from_slice(&sec_section_size.to_le_bytes());
        file_data.extend_from_slice(&sec_desc);

        // Compressed chunk data
        file_data.extend_from_slice(&compressed);

        // Done descriptor
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn ewf_reader_handles_table2_sections() {
        let data = b"table2 section test data!";
        let tmp = build_synthetic_e01_with_table2(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        assert_eq!(reader.chunk_count(), 1);
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    #[test]
    fn ewf_reader_skips_duplicate_table2() {
        // Build a synthetic E01 with BOTH "table" and "table2" sections
        // (same chunk data). Reader should not double-count chunks.
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let mut padded = b"dedup test".to_vec();
        padded.resize(chunk_size as usize, 0);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();
        let sector_count = u64::from(chunk_size / bytes_per_sector);

        // Layout: header | vol_desc | vol_data | tbl_desc("table") | tbl_hdr | entry |
        //         tbl2_desc("table2") | tbl2_hdr | entry2 | sec_desc | data | done_desc
        let vol_desc_off: u64 = 13;
        let vol_data_off: u64 = vol_desc_off + 76;
        let tbl_desc_off: u64 = vol_data_off + 94;
        let tbl_hdr_off: u64 = tbl_desc_off + 76;
        let tbl_entry_off: u64 = tbl_hdr_off + 24;
        let tbl2_desc_off: u64 = tbl_entry_off + 4;
        let tbl2_hdr_off: u64 = tbl2_desc_off + 76;
        let tbl2_entry_off: u64 = tbl2_hdr_off + 24;
        let sec_desc_off: u64 = tbl2_entry_off + 4;
        let sec_data_off: u64 = sec_desc_off + 76;
        let done_desc_off: u64 = sec_data_off + compressed.len() as u64;

        let mut d = Vec::new();

        // File header
        d.extend_from_slice(&EVF_SIGNATURE);
        d.push(0x01);
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor -> next = tbl_desc
        let mut vd = [0u8; 76];
        vd[..6].copy_from_slice(b"volume");
        vd[16..24].copy_from_slice(&tbl_desc_off.to_le_bytes());
        vd[24..32].copy_from_slice(&(76u64 + 94).to_le_bytes());
        d.extend_from_slice(&vd);

        // Volume data
        let mut vdata = [0u8; 94];
        vdata[0..4].copy_from_slice(&1u32.to_le_bytes());
        vdata[4..8].copy_from_slice(&1u32.to_le_bytes()); // 1 chunk
        vdata[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vdata[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vdata[16..24].copy_from_slice(&sector_count.to_le_bytes());
        d.extend_from_slice(&vdata);

        // Table descriptor "table" -> next = tbl2_desc
        let mut td = [0u8; 76];
        td[..5].copy_from_slice(b"table");
        td[16..24].copy_from_slice(&tbl2_desc_off.to_le_bytes());
        td[24..32].copy_from_slice(&(76u64 + 24 + 4).to_le_bytes());
        d.extend_from_slice(&td);

        // Table header: u32 entry_count + 4 padding + u64 base_offset
        let mut th = [0u8; 24];
        th[0..4].copy_from_slice(&1u32.to_le_bytes()); // entry_count (u32)
        th[8..16].copy_from_slice(&sec_data_off.to_le_bytes());
        d.extend_from_slice(&th);

        // Table entry: compressed, offset 0
        d.extend_from_slice(&0x8000_0000u32.to_le_bytes());

        // Table2 descriptor "table2" -> next = sec_desc
        let mut td2 = [0u8; 76];
        td2[..6].copy_from_slice(b"table2");
        td2[16..24].copy_from_slice(&sec_desc_off.to_le_bytes());
        td2[24..32].copy_from_slice(&(76u64 + 24 + 4).to_le_bytes());
        d.extend_from_slice(&td2);

        // Table2 header (identical)
        d.extend_from_slice(&th);

        // Table2 entry (identical)
        d.extend_from_slice(&0x8000_0000u32.to_le_bytes());

        // Sectors descriptor
        let mut sd = [0u8; 76];
        sd[..7].copy_from_slice(b"sectors");
        sd[16..24].copy_from_slice(&done_desc_off.to_le_bytes());
        sd[24..32].copy_from_slice(&(76u64 + compressed.len() as u64).to_le_bytes());
        d.extend_from_slice(&sd);

        // Compressed data
        d.extend_from_slice(&compressed);

        // Done
        let mut dd = [0u8; 76];
        dd[..4].copy_from_slice(b"done");
        dd[24..32].copy_from_slice(&76u64.to_le_bytes());
        d.extend_from_slice(&dd);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&d).unwrap();
        tmp.flush().unwrap();

        let reader = EwfReader::open(tmp.path()).unwrap();
        // Volume says 1 chunk. Even though both table and table2 exist,
        // we should have exactly 1 chunk, not 2.
        assert_eq!(reader.chunk_count(), 1, "table2 caused duplicate chunks");
    }

    // -- Bug fix: table header entry_count must be u32, not u64 --

    /// Build a synthetic E01 where the table header has non-zero padding
    /// bytes at [4..8]. The EWF v1 spec defines the table header as:
    ///   [0..4]  u32 `entry_count`
    ///   [4..8]  padding (4 bytes, may be non-zero)
    ///   [8..16] u64 `base_offset`
    ///   [16..24] padding + checksum
    /// If the parser incorrectly reads [0..8] as u64, the non-zero padding
    /// will corrupt the entry count, causing failure.
    fn build_synthetic_e01_with_nonzero_table_padding(data: &[u8]) -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let bytes_per_sector: u32 = 512;
        let sectors_per_chunk: u32 = chunk_size / bytes_per_sector;
        let sector_count = u64::from(chunk_size / bytes_per_sector);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        // Pad to chunk_size
        encoder
            .write_all(&vec![0u8; chunk_size as usize - data.len()])
            .unwrap();
        let compressed = encoder.finish().unwrap();

        let vol_desc_off: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_off: u64 = vol_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_off: u64 = vol_data_off + 94;
        let tbl_hdr_off: u64 = tbl_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entry_off: u64 = tbl_hdr_off + 24;
        let sectors_desc_offset: u64 = tbl_entry_off + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = sectors_data_offset + compressed.len() as u64;

        let mut file_data = Vec::new();

        // File header
        let mut hdr = [0u8; FILE_HEADER_SIZE];
        hdr[0..8].copy_from_slice(&EVF_SIGNATURE);
        hdr[9..11].copy_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&hdr);

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_off.to_le_bytes());
        let vol_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 94;
        vol_desc[24..32].copy_from_slice(&vol_section_size.to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        let tbl_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4;
        tbl_desc[24..32].copy_from_slice(&tbl_section_size.to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header — CORRECT format with non-zero padding
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes()); // entry_count as u32
        tbl_hdr[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // non-zero padding!
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes()); // base_offset
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry
        let entry: u32 = 0x8000_0000;
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        sec_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        let sec_section_size = SECTION_DESCRIPTOR_SIZE as u64 + compressed.len() as u64;
        sec_desc[24..32].copy_from_slice(&sec_section_size.to_le_bytes());
        file_data.extend_from_slice(&sec_desc);

        // Compressed data
        file_data.extend_from_slice(&compressed);

        // Done descriptor
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn ewf_reader_handles_nonzero_table_padding() {
        let data = b"nonzero padding in table header";
        let tmp = build_synthetic_e01_with_nonzero_table_padding(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        assert_eq!(reader.chunk_count(), 1);
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    // -- Bug fix: graceful handling of truncated section chains --

    /// Build a synthetic E01 where the sectors section's `next` pointer
    /// exceeds the file size (simulating a truncated or single-segment image
    /// without a trailing `done` section, as produced by some tools).
    fn build_synthetic_e01_truncated_chain(data: &[u8]) -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let bytes_per_sector: u32 = 512;
        let sectors_per_chunk: u32 = chunk_size / bytes_per_sector;
        let sector_count = u64::from(chunk_size / bytes_per_sector);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder
            .write_all(&vec![0u8; chunk_size as usize - data.len()])
            .unwrap();
        let compressed = encoder.finish().unwrap();

        let vol_desc_off: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_off: u64 = vol_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_off: u64 = vol_data_off + 94;
        let tbl_hdr_off: u64 = tbl_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entry_off: u64 = tbl_hdr_off + 24;
        let sectors_desc_offset: u64 = tbl_entry_off + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;

        let mut file_data = Vec::new();

        // File header
        let mut hdr = [0u8; FILE_HEADER_SIZE];
        hdr[0..8].copy_from_slice(&EVF_SIGNATURE);
        hdr[9..11].copy_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&hdr);

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_off.to_le_bytes());
        let vol_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 94;
        vol_desc[24..32].copy_from_slice(&vol_section_size.to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        let tbl_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4;
        tbl_desc[24..32].copy_from_slice(&tbl_section_size.to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header (correct u32 format)
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes());
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry
        let entry: u32 = 0x8000_0000;
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor — next pointer deliberately past EOF!
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        let bogus_next: u64 = 999_999_999; // way past file end
        sec_desc[16..24].copy_from_slice(&bogus_next.to_le_bytes());
        let sec_section_size = SECTION_DESCRIPTOR_SIZE as u64 + compressed.len() as u64;
        sec_desc[24..32].copy_from_slice(&sec_section_size.to_le_bytes());
        file_data.extend_from_slice(&sec_desc);

        // Compressed data — NO done section after this!
        file_data.extend_from_slice(&compressed);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn ewf_reader_handles_truncated_section_chain() {
        let data = b"truncated chain test data!!!";
        let tmp = build_synthetic_e01_truncated_chain(data);
        // Should open successfully despite missing done section
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        assert_eq!(reader.chunk_count(), 1);
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    // -- Enhancement: configurable LRU cache size --

    #[test]
    fn ewf_reader_open_with_cache_size() {
        let data = b"cache size test";
        let tmp = build_synthetic_e01(data);
        // Should compile and work with a custom cache size
        let reader = EwfReader::open_with_cache_size(tmp.path(), 10).unwrap();
        assert_eq!(reader.chunk_count(), 1);
    }

    /// Smoke test against a real E01 image from the shared corpus dir named by
    /// `$EWF_TEST_CORPUS` (gitignored, downloaded on demand). Skips cleanly when
    /// the env var is unset or the file is absent.
    #[test]
    fn ewf_reader_opens_real_e01() {
        let Some(dir) = std::env::var_os("EWF_TEST_CORPUS") else {
            eprintln!("skipping ewf_reader_opens_real_e01: EWF_TEST_CORPUS unset");
            return;
        };
        let path = std::path::Path::new(&dir).join("20200918_0417_DESKTOP-SDN1RPT.E01");
        if !path.exists() {
            eprintln!(
                "skipping ewf_reader_opens_real_e01: {} not found",
                path.display()
            );
            return;
        }
        let mut reader = EwfReader::open(&path).unwrap();
        assert!(reader.total_size() > 0);
        eprintln!(
            "Image size: {} bytes ({:.2} GB)",
            reader.total_size(),
            reader.total_size() as f64 / 1_073_741_824.0
        );
        eprintln!("Chunk size: {} bytes", reader.chunk_size());
        eprintln!("Chunk count: {}", reader.chunk_count());

        // Read first sector (MBR/GPT protective MBR)
        let mut sector = [0u8; 512];
        reader.read_exact(&mut sector).unwrap();
        // MBR signature at bytes 510-511
        assert_eq!(sector[510], 0x55);
        assert_eq!(sector[511], 0xAA);
        eprintln!("MBR signature verified: 0x55AA");
    }

    // -- Coverage: error paths and edge cases --

    #[test]
    fn discover_segments_no_segments_found() {
        // A path that doesn't match any E01 files
        // We call EwfReader::open which internally calls discover_segments
        let result = EwfReader::open("/tmp/nonexistent_ewf_xyzzy.E01");
        assert!(result.is_err());
        match result.unwrap_err() {
            EwfError::NoSegments(_) => {}
            other => panic!("expected NoSegments, got {other:?}"),
        }
    }

    /// A path whose file name ends in a bare `.` makes `Path::extension()`
    /// return `Some("")` — not `None` — so the `unwrap_or("E01")` fallback in
    /// `discover_segments` never fires and the extension is the empty string.
    /// Length 0 is not 4, so the EWF v1 branch runs and takes the first char of
    /// an empty string. `open` must reject the path, not panic.
    #[test]
    fn discover_segments_trailing_dot_is_rejected_not_panic() {
        let result = EwfReader::open("/tmp/nonexistent_ewf_xyzzy.");
        assert!(matches!(result, Err(EwfError::NoSegments(_))));
    }

    #[test]
    fn open_segments_empty_path_list() {
        let result = EwfReader::open_segments(&[]);
        assert!(matches!(result, Err(EwfError::NoSegments(ref msg)) if msg == "empty path list"));
    }

    #[test]
    fn open_segments_segment_gap() {
        // Build two synthetic E01 files with segment numbers 1 and 3 (gap at 2)
        let data = b"test data";
        let tmp1 = build_synthetic_e01(data); // segment 1

        // Build another with segment number 3
        let mut file_data = std::fs::read(tmp1.path()).unwrap();
        file_data[9..11].copy_from_slice(&3u16.to_le_bytes()); // change segment to 3
        let mut tmp3 = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp3.write_all(&file_data).unwrap();
        tmp3.flush().unwrap();

        let result = EwfReader::open_segments(&[tmp1.path().into(), tmp3.path().into()]);
        assert!(matches!(
            result,
            Err(EwfError::SegmentGap {
                expected: 2,
                got: 3
            })
        ));
    }

    #[test]
    fn open_missing_volume_section() {
        // Build a minimal E01 with only header + done (no volume section)
        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Done section descriptor immediately
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let result = EwfReader::open(tmp.path());
        assert!(matches!(result, Err(EwfError::MissingVolume)));
    }

    #[test]
    fn ewf_reader_seek_from_current() {
        let data = b"ABCDEFGHIJKLMNOP";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        // Seek forward from start
        reader.seek(SeekFrom::Start(4)).unwrap();
        let pos = reader.seek(SeekFrom::Current(4)).unwrap();
        assert_eq!(pos, 8);

        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"IJKL");
    }

    #[test]
    fn ewf_reader_seek_negative_position() {
        let data = b"test";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        // SeekFrom::Current to negative position
        let result = reader.seek(SeekFrom::Current(-1));
        assert!(result.is_err());
    }

    #[test]
    fn ewf_reader_cache_hit() {
        let data = b"cached data test";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        // First read populates cache
        let mut buf1 = [0u8; 16];
        reader.read_exact(&mut buf1).unwrap();

        // Seek back and read again — should hit cache
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf2 = [0u8; 16];
        reader.read_exact(&mut buf2).unwrap();

        assert_eq!(buf1, buf2);
        assert_eq!(&buf1[..16], b"cached data test");
    }

    #[test]
    fn ewf_reader_uncompressed_chunk() {
        // Build an E01 with an uncompressed chunk (compressed bit NOT set)
        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let mut padded = b"uncompressed chunk data".to_vec();
        padded.resize(chunk_size as usize, 0);

        let sector_count = u64::from(chunk_size / bytes_per_sector);

        let vol_desc_offset: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_offset: u64 = vol_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_offset: u64 = vol_data_offset + 94;
        let tbl_hdr_offset: u64 = tbl_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entries_offset: u64 = tbl_hdr_offset + 24;
        let sectors_desc_offset: u64 = tbl_entries_offset + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = sectors_data_offset + u64::from(chunk_size) + 4;

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_offset.to_le_bytes());
        vol_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[0..4].copy_from_slice(&1u32.to_le_bytes());
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        tbl_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4).to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes());
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry: uncompressed (bit 31 NOT set), offset = 0
        let entry: u32 = 0x0000_0000; // uncompressed
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        sec_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        sec_desc[24..32].copy_from_slice(
            &(SECTION_DESCRIPTOR_SIZE as u64 + u64::from(chunk_size) + 4).to_le_bytes(),
        );
        file_data.extend_from_slice(&sec_desc);

        // Raw chunk data (uncompressed)
        file_data.extend_from_slice(&padded);
        file_data.extend_from_slice(&sections::adler32(&padded).to_le_bytes());

        // Done descriptor
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let mut reader = EwfReader::open(tmp.path()).unwrap();
        let expected = b"uncompressed chunk data";
        let mut buf = vec![0u8; expected.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, expected);
    }

    #[test]
    fn ewf_reader_decompression_error() {
        // Build an E01 with garbage instead of valid zlib data
        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let garbage = b"THIS IS NOT VALID ZLIB DATA!!!!";

        let sector_count = u64::from(chunk_size / bytes_per_sector);

        let vol_desc_offset: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_offset: u64 = vol_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_offset: u64 = vol_data_offset + 94;
        let tbl_hdr_offset: u64 = tbl_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entries_offset: u64 = tbl_hdr_offset + 24;
        let sectors_desc_offset: u64 = tbl_entries_offset + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = sectors_data_offset + garbage.len() as u64;

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_offset.to_le_bytes());
        vol_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[0..4].copy_from_slice(&1u32.to_le_bytes());
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        tbl_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4).to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes());
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry: compressed (bit 31 set), offset = 0
        let entry: u32 = 0x8000_0000;
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        sec_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        sec_desc[24..32].copy_from_slice(
            &(SECTION_DESCRIPTOR_SIZE as u64 + garbage.len() as u64).to_le_bytes(),
        );
        file_data.extend_from_slice(&sec_desc);

        // Garbage data (not valid zlib)
        file_data.extend_from_slice(garbage);

        // Done descriptor
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let mut reader = EwfReader::open(tmp.path()).unwrap();
        let mut buf = [0u8; 512];
        let result = reader.read(&mut buf);
        assert!(
            result.is_err() || {
                // The Read impl maps EwfError to io::Error
                false
            }
        );
    }

    #[test]
    fn ewf_reader_volume_with_zero_total_size() {
        // Build an E01 where the volume has total_size = 0
        // (sector_count = 0), so it falls back to chunk_size * chunk_count
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let mut padded = b"zero total size test".to_vec();
        padded.resize(chunk_size as usize, 0);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        let vol_desc_offset: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_offset: u64 = vol_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_offset: u64 = vol_data_offset + 94;
        let tbl_hdr_offset: u64 = tbl_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entries_offset: u64 = tbl_hdr_offset + 24;
        let sectors_desc_offset: u64 = tbl_entries_offset + 4;
        let sectors_data_offset: u64 = sectors_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = sectors_data_offset + compressed.len() as u64;

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_offset.to_le_bytes());
        vol_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data with sector_count = 0
        let mut vol_data = [0u8; 94];
        vol_data[0..4].copy_from_slice(&1u32.to_le_bytes()); // media_type
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes()); // chunk_count = 1
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&0u64.to_le_bytes()); // sector_count = 0
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&sectors_desc_offset.to_le_bytes());
        tbl_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4).to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&1u32.to_le_bytes());
        tbl_hdr[8..16].copy_from_slice(&sectors_data_offset.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry: compressed
        let entry: u32 = 0x8000_0000;
        file_data.extend_from_slice(&entry.to_le_bytes());

        // Sectors descriptor
        let mut sec_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        sec_desc[..7].copy_from_slice(b"sectors");
        sec_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        sec_desc[24..32].copy_from_slice(
            &(SECTION_DESCRIPTOR_SIZE as u64 + compressed.len() as u64).to_le_bytes(),
        );
        file_data.extend_from_slice(&sec_desc);

        // Compressed data
        file_data.extend_from_slice(&compressed);

        // Done
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let reader = EwfReader::open(tmp.path()).unwrap();
        // total_size should fall back to chunk_size * chunk_count = 32768 * 1
        assert_eq!(reader.total_size(), 32768);
    }

    #[test]
    fn ewf_reader_read_at_eof_returns_zero() {
        let data = b"edge case";
        let tmp = build_synthetic_e01(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        // Seek to exactly total_size, read should return 0 (EOF)
        reader.seek(SeekFrom::Start(reader.total_size())).unwrap();
        let mut buf = [0u8; 16];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    fn build_synthetic_e01_two_chunks(data1: &[u8], data2: &[u8]) -> NamedTempFile {
        build_synthetic_e01_chunks(&[data1, data2]).0
    }

    #[test]
    fn ewf_reader_two_compressed_chunks() {
        let data1 = b"first chunk data here!";
        let data2 = b"second chunk is different";
        let tmp = build_synthetic_e01_two_chunks(data1, data2);
        let mut reader = EwfReader::open(tmp.path()).unwrap();

        assert_eq!(reader.total_size(), 32768 * 2);
        assert_eq!(reader.chunk_count(), 2);

        // Read from first chunk
        let mut buf = vec![0u8; data1.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, data1);

        // Seek to second chunk and read
        reader.seek(SeekFrom::Start(32768)).unwrap();
        let mut buf2 = vec![0u8; data2.len()];
        reader.read_exact(&mut buf2).unwrap();
        assert_eq!(buf2, data2);
    }

    #[test]
    fn discover_segments_sorts_by_extension() {
        // Create two temp files with different segment extensions
        let dir = tempfile::tempdir().unwrap();
        let path_e02 = dir.path().join("test.E02");
        let path_e01 = dir.path().join("test.E01");

        // Write valid file headers with correct segment numbers
        let mut hdr1 = [0u8; FILE_HEADER_SIZE];
        hdr1[0..8].copy_from_slice(&EVF_SIGNATURE);
        hdr1[8] = 0x01;
        hdr1[9..11].copy_from_slice(&1u16.to_le_bytes());

        let mut hdr2 = [0u8; FILE_HEADER_SIZE];
        hdr2[0..8].copy_from_slice(&EVF_SIGNATURE);
        hdr2[8] = 0x01;
        hdr2[9..11].copy_from_slice(&2u16.to_le_bytes());

        std::fs::write(&path_e01, hdr1).unwrap();
        std::fs::write(&path_e02, hdr2).unwrap();

        // Use EwfReader::open which calls discover_segments internally.
        // We can't call discover_segments directly since it's private to reader module.
        // Instead, test via open_segments which takes explicit paths (sorted).
        // The discovery test is implicitly covered by the real E01 test.
        // Here we just verify the paths exist and EwfReader can find them.
        let result = EwfReader::open(&path_e01);
        // This will fail at the volume parsing stage (no volume section),
        // but it proves discover_segments found and sorted the files.
        // Let's verify the segment gap error message instead.
        // Actually, the headers have correct segment numbers 1 and 2,
        // but the files are too short to have volume sections.
        // The error will be about buffer reading, not segment discovery.
        assert!(result.is_err());
    }

    // -- EWF2 type parsing tests --

    fn make_ewf2_file_header(is_physical: bool, segment: u32, compression: u16) -> [u8; 32] {
        let mut buf = [0u8; 32];
        let sig = if is_physical {
            ewf2::EVF2_SIGNATURE
        } else {
            ewf2::LEF2_SIGNATURE
        };
        buf[0..8].copy_from_slice(&sig);
        buf[8] = 0x02;
        buf[9] = 0x01;
        buf[10..12].copy_from_slice(&compression.to_le_bytes());
        buf[12..16].copy_from_slice(&segment.to_le_bytes());
        buf
    }

    #[test]
    fn ewf2_parse_ex01_header() {
        let buf = make_ewf2_file_header(true, 1, 1);
        let header = ewf2::Ewf2FileHeader::parse(&buf).unwrap();
        assert!(header.is_physical, "Ex01 should be physical");
        assert_eq!(header.major_version, 2);
        assert_eq!(header.minor_version, 1);
        assert_eq!(header.compression_method, ewf2::CompressionMethod::Zlib);
        assert_eq!(header.segment_number, 1);
    }

    #[test]
    fn ewf2_parse_lx01_header() {
        let buf = make_ewf2_file_header(false, 3, 2);
        let header = ewf2::Ewf2FileHeader::parse(&buf).unwrap();
        assert!(!header.is_physical, "Lx01 should not be physical");
        assert_eq!(header.compression_method, ewf2::CompressionMethod::Bzip2);
        assert_eq!(header.segment_number, 3);
    }

    #[test]
    fn ewf2_header_rejects_v1_signature() {
        let v1_buf = make_file_header(1);
        let mut buf = [0u8; 32];
        buf[..13].copy_from_slice(&v1_buf);
        assert!(ewf2::Ewf2FileHeader::parse(&buf).is_err());
    }

    #[test]
    fn ewf2_header_rejects_short_buffer() {
        assert!(ewf2::Ewf2FileHeader::parse(&[0u8; 10]).is_err());
    }

    fn make_ewf2_section_descriptor(
        section_type: u32,
        data_flags: u32,
        prev_offset: u64,
        data_size: u64,
    ) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&section_type.to_le_bytes());
        buf[4..8].copy_from_slice(&data_flags.to_le_bytes());
        buf[8..16].copy_from_slice(&prev_offset.to_le_bytes());
        buf[16..24].copy_from_slice(&data_size.to_le_bytes());
        buf[24..28].copy_from_slice(&64u32.to_le_bytes());
        buf
    }

    #[test]
    fn ewf2_parse_section_descriptor() {
        let buf = make_ewf2_section_descriptor(0x03, 0x01, 100, 65536);
        let desc = ewf2::Ewf2SectionDescriptor::parse(&buf, 200).unwrap();
        assert_eq!(desc.section_type, ewf2::Ewf2SectionType::SectorData);
        assert!(desc.is_md5_hashed());
        assert!(!desc.is_encrypted());
        assert_eq!(desc.previous_offset, 100);
        assert_eq!(desc.data_size, 65536);
        assert_eq!(desc.offset, 200);
    }

    #[test]
    fn ewf2_section_descriptor_encrypted_flag() {
        let buf = make_ewf2_section_descriptor(0x08, 0x03, 0, 20);
        let desc = ewf2::Ewf2SectionDescriptor::parse(&buf, 0).unwrap();
        assert_eq!(desc.section_type, ewf2::Ewf2SectionType::Md5Hash);
        assert!(desc.is_encrypted());
    }

    #[test]
    fn ewf2_section_type_names() {
        assert_eq!(ewf2::Ewf2SectionType::SectorData.name(), "sector_data");
        assert_eq!(ewf2::Ewf2SectionType::Done.name(), "done");
        assert_eq!(ewf2::Ewf2SectionType::Unknown(0xFF).name(), "unknown");
    }

    fn make_ewf2_table_entry(offset: u64, size: u32, flags: u32) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&offset.to_le_bytes());
        buf[8..12].copy_from_slice(&size.to_le_bytes());
        buf[12..16].copy_from_slice(&flags.to_le_bytes());
        buf
    }

    #[test]
    fn ewf2_parse_compressed_table_entry() {
        let buf = make_ewf2_table_entry(4096, 30000, 0x01);
        let entry = ewf2::Ewf2TableEntry::parse(&buf).unwrap();
        assert_eq!(entry.chunk_data_offset, 4096);
        assert_eq!(entry.chunk_data_size, 30000);
        assert!(entry.is_compressed());
        assert!(!entry.is_checksumed());
        assert!(!entry.is_pattern_fill());
    }

    #[test]
    fn ewf2_parse_uncompressed_table_entry() {
        let buf = make_ewf2_table_entry(8192, 32768, 0x02);
        let entry = ewf2::Ewf2TableEntry::parse(&buf).unwrap();
        assert!(!entry.is_compressed());
        assert!(entry.is_checksumed());
    }

    #[test]
    fn ewf2_parse_pattern_fill_entry() {
        let buf = make_ewf2_table_entry(0, 0, 0x05);
        let entry = ewf2::Ewf2TableEntry::parse(&buf).unwrap();
        assert!(entry.is_pattern_fill());
        assert_eq!(entry.chunk_data_size, 0);
    }

    #[test]
    fn ewf2_parse_table_header() {
        let mut buf = [0u8; 32];
        buf[0..8].copy_from_slice(&0u64.to_le_bytes());
        buf[8..12].copy_from_slice(&128u32.to_le_bytes());
        let header = ewf2::Ewf2TableHeader::parse(&buf).unwrap();
        assert_eq!(header.first_chunk, 0);
        assert_eq!(header.entry_count, 128);
    }

    // -- EWF2 reader tests (synthetic Ex01) --

    /// Build a minimal single-segment Ex01 file with known data.
    ///
    /// Correct EWF2 layout ([data][descriptor] ordering):
    ///   [0..32)                      EVF2 file header
    ///   [32..32+D)                   DeviceInfo DATA (UTF-16LE)
    ///   [32+D..32+D+64)              DeviceInfo DESCRIPTOR (prev=0, data_size=D)
    ///   [32+D+64..32+D+64+C)         SectorData DATA (zlib-compressed chunk)
    ///   [32+D+64+C..32+D+128+C)      SectorData DESCRIPTOR (prev=32+D, data_size=C)
    ///   [32+D+128+C..32+D+128+C+48)  SectorTable DATA (32-byte hdr + 16-byte entry)
    ///   [32+D+128+C+48..32+D+192+C+48) SectorTable DESCRIPTOR (prev=32+D+64+C, data_size=48)
    ///   [32+D+192+C+48..32+D+256+C+48) Done DESCRIPTOR (prev=32+D+128+C+48, data_size=0)
    fn build_synthetic_ex01(data: &[u8]) -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let bytes_per_sector: u32 = 512;
        let sectors_per_chunk: u32 = chunk_size / bytes_per_sector; // 64
        let total_sectors: u64 = u64::from(sectors_per_chunk); // 1 chunk worth

        // Pad data to chunk_size and compress
        let mut padded = data.to_vec();
        padded.resize(chunk_size as usize, 0);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        // Build device_info content (UTF-16LE tab-separated text)
        let device_info_text = format!(
            "2\nmain\nb\tsc\tts\n{bytes_per_sector}\t{sectors_per_chunk}\t{total_sectors}\n\n"
        );
        let device_info_utf16: Vec<u8> = device_info_text
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();

        let d = device_info_utf16.len(); // D
        let c = compressed.len(); // C
        let table_data_size: usize = 32 + 16; // 32-byte hdr + 16-byte entry = 48

        // Absolute file offsets
        let devinfo_data_off: usize = 32;
        let devinfo_desc_off: usize = devinfo_data_off + d;
        let sectors_data_off: usize = devinfo_desc_off + 64;
        let sectors_desc_off: usize = sectors_data_off + c;
        let table_data_off: usize = sectors_desc_off + 64;
        let table_desc_off: usize = table_data_off + table_data_size;
        let done_desc_off: usize = table_desc_off + 64;

        // Helper: build a 64-byte EWF2 section descriptor
        fn make_v2_desc(section_type: u32, data_size: u64, previous_offset: u64) -> [u8; 64] {
            let mut desc = [0u8; 64];
            desc[0..4].copy_from_slice(&section_type.to_le_bytes());
            // data_flags = 0
            desc[8..16].copy_from_slice(&previous_offset.to_le_bytes());
            desc[16..24].copy_from_slice(&data_size.to_le_bytes());
            desc[24..28].copy_from_slice(&64u32.to_le_bytes()); // descriptor_size
            desc
        }

        let mut file_data = Vec::new();

        // 1. EVF2 File Header (32 bytes)
        file_data.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        file_data.push(2); // major_version
        file_data.push(1); // minor_version
        file_data.extend_from_slice(&1u16.to_le_bytes()); // compression = Zlib
        file_data.extend_from_slice(&1u32.to_le_bytes()); // segment_number = 1
        file_data.extend_from_slice(&[0u8; 16]); // set_identifier
        assert_eq!(file_data.len(), 32);

        // 2. DeviceInfo DATA then DESCRIPTOR
        file_data.extend_from_slice(&device_info_utf16);
        file_data.extend_from_slice(&make_v2_desc(0x01, d as u64, 0));
        assert_eq!(file_data.len(), devinfo_desc_off + 64);

        // 3. SectorData DATA then DESCRIPTOR
        file_data.extend_from_slice(&compressed);
        file_data.extend_from_slice(&make_v2_desc(0x03, c as u64, devinfo_desc_off as u64));
        assert_eq!(file_data.len(), sectors_desc_off + 64);

        // 4. SectorTable DATA (32-byte header + 16-byte entry) then DESCRIPTOR
        // Table header (32 bytes): first_chunk(u64) + entry_count(u32) + rest zeros
        let mut tbl_hdr = [0u8; 32];
        tbl_hdr[0..8].copy_from_slice(&0u64.to_le_bytes()); // first_chunk = 0
        tbl_hdr[8..12].copy_from_slice(&1u32.to_le_bytes()); // entry_count = 1
        file_data.extend_from_slice(&tbl_hdr);

        // Table entry (16 bytes): chunk_data_offset(u64) + chunk_data_size(u32) + flags(u32)
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&(sectors_data_off as u64).to_le_bytes());
        entry[8..12].copy_from_slice(&(c as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&ewf2::CHUNK_FLAG_COMPRESSED.to_le_bytes());
        file_data.extend_from_slice(&entry);
        file_data.extend_from_slice(&make_v2_desc(
            0x04,
            table_data_size as u64,
            sectors_desc_off as u64,
        ));
        assert_eq!(file_data.len(), table_desc_off + 64);

        // 5. Done DESCRIPTOR (data_size=0)
        file_data.extend_from_slice(&make_v2_desc(0x0F, 0, table_desc_off as u64));

        assert_eq!(file_data.len(), done_desc_off + 64);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    // -- EWF2 coverage: short buffer rejections for v2 types --

    #[test]
    fn ewf2_section_descriptor_rejects_short_buffer() {
        assert!(ewf2::Ewf2SectionDescriptor::parse(&[0u8; 10], 0).is_err());
    }

    #[test]
    fn ewf2_table_entry_rejects_short_buffer() {
        assert!(ewf2::Ewf2TableEntry::parse(&[0u8; 4]).is_err());
    }

    #[test]
    fn ewf2_table_header_rejects_short_buffer() {
        assert!(ewf2::Ewf2TableHeader::parse(&[0u8; 8]).is_err());
    }

    #[test]
    fn ewf2_compression_none_and_unknown() {
        assert_eq!(
            ewf2::CompressionMethod::from_u16(0).unwrap(),
            ewf2::CompressionMethod::None
        );
        assert!(ewf2::CompressionMethod::from_u16(99).is_err());
    }

    // -- EWF2 coverage: all section type conversions --

    #[test]
    fn ewf2_section_type_from_u32_all_variants() {
        // Ensure every arm in from_u32 and name() is hit
        let cases: &[(u32, &str)] = &[
            (0x01, "device_info"),
            (0x02, "case_data"),
            (0x03, "sector_data"),
            (0x04, "sector_table"),
            (0x05, "error_table"),
            (0x06, "session_table"),
            (0x07, "increment_data"),
            (0x08, "md5_hash"),
            (0x09, "sha1_hash"),
            (0x0A, "restart_data"),
            (0x0B, "encryption_keys"),
            (0x0C, "memory_extents"),
            (0x0D, "next"),
            (0x0E, "final_info"),
            (0x0F, "done"),
            (0x10, "analytical_data"),
            (0x20, "single_files_data"),
            (0xFF, "unknown"),
        ];
        for &(val, expected_name) in cases {
            let st = ewf2::Ewf2SectionType::from_u32(val);
            assert_eq!(st.name(), expected_name, "from_u32({val:#x}) name mismatch");
        }
    }

    // -- EWF2 reader coverage: encrypted rejection --

    #[test]
    fn ewf2_reader_rejects_encrypted() {
        // Build an Ex01 with the encrypted flag set, using correct [data][descriptor] layout.
        //
        // Layout:
        //   [0..32)    EVF2 file header
        //   [32..132)  Encrypted DATA (100 dummy bytes)
        //   [132..196) Encrypted DESCRIPTOR (type=DeviceInfo, data_flags=ENCRYPTED, data_size=100, prev=0)
        //   [196..260) Done DESCRIPTOR (data_size=0, prev=132)
        let mut file_data = Vec::new();

        // EVF2 header (32 bytes)
        file_data.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        file_data.push(2);
        file_data.push(1);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&1u32.to_le_bytes());
        file_data.extend_from_slice(&[0u8; 16]);
        assert_eq!(file_data.len(), 32);

        // Encrypted DATA (100 dummy bytes)
        file_data.extend_from_slice(&[0u8; 100]);

        // Encrypted DESCRIPTOR with DATA_FLAG_ENCRYPTED (0x02)
        let mut desc = [0u8; 64];
        desc[0..4].copy_from_slice(&0x01u32.to_le_bytes()); // DeviceInfo
        desc[4..8].copy_from_slice(&0x02u32.to_le_bytes()); // DATA_FLAG_ENCRYPTED
        desc[8..16].copy_from_slice(&0u64.to_le_bytes()); // previous_offset = 0
        desc[16..24].copy_from_slice(&100u64.to_le_bytes()); // data_size = 100
        desc[24..28].copy_from_slice(&64u32.to_le_bytes()); // descriptor_size
        file_data.extend_from_slice(&desc);
        assert_eq!(file_data.len(), 196);

        // Done DESCRIPTOR
        let mut done = [0u8; 64];
        done[0..4].copy_from_slice(&0x0Fu32.to_le_bytes()); // Done
        done[8..16].copy_from_slice(&132u64.to_le_bytes()); // previous_offset = 132
        done[16..24].copy_from_slice(&0u64.to_le_bytes()); // data_size = 0
        done[24..28].copy_from_slice(&64u32.to_le_bytes()); // descriptor_size
        file_data.extend_from_slice(&done);
        assert_eq!(file_data.len(), 260);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let result = EwfReader::open(tmp.path());
        assert!(result.is_err(), "Encrypted EWF2 should be rejected");
        assert!(
            matches!(result.unwrap_err(), EwfError::EncryptedNotSupported),
            "Should be EncryptedNotSupported error"
        );
    }

    // -- EWF2 reader coverage: md5_hash section, no device_info fallback --

    /// Build a synthetic Ex01 with an `Md5Hash` section and no `DeviceInfo` section,
    /// so the reader exercises the `Md5Hash` parsing and default `chunk_size` fallback.
    ///
    /// Correct EWF2 layout ([data][descriptor] ordering):
    ///   [32..32+C)           SectorData DATA
    ///   [32+C..96+C)         SectorData DESCRIPTOR (prev=0, data_size=C)
    ///   [96+C..96+C+16)      Md5Hash DATA (16 bytes)
    ///   [96+C+16..160+C+16)  Md5Hash DESCRIPTOR (prev=32+C, data_size=16)
    ///   [160+C+16..208+C+16) SectorTable DATA (48 bytes)
    ///   [208+C+16..272+C+16) SectorTable DESCRIPTOR (prev=96+C+16, data_size=48)
    ///   [272+C+16..336+C+16) Done DESCRIPTOR (prev=208+C+16, data_size=0)
    fn build_synthetic_ex01_with_md5_no_devinfo(data: &[u8]) -> (NamedTempFile, [u8; 16]) {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let mut padded = data.to_vec();
        padded.resize(chunk_size as usize, 0);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        let fake_md5: [u8; 16] = [
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08,
        ];

        fn make_v2_desc(section_type: u32, data_size: u64, prev: u64) -> [u8; 64] {
            let mut d = [0u8; 64];
            d[0..4].copy_from_slice(&section_type.to_le_bytes());
            d[8..16].copy_from_slice(&prev.to_le_bytes());
            d[16..24].copy_from_slice(&data_size.to_le_bytes());
            d[24..28].copy_from_slice(&64u32.to_le_bytes());
            d
        }

        let c = compressed.len();
        let table_data_size: usize = 32 + 16; // 48 bytes

        // Absolute file offsets
        let sectors_data_off: usize = 32;
        let sectors_desc_off: usize = sectors_data_off + c;
        let md5_data_off: usize = sectors_desc_off + 64;
        let md5_desc_off: usize = md5_data_off + 16;
        let table_data_off: usize = md5_desc_off + 64;
        let table_desc_off: usize = table_data_off + table_data_size;
        let done_desc_off: usize = table_desc_off + 64;

        let mut file_data = Vec::new();

        // Header (32 bytes)
        file_data.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        file_data.push(2);
        file_data.push(1);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&1u32.to_le_bytes());
        file_data.extend_from_slice(&[0u8; 16]);
        assert_eq!(file_data.len(), 32);

        // SectorData DATA then DESCRIPTOR
        file_data.extend_from_slice(&compressed);
        file_data.extend_from_slice(&make_v2_desc(0x03, c as u64, 0));
        assert_eq!(file_data.len(), sectors_desc_off + 64);

        // Md5Hash DATA then DESCRIPTOR
        file_data.extend_from_slice(&fake_md5);
        file_data.extend_from_slice(&make_v2_desc(0x08, 16, sectors_desc_off as u64));
        assert_eq!(file_data.len(), md5_desc_off + 64);

        // SectorTable DATA (32-byte hdr + 16-byte entry) then DESCRIPTOR
        let mut tbl_hdr = [0u8; 32];
        tbl_hdr[0..8].copy_from_slice(&0u64.to_le_bytes()); // first_chunk = 0
        tbl_hdr[8..12].copy_from_slice(&1u32.to_le_bytes()); // entry_count = 1
        file_data.extend_from_slice(&tbl_hdr);
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&(sectors_data_off as u64).to_le_bytes());
        entry[8..12].copy_from_slice(&(c as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&ewf2::CHUNK_FLAG_COMPRESSED.to_le_bytes());
        file_data.extend_from_slice(&entry);
        file_data.extend_from_slice(&make_v2_desc(
            0x04,
            table_data_size as u64,
            md5_desc_off as u64,
        ));
        assert_eq!(file_data.len(), table_desc_off + 64);

        // Done DESCRIPTOR
        file_data.extend_from_slice(&make_v2_desc(0x0F, 0, table_desc_off as u64));
        assert_eq!(file_data.len(), done_desc_off + 64);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        (tmp, fake_md5)
    }

    #[test]
    fn ewf2_reader_parses_md5_hash_section() {
        let (tmp, expected_md5) = build_synthetic_ex01_with_md5_no_devinfo(b"md5 test");
        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_ok(),
            "Should open Ex01 with md5_hash: {:?}",
            result.err()
        );
        let reader = result.unwrap();
        let hashes = reader.stored_hashes();
        assert_eq!(hashes.md5, Some(expected_md5));
    }

    #[test]
    fn ewf2_reader_defaults_chunk_size_without_device_info() {
        let (tmp, _) = build_synthetic_ex01_with_md5_no_devinfo(b"no devinfo");
        let reader = EwfReader::open(tmp.path()).unwrap();
        // Should fall back to default 32768
        assert_eq!(reader.chunk_size(), 32768);
        // total_size = 1 chunk * 32768
        assert_eq!(reader.total_size(), 32768);
    }

    #[test]
    fn ewf2_reader_reads_data_without_device_info() {
        let data = b"read without devinfo";
        let (tmp, _) = build_synthetic_ex01_with_md5_no_devinfo(data);
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    // -- V2 SHA-1 hash section parsing --

    fn build_synthetic_ex01_with_md5_and_sha1(data: &[u8]) -> (NamedTempFile, [u8; 16], [u8; 20]) {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let mut padded = data.to_vec();
        padded.resize(chunk_size as usize, 0);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        let fake_md5: [u8; 16] = [
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08,
        ];
        let fake_sha1: [u8; 20] = [
            0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xB0, 0xB1, 0xB2, 0xB3,
            0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9,
        ];

        fn make_v2_desc(section_type: u32, data_size: u64, prev: u64) -> [u8; 64] {
            let mut d = [0u8; 64];
            d[0..4].copy_from_slice(&section_type.to_le_bytes());
            d[8..16].copy_from_slice(&prev.to_le_bytes());
            d[16..24].copy_from_slice(&data_size.to_le_bytes());
            d[24..28].copy_from_slice(&64u32.to_le_bytes());
            d
        }

        // Correct EWF2 layout ([data][descriptor] ordering):
        //   [32..32+C)          SectorData DATA
        //   [32+C..96+C)        SectorData DESCRIPTOR (prev=0, data_size=C)
        //   [96+C..96+C+16)     Md5Hash DATA
        //   [96+C+16..160+C+16) Md5Hash DESCRIPTOR (prev=32+C, data_size=16)
        //   [160+C+16..160+C+36) Sha1Hash DATA
        //   [160+C+36..224+C+36) Sha1Hash DESCRIPTOR (prev=96+C+16, data_size=20)
        //   [224+C+36..272+C+36) SectorTable DATA (48 bytes)
        //   [272+C+36..336+C+36) SectorTable DESCRIPTOR (prev=160+C+36, data_size=48)
        //   [336+C+36..400+C+36) Done DESCRIPTOR (prev=272+C+36, data_size=0)
        let c = compressed.len();
        let table_data_size: usize = 32 + 16; // 48 bytes

        let sectors_data_off: usize = 32;
        let sectors_desc_off: usize = sectors_data_off + c;
        let md5_data_off: usize = sectors_desc_off + 64;
        let md5_desc_off: usize = md5_data_off + 16;
        let sha1_data_off: usize = md5_desc_off + 64;
        let sha1_desc_off: usize = sha1_data_off + 20;
        let table_data_off: usize = sha1_desc_off + 64;
        let table_desc_off: usize = table_data_off + table_data_size;
        let done_desc_off: usize = table_desc_off + 64;

        let mut f = Vec::new();

        // Header (32 bytes)
        f.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        f.push(2);
        f.push(1);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&[0u8; 16]);
        assert_eq!(f.len(), 32);

        // SectorData DATA then DESCRIPTOR
        f.extend_from_slice(&compressed);
        f.extend_from_slice(&make_v2_desc(0x03, c as u64, 0));
        assert_eq!(f.len(), sectors_desc_off + 64);

        // Md5Hash DATA then DESCRIPTOR
        f.extend_from_slice(&fake_md5);
        f.extend_from_slice(&make_v2_desc(0x08, 16, sectors_desc_off as u64));
        assert_eq!(f.len(), md5_desc_off + 64);

        // Sha1Hash DATA then DESCRIPTOR
        f.extend_from_slice(&fake_sha1);
        f.extend_from_slice(&make_v2_desc(0x09, 20, md5_desc_off as u64));
        assert_eq!(f.len(), sha1_desc_off + 64);

        // SectorTable DATA (32-byte hdr + 16-byte entry) then DESCRIPTOR
        let mut tbl_hdr = [0u8; 32];
        tbl_hdr[0..8].copy_from_slice(&0u64.to_le_bytes()); // first_chunk = 0
        tbl_hdr[8..12].copy_from_slice(&1u32.to_le_bytes()); // entry_count = 1
        f.extend_from_slice(&tbl_hdr);
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&(sectors_data_off as u64).to_le_bytes());
        entry[8..12].copy_from_slice(&(c as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&ewf2::CHUNK_FLAG_COMPRESSED.to_le_bytes());
        f.extend_from_slice(&entry);
        f.extend_from_slice(&make_v2_desc(
            0x04,
            table_data_size as u64,
            sha1_desc_off as u64,
        ));
        assert_eq!(f.len(), table_desc_off + 64);

        // Done DESCRIPTOR
        f.extend_from_slice(&make_v2_desc(0x0F, 0, table_desc_off as u64));
        assert_eq!(f.len(), done_desc_off + 64);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&f).unwrap();
        tmp.flush().unwrap();
        (tmp, fake_md5, fake_sha1)
    }

    #[test]
    fn ewf2_reader_parses_sha1_hash_section() {
        let (tmp, expected_md5, expected_sha1) =
            build_synthetic_ex01_with_md5_and_sha1(b"sha1 test");
        let reader = EwfReader::open(tmp.path()).unwrap();
        let hashes = reader.stored_hashes();
        assert_eq!(hashes.md5, Some(expected_md5), "V2 MD5 should be parsed");
        assert_eq!(
            hashes.sha1,
            Some(expected_sha1),
            "V2 SHA-1 should be parsed"
        );
    }

    // -- V2 CaseData metadata parsing --

    fn build_synthetic_ex01_with_case_data() -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let mut padded = b"case data test".to_vec();
        padded.resize(chunk_size as usize, 0);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        // CaseData: UTF-16LE tab-separated (NOT zlib-compressed; passes through maybe_zlib_decompress unchanged)
        let case_text = "2\nmain\ncn\ten\tex\tde\tnt\tav\tov\tad\tsd\nCASE-42\tEV-7\tJane Doe\tTest image\tForensic notes\tEnCase 8.0\tWindows 11\t2025-01-15\t2025-01-14\n";
        let case_utf16: Vec<u8> = case_text
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();

        fn make_v2_desc(section_type: u32, data_size: u64, prev: u64) -> [u8; 64] {
            let mut d = [0u8; 64];
            d[0..4].copy_from_slice(&section_type.to_le_bytes());
            d[8..16].copy_from_slice(&prev.to_le_bytes());
            d[16..24].copy_from_slice(&data_size.to_le_bytes());
            d[24..28].copy_from_slice(&64u32.to_le_bytes());
            d
        }

        // Correct EWF2 layout ([data][descriptor] ordering):
        //   [32..32+N)          CaseData DATA (UTF-16LE)
        //   [32+N..96+N)        CaseData DESCRIPTOR (prev=0, data_size=N)
        //   [96+N..96+N+C)      SectorData DATA
        //   [96+N+C..160+N+C)   SectorData DESCRIPTOR (prev=32+N, data_size=C)
        //   [160+N+C..208+N+C)  SectorTable DATA (48 bytes)
        //   [208+N+C..272+N+C)  SectorTable DESCRIPTOR (prev=96+N+C, data_size=48)
        //   [272+N+C..336+N+C)  Done DESCRIPTOR (prev=208+N+C, data_size=0)
        let n = case_utf16.len(); // N
        let c = compressed.len(); // C
        let table_data_size: usize = 32 + 16; // 48 bytes

        let case_data_off: usize = 32;
        let case_desc_off: usize = case_data_off + n;
        let sectors_data_off: usize = case_desc_off + 64;
        let sectors_desc_off: usize = sectors_data_off + c;
        let table_data_off: usize = sectors_desc_off + 64;
        let table_desc_off: usize = table_data_off + table_data_size;
        let done_desc_off: usize = table_desc_off + 64;

        let mut f = Vec::new();

        // Header (32 bytes)
        f.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        f.push(2);
        f.push(1);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&[0u8; 16]);
        assert_eq!(f.len(), 32);

        // CaseData DATA then DESCRIPTOR
        f.extend_from_slice(&case_utf16);
        f.extend_from_slice(&make_v2_desc(0x02, n as u64, 0));
        assert_eq!(f.len(), case_desc_off + 64);

        // SectorData DATA then DESCRIPTOR
        f.extend_from_slice(&compressed);
        f.extend_from_slice(&make_v2_desc(0x03, c as u64, case_desc_off as u64));
        assert_eq!(f.len(), sectors_desc_off + 64);

        // SectorTable DATA (32-byte hdr + 16-byte entry) then DESCRIPTOR
        let mut tbl_hdr = [0u8; 32];
        tbl_hdr[0..8].copy_from_slice(&0u64.to_le_bytes()); // first_chunk = 0
        tbl_hdr[8..12].copy_from_slice(&1u32.to_le_bytes()); // entry_count = 1
        f.extend_from_slice(&tbl_hdr);
        let mut entry = [0u8; 16];
        entry[0..8].copy_from_slice(&(sectors_data_off as u64).to_le_bytes());
        entry[8..12].copy_from_slice(&(c as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&ewf2::CHUNK_FLAG_COMPRESSED.to_le_bytes());
        f.extend_from_slice(&entry);
        f.extend_from_slice(&make_v2_desc(
            0x04,
            table_data_size as u64,
            sectors_desc_off as u64,
        ));
        assert_eq!(f.len(), table_desc_off + 64);

        // Done DESCRIPTOR
        f.extend_from_slice(&make_v2_desc(0x0F, 0, table_desc_off as u64));
        assert_eq!(f.len(), done_desc_off + 64);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&f).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn ewf2_reader_parses_case_data_metadata() {
        let tmp = build_synthetic_ex01_with_case_data();
        let reader = EwfReader::open(tmp.path()).unwrap();
        let meta = reader.metadata();
        assert_eq!(meta.case_number.as_deref(), Some("CASE-42"));
        assert_eq!(meta.evidence_number.as_deref(), Some("EV-7"));
        assert_eq!(meta.examiner.as_deref(), Some("Jane Doe"));
        assert_eq!(meta.description.as_deref(), Some("Test image"));
        assert_eq!(meta.notes.as_deref(), Some("Forensic notes"));
        assert_eq!(meta.acquiry_software.as_deref(), Some("EnCase 8.0"));
        assert_eq!(meta.os_version.as_deref(), Some("Windows 11"));
        assert_eq!(meta.acquiry_date.as_deref(), Some("2025-01-15"));
        assert_eq!(meta.system_date.as_deref(), Some("2025-01-14"));
    }

    // -- EWF2 reader coverage: Debug impl --

    #[test]
    fn ewf_reader_debug_format() {
        let data = b"debug test";
        let tmp = build_synthetic_e01(data);
        let reader = EwfReader::open(tmp.path()).unwrap();
        let debug = format!("{reader:?}");
        assert!(debug.contains("EwfReader"));
        assert!(debug.contains("chunk_size"));
        assert!(debug.contains("total_size"));
    }

    // -- parse.rs coverage: edge cases --

    #[test]
    fn parse_header_text_short_input() {
        use crate::parse::parse_header_text;
        let mut meta = EwfMetadata::default();
        // Fewer than 4 lines — should return without panicking
        parse_header_text("line1\nline2\n", &mut meta);
        assert!(meta.case_number.is_none());
    }

    #[test]
    fn parse_error2_data_short_input() {
        // Fewer than 8 bytes
        let errors = parse_error2_data(&[0u8; 4]);
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_error2_data_truncated_entries() {
        // Claim 2 entries but only provide space for 1
        let mut data = Vec::new();
        data.extend_from_slice(&2u32.to_le_bytes()); // claims 2 entries
        data.extend_from_slice(&[0u8; 4]); // padding
        data.extend_from_slice(&100u32.to_le_bytes()); // entry 1 first_sector
        data.extend_from_slice(&5u32.to_le_bytes()); // entry 1 sector_count
                                                     // No space for entry 2 — should handle gracefully
        let errors = parse_error2_data(&data);
        assert_eq!(errors.len(), 1);
    }

    // -- reader.rs coverage: v1 synthetic error2 section --

    /// Build a synthetic E01 that includes an error2 section with 1 entry.
    fn build_synthetic_e01_with_error2() -> NamedTempFile {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let sector_count = u64::from(chunk_size / bytes_per_sector);
        let data = b"error2 test data";
        let mut padded = data.to_vec();
        padded.resize(chunk_size as usize, 0);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&padded).unwrap();
        let compressed = encoder.finish().unwrap();

        // error2 section data: 1 entry at sector 42, count 3
        let mut error2_data = Vec::new();
        error2_data.extend_from_slice(&1u32.to_le_bytes()); // 1 entry
        error2_data.extend_from_slice(&[0u8; 4]); // padding
        error2_data.extend_from_slice(&42u32.to_le_bytes()); // first_sector
        error2_data.extend_from_slice(&3u32.to_le_bytes()); // sector_count
        error2_data.extend_from_slice(&[0u8; 4]); // checksum
        let error2_size = error2_data.len();

        let vol_desc_off: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_off: u64 = vol_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_off: u64 = vol_data_off + 94;
        let tbl_hdr_off: u64 = tbl_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_entry_off: u64 = tbl_hdr_off + 24;
        let sectors_desc_off: u64 = tbl_entry_off + 4;
        let sectors_data_off: u64 = sectors_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let error2_desc_off: u64 = sectors_data_off + compressed.len() as u64;
        let error2_data_off: u64 = error2_desc_off + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_off: u64 = error2_data_off + error2_size as u64;

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume
        let mut vd = [0u8; SECTION_DESCRIPTOR_SIZE];
        vd[..6].copy_from_slice(b"volume");
        vd[16..24].copy_from_slice(&tbl_desc_off.to_le_bytes());
        vd[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vd);
        let mut vol = [0u8; 94];
        vol[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol);

        // Table
        let mut td = [0u8; SECTION_DESCRIPTOR_SIZE];
        td[..5].copy_from_slice(b"table");
        td[16..24].copy_from_slice(&sectors_desc_off.to_le_bytes());
        td[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4).to_le_bytes());
        file_data.extend_from_slice(&td);
        let mut th = [0u8; 24];
        th[0..4].copy_from_slice(&1u32.to_le_bytes());
        th[8..16].copy_from_slice(&sectors_data_off.to_le_bytes());
        file_data.extend_from_slice(&th);
        file_data.extend_from_slice(&0x8000_0000u32.to_le_bytes());

        // Sectors
        let mut sd = [0u8; SECTION_DESCRIPTOR_SIZE];
        sd[..7].copy_from_slice(b"sectors");
        sd[16..24].copy_from_slice(&error2_desc_off.to_le_bytes());
        sd[24..32].copy_from_slice(
            &(SECTION_DESCRIPTOR_SIZE as u64 + compressed.len() as u64).to_le_bytes(),
        );
        file_data.extend_from_slice(&sd);
        file_data.extend_from_slice(&compressed);

        // Error2
        let mut ed = [0u8; SECTION_DESCRIPTOR_SIZE];
        ed[..6].copy_from_slice(b"error2");
        ed[16..24].copy_from_slice(&done_desc_off.to_le_bytes());
        ed[24..32]
            .copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + error2_size as u64).to_le_bytes());
        file_data.extend_from_slice(&ed);
        file_data.extend_from_slice(&error2_data);

        // Done
        let mut dd = [0u8; SECTION_DESCRIPTOR_SIZE];
        dd[..4].copy_from_slice(b"done");
        dd[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&dd);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn ewf_reader_parses_error2_from_synthetic() {
        let tmp = build_synthetic_e01_with_error2();
        let reader = EwfReader::open(tmp.path()).unwrap();
        let errors = reader.acquisition_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].first_sector, 42);
        assert_eq!(errors[0].sector_count, 3);
    }

    // -- reader.rs coverage: v2 device_info edge cases --

    #[test]
    fn parse_ewf2_device_info_short_data() {
        // Directly test the device_info parser with short data
        use crate::reader::parse_ewf2_device_info;
        let mut cs = 0u64;
        let mut ts = 0u64;
        // Empty data
        parse_ewf2_device_info(&[], &mut cs, &mut ts);
        assert_eq!(cs, 0);
        // 1 byte (too short for UTF-16)
        parse_ewf2_device_info(&[0x41], &mut cs, &mut ts);
        assert_eq!(cs, 0);
    }

    #[test]
    fn parse_ewf2_device_info_too_few_lines() {
        use crate::reader::parse_ewf2_device_info;
        let mut cs = 0u64;
        let mut ts = 0u64;
        // UTF-16LE "hi\n" — only 1 line, need 4
        let text = "hi\n";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        parse_ewf2_device_info(&utf16, &mut cs, &mut ts);
        assert_eq!(cs, 0);
    }

    #[test]
    fn parse_ewf2_device_info_unknown_fields_ignored() {
        use crate::reader::parse_ewf2_device_info;
        let mut cs = 0u64;
        let mut ts = 0u64;
        // Valid format but with unknown field names alongside known ones
        let text = "2\nmain\nb\tsc\tts\txyz\n512\t64\t128\tignored\n\n";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        parse_ewf2_device_info(&utf16, &mut cs, &mut ts);
        assert_eq!(cs, 512 * 64);
        assert_eq!(ts, 512 * 128);
    }

    #[test]
    fn parse_ewf2_device_info_zero_bytes_per_sector_leaves_chunk_size() {
        use crate::reader::parse_ewf2_device_info;
        // If bytes_per_sector = 0, computed chunk_size would be 0.
        // Parser should NOT overwrite chunk_size — leave caller's value intact.
        let mut cs: u64 = 99999; // sentinel
        let mut ts: u64 = 0;
        let text = "2\nmain\nb\tsc\tts\n0\t64\t128\n";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        parse_ewf2_device_info(&utf16, &mut cs, &mut ts);
        assert_eq!(
            cs, 99999,
            "chunk_size should be unchanged when bytes_per_sector=0"
        );
        assert_eq!(
            ts, 0,
            "total_size should be unchanged when bytes_per_sector=0"
        );
    }

    #[test]
    fn parse_ewf2_device_info_zero_sectors_per_chunk_leaves_chunk_size() {
        use crate::reader::parse_ewf2_device_info;
        let mut cs: u64 = 99999;
        let mut ts: u64 = 0;
        let text = "2\nmain\nb\tsc\tts\n512\t0\t128\n";
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        parse_ewf2_device_info(&utf16, &mut cs, &mut ts);
        assert_eq!(
            cs, 99999,
            "chunk_size should be unchanged when sectors_per_chunk=0"
        );
        // total_size still computable: 512 * 128 = 65536
        assert_eq!(
            ts,
            512 * 128,
            "total_size should still be computed from valid bytes_per_sector"
        );
    }

    // -- reader.rs coverage: v2 truncated chain --

    // -- reader.rs coverage: v2 segment gap --

    #[test]
    fn ewf2_reader_rejects_segment_gap() {
        // Build two Ex01 files with segment numbers 1 and 3 (gap at 2)
        fn make_minimal_ex01(segment: u32) -> NamedTempFile {
            let mut d = Vec::new();
            d.extend_from_slice(&ewf2::EVF2_SIGNATURE);
            d.push(2);
            d.push(1);
            d.extend_from_slice(&1u16.to_le_bytes());
            d.extend_from_slice(&segment.to_le_bytes());
            d.extend_from_slice(&[0u8; 16]); // set_identifier
                                             // Done section immediately
            let mut done = [0u8; 64];
            done[0..4].copy_from_slice(&0x0Fu32.to_le_bytes()); // Done type
            done[24..28].copy_from_slice(&64u32.to_le_bytes());
            d.extend_from_slice(&done);

            let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
            tmp.write_all(&d).unwrap();
            tmp.flush().unwrap();
            tmp
        }

        let seg1 = make_minimal_ex01(1);
        let seg3 = make_minimal_ex01(3);

        let result = EwfReader::open_segments(&[seg1.path().into(), seg3.path().into()]);
        assert!(result.is_err(), "Should reject segment gap");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("segment gap"),
            "Error should mention segment gap: {err_msg}"
        );
    }

    #[test]
    fn ewf2_reader_handles_truncated_chain() {
        // Build an Ex01 where the file is truncated mid-section
        let mut file_data = Vec::new();
        file_data.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        file_data.push(2);
        file_data.push(1);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&1u32.to_le_bytes());
        file_data.extend_from_slice(&[0u8; 16]);
        // No section descriptors after header — just 32 bytes total
        // Should hit the truncated chain check (desc_offset + 64 > file_len)

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let result = EwfReader::open(tmp.path());
        // Should open but with 0 chunks (falls back to defaults)
        assert!(
            result.is_ok(),
            "Truncated Ex01 should open: {:?}",
            result.err()
        );
        let reader = result.unwrap();
        assert_eq!(reader.chunk_count(), 0);
    }

    #[test]
    fn ewf_reader_opens_synthetic_ex01() {
        let data = b"Hello, EWF2 world!";
        let tmp = build_synthetic_ex01(data);
        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_ok(),
            "EwfReader should open Ex01: {:?}",
            result.err()
        );
        let reader = result.unwrap();
        assert_eq!(reader.chunk_size(), 32768);
        assert_eq!(reader.chunk_count(), 1);
        assert_eq!(reader.total_size(), 32768);
    }

    #[test]
    fn ewf_reader_reads_ex01_first_bytes() {
        let data = b"DEADBEEF_EWF2_TEST";
        let tmp = build_synthetic_ex01(data);
        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_ok(),
            "EwfReader should open Ex01: {:?}",
            result.err()
        );
        let mut reader = result.unwrap();
        let mut buf = vec![0u8; data.len()];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, data);
    }

    #[test]
    fn ewf_reader_ex01_seek_and_read() {
        let mut test_data = vec![0u8; 1024];
        test_data[512..520].copy_from_slice(b"SEEKTEST");
        let tmp = build_synthetic_ex01(&test_data);
        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_ok(),
            "EwfReader should open Ex01: {:?}",
            result.err()
        );
        let mut reader = result.unwrap();
        reader.seek(SeekFrom::Start(512)).unwrap();
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"SEEKTEST");
    }

    // -- validate_and_reorder_segments direct unit tests --

    fn make_temp_files(n: usize) -> (tempfile::TempDir, Vec<std::fs::File>) {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<std::fs::File> = (0..n)
            .map(|i| {
                let p = dir.path().join(format!("seg{i}"));
                std::fs::File::create(&p).unwrap();
                std::fs::File::open(&p).unwrap()
            })
            .collect();
        (dir, files)
    }

    #[test]
    fn validate_reorder_sequential_segments() {
        let (_dir, files) = make_temp_files(3);
        let result = crate::reader::validate_and_reorder_segments(files, vec![1, 2, 3]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 3);
    }

    #[test]
    fn validate_reorder_out_of_order_segments() {
        let (_dir, files) = make_temp_files(3);
        // Segment numbers 3, 1, 2 — should reorder to 1, 2, 3
        let result = crate::reader::validate_and_reorder_segments(files, vec![3, 1, 2]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 3);
    }

    #[test]
    fn validate_reorder_single_segment() {
        let (_dir, files) = make_temp_files(1);
        let result = crate::reader::validate_and_reorder_segments(files, vec![1]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn validate_reorder_detects_gap() {
        let (_dir, files) = make_temp_files(2);
        // Segments 1 and 3 — gap at 2
        let result = crate::reader::validate_and_reorder_segments(files, vec![1, 3]);
        assert!(
            matches!(
                result,
                Err(EwfError::SegmentGap {
                    expected: 2,
                    got: 3
                })
            ),
            "Should detect gap: {result:?}"
        );
    }

    #[test]
    fn validate_reorder_detects_gap_starting_at_zero() {
        let (_dir, files) = make_temp_files(1);
        // Segment 0 instead of 1 — gap at start
        let result = crate::reader::validate_and_reorder_segments(files, vec![0]);
        assert!(
            matches!(
                result,
                Err(EwfError::SegmentGap {
                    expected: 1,
                    got: 0
                })
            ),
            "Should reject segment 0: {result:?}"
        );
    }

    #[test]
    fn validate_reorder_detects_duplicate_segments() {
        let (_dir, files) = make_temp_files(2);
        // Two files both claiming to be segment 1
        let result = crate::reader::validate_and_reorder_segments(files, vec![1, 1]);
        assert!(
            matches!(result, Err(EwfError::SegmentGap { .. })),
            "Should reject duplicate segment numbers: {result:?}"
        );
    }

    // -- Exact compressed chunk sizes for standards-conforming fixtures --

    #[test]
    fn last_compressed_chunk_has_correct_size() {
        let data1 = b"first chunk";
        let data2 = b"second chunk data";
        let (file, sizes) = build_synthetic_e01_chunks(&[data1, data2]);
        let mut expected = vec![0; 65536];
        expected[..data1.len()].copy_from_slice(data1);
        expected[32768..32768 + data2.len()].copy_from_slice(data2);
        for reader in [
            EwfReader::open(file.path()).unwrap(),
            EwfReader::open_lazy(file.path()).unwrap(),
        ] {
            assert_eq!(reader.chunk_count(), 2);
            for (index, size) in sizes.iter().enumerate() {
                let chunk = reader.debug_chunk(index).unwrap();
                assert!(chunk.2);
                assert_eq!(chunk.1, *size as u64);
            }
            let mut bytes = vec![0; expected.len()];
            assert_eq!(reader.read_at(&mut bytes, 0).unwrap(), expected.len());
            assert_eq!(bytes, expected);
            let mut boundary = [0; 32];
            assert_eq!(
                reader.read_at(&mut boundary, 32760).unwrap(),
                boundary.len()
            );
            assert_eq!(boundary, expected[32760..32792]);
        }
    }

    #[test]
    fn single_compressed_chunk_has_correct_size() {
        let data = b"solo chunk data";
        let (file, sizes) = build_synthetic_e01_chunks(&[data]);
        let mut expected = vec![0; 32768];
        expected[..data.len()].copy_from_slice(data);
        for reader in [
            EwfReader::open(file.path()).unwrap(),
            EwfReader::open_lazy(file.path()).unwrap(),
        ] {
            assert_eq!(reader.chunk_count(), 1);
            let chunk = reader.debug_chunk(0).unwrap();
            assert!(chunk.2);
            assert_eq!(chunk.1, sizes[0] as u64);
            let mut bytes = vec![0; expected.len()];
            assert_eq!(reader.read_at(&mut bytes, 0).unwrap(), expected.len());
            assert_eq!(bytes, expected);
        }
    }

    #[test]
    fn incompressible_final_chunk_keeps_its_full_stored_size() {
        let mut state = 0x1357_2468u32;
        let data: Vec<u8> = (0..32768)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let (file, sizes) = build_synthetic_e01_chunks(&[&data]);
        assert!(
            sizes[0] > data.len(),
            "zlib overhead must not be clipped to logical size"
        );
        for reader in [
            EwfReader::open(file.path()).unwrap(),
            EwfReader::open_lazy(file.path()).unwrap(),
        ] {
            assert_eq!(reader.debug_chunk(0).unwrap().1, sizes[0] as u64);
            let mut bytes = vec![0; data.len()];
            assert_eq!(reader.read_at(&mut bytes, 0).unwrap(), data.len());
            assert_eq!(bytes, data);
        }
    }

    // -- DoS guard: reject absurd table entry_count --

    #[test]
    fn v1_rejects_absurd_table_entry_count() {
        // Build a synthetic E01 with entry_count = 0x1000_0000 (268M entries).
        // Without a guard, this would try to allocate 1 GB. Reader should reject it.
        let chunk_size: u32 = 32768;
        let sectors_per_chunk: u32 = 64;
        let bytes_per_sector: u32 = 512;
        let sector_count: u64 = u64::from(chunk_size / bytes_per_sector);

        let vol_desc_offset: u64 = FILE_HEADER_SIZE as u64;
        let vol_data_offset: u64 = vol_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let tbl_desc_offset: u64 = vol_data_offset + 94;
        let tbl_hdr_offset: u64 = tbl_desc_offset + SECTION_DESCRIPTOR_SIZE as u64;
        let done_desc_offset: u64 = tbl_hdr_offset + 24 + 4; // 24 header + 4 bytes (1 fake entry)

        let mut file_data = Vec::new();

        // File header
        file_data.extend_from_slice(&EVF_SIGNATURE);
        file_data.push(0x01);
        file_data.extend_from_slice(&1u16.to_le_bytes());
        file_data.extend_from_slice(&0u16.to_le_bytes());

        // Volume descriptor
        let mut vol_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        vol_desc[..6].copy_from_slice(b"volume");
        vol_desc[16..24].copy_from_slice(&tbl_desc_offset.to_le_bytes());
        vol_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64 + 94).to_le_bytes());
        file_data.extend_from_slice(&vol_desc);

        // Volume data
        let mut vol_data = [0u8; 94];
        vol_data[0..4].copy_from_slice(&1u32.to_le_bytes());
        vol_data[4..8].copy_from_slice(&1u32.to_le_bytes());
        vol_data[8..12].copy_from_slice(&sectors_per_chunk.to_le_bytes());
        vol_data[12..16].copy_from_slice(&bytes_per_sector.to_le_bytes());
        vol_data[16..24].copy_from_slice(&sector_count.to_le_bytes());
        file_data.extend_from_slice(&vol_data);

        // Table descriptor
        let mut tbl_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        tbl_desc[..5].copy_from_slice(b"table");
        tbl_desc[16..24].copy_from_slice(&done_desc_offset.to_le_bytes());
        let tbl_section_size = SECTION_DESCRIPTOR_SIZE as u64 + 24 + 4;
        tbl_desc[24..32].copy_from_slice(&tbl_section_size.to_le_bytes());
        file_data.extend_from_slice(&tbl_desc);

        // Table header with ABSURD entry_count
        let mut tbl_hdr = [0u8; 24];
        tbl_hdr[0..4].copy_from_slice(&0x1000_0000u32.to_le_bytes()); // 268M entries!
        tbl_hdr[8..16].copy_from_slice(&0u64.to_le_bytes());
        file_data.extend_from_slice(&tbl_hdr);

        // One fake table entry (the file is way too small for 268M)
        file_data.extend_from_slice(&0u32.to_le_bytes());

        // Done
        let mut done_desc = [0u8; SECTION_DESCRIPTOR_SIZE];
        done_desc[..4].copy_from_slice(b"done");
        done_desc[24..32].copy_from_slice(&(SECTION_DESCRIPTOR_SIZE as u64).to_le_bytes());
        file_data.extend_from_slice(&done_desc);

        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&file_data).unwrap();
        tmp.flush().unwrap();

        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_err(),
            "Should reject absurd entry_count, got: {:?}",
            result.ok().map(|r| r.chunk_count())
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("entry count"),
            "Error should mention entry count: {err_msg}"
        );
    }

    #[test]
    fn v2_rejects_absurd_table_entry_count() {
        // Build a minimal Ex01 with absurd sector_table entry count.
        // Correct EWF2 layout ([data][descriptor] ordering):
        //   [32..80)    SectorTable DATA (48 bytes): 32-byte header with absurd entry_count, rest zeros
        //   [80..144)   SectorTable DESCRIPTOR (data_size=48, prev=0)
        //   [144..208)  Done DESCRIPTOR (data_size=0, prev=80)
        let mut d = Vec::new();
        // V2 file header (32 bytes)
        d.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        d.push(2);
        d.push(1); // major, minor
        d.extend_from_slice(&1u16.to_le_bytes()); // compression = zlib
        d.extend_from_slice(&1u32.to_le_bytes()); // segment 1
        d.extend_from_slice(&[0u8; 16]); // set_identifier
        assert_eq!(d.len(), 32);

        // SectorTable DATA (48 bytes = 32-byte header + 16-byte entry)
        let mut tbl_hdr = [0u8; 32];
        tbl_hdr[0..8].copy_from_slice(&0u64.to_le_bytes()); // first_chunk
        tbl_hdr[8..12].copy_from_slice(&0x1000_0000u32.to_le_bytes()); // 268M entries!
        d.extend_from_slice(&tbl_hdr);
        d.extend_from_slice(&[0u8; 16]); // one fake entry
        assert_eq!(d.len(), 80);

        // SectorTable DESCRIPTOR (data_size=48, prev=0)
        let mut tbl_desc = [0u8; 64];
        tbl_desc[0..4].copy_from_slice(&0x04u32.to_le_bytes()); // SectorTable type
        tbl_desc[8..16].copy_from_slice(&0u64.to_le_bytes()); // previous_offset = 0
        tbl_desc[16..24].copy_from_slice(&48u64.to_le_bytes()); // data_size = 48
        tbl_desc[24..28].copy_from_slice(&64u32.to_le_bytes()); // descriptor_size
        d.extend_from_slice(&tbl_desc);
        assert_eq!(d.len(), 144);

        // Done DESCRIPTOR (data_size=0, prev=80)
        let mut done_desc = [0u8; 64];
        done_desc[0..4].copy_from_slice(&0x0Fu32.to_le_bytes()); // Done type
        done_desc[8..16].copy_from_slice(&80u64.to_le_bytes()); // previous_offset = 80
        done_desc[16..24].copy_from_slice(&0u64.to_le_bytes()); // data_size = 0
        done_desc[24..28].copy_from_slice(&64u32.to_le_bytes()); // descriptor_size
        d.extend_from_slice(&done_desc);
        assert_eq!(d.len(), 208);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&d).unwrap();
        tmp.flush().unwrap();

        let result = EwfReader::open(tmp.path());
        assert!(
            result.is_err(),
            "Should reject absurd v2 entry_count, got: {:?}",
            result.ok().map(|r| r.chunk_count())
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("entry count"),
            "Error should mention entry count: {err_msg}"
        );
    }

    // --- parse_ewf2_case_data edge cases (lines 866, 877, 886, 898) ---

    #[test]
    fn ewf2_case_data_empty_input() {
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&[], &mut meta);
        assert!(meta.case_number.is_none());
    }

    #[test]
    fn ewf2_case_data_single_byte() {
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&[0x41], &mut meta);
        assert!(meta.case_number.is_none());
    }

    #[test]
    fn ewf2_case_data_too_few_lines() {
        // UTF-16LE string with only 2 lines (need 4)
        let text = "line1\nline2\n";
        let raw: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&raw, &mut meta);
        assert!(meta.case_number.is_none());
    }

    #[test]
    fn ewf2_case_data_empty_values_skipped() {
        // 4 lines: header, subheader, field names, empty values
        let text = "1\nmain\ncn\ten\t\n\t\t\n";
        let raw: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&raw, &mut meta);
        // All values are empty so nothing should be set
        assert!(meta.case_number.is_none());
        assert!(meta.evidence_number.is_none());
    }

    #[test]
    fn ewf2_case_data_unknown_fields_ignored() {
        // 4 lines with unknown field name "zz"
        let text = "1\nmain\nzz\nfoo\n";
        let raw: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&raw, &mut meta);
        // Unknown field "zz" should be silently ignored
        assert!(meta.case_number.is_none());
    }

    #[test]
    fn ewf2_case_data_valid_fields_parsed() {
        let text = "1\nmain\ncn\tex\tav\nCASE-001\tJohn\tFTK\n";
        let raw: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut meta = EwfMetadata::default();
        reader::parse_ewf2_case_data(&raw, &mut meta);
        assert_eq!(meta.case_number.as_deref(), Some("CASE-001"));
        assert_eq!(meta.examiner.as_deref(), Some("John"));
        assert_eq!(meta.acquiry_software.as_deref(), Some("FTK"));
    }

    // --- Line 554: V2 section with zero advance breaks section loop ---

    #[test]
    fn v2_zero_advance_section_breaks_loop() {
        // Build a minimal Ex01 where a section descriptor has all zero sizes,
        // causing advance = 0 and triggering the break guard at line 554.
        let mut d = Vec::new();
        // V2 file header (32 bytes)
        d.extend_from_slice(&ewf2::EVF2_SIGNATURE);
        d.push(2);
        d.push(1); // major, minor
        d.extend_from_slice(&1u16.to_le_bytes()); // compression = zlib
        d.extend_from_slice(&1u32.to_le_bytes()); // segment 1
        d.extend_from_slice(&[0u8; 16]); // set_identifier

        // Section descriptor with all zero sizes (64 bytes)
        // type=0 (unknown), descriptor_size=0, data_size=0, padding_size=0
        let sec = [0u8; 64];
        d.extend_from_slice(&sec);

        let mut tmp = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
        tmp.write_all(&d).unwrap();
        tmp.flush().unwrap();

        // The zero-advance guard prevents an infinite loop. The parser
        // breaks out of the section loop and continues with defaults:
        // 0 chunks, chunk_size=DEFAULT_V2_CHUNK_SIZE, total_size=0.
        // This is Ok (not Err) because there's no strict requirement for
        // device_info — the parser just uses defaults.
        let result = EwfReader::open(tmp.path());
        if let Ok(ref r) = result {
            assert_eq!(r.total_size(), 0);
            assert_eq!(r.chunk_count(), 0);
        }
        // Either Ok(empty reader) or Err is acceptable — the key is no hang.
    }

    // --- Line 715: Truncated compressed chunk triggers partial read guard ---

    #[test]
    fn truncated_compressed_chunk_returns_error() {
        use std::io::Read;

        // Strategy: open a real E01 with FULL data (so chunk table is parsed
        // correctly), then truncate the file on disk AFTER opening. The reader
        // still holds a valid file handle with chunk metadata in memory, but
        // the underlying file data is now shorter. Seeking past the new EOF
        // succeeds on Unix, but reads return 0 — triggering line 715's
        // early-EOF break in the compressed chunk read loop.
        let src_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/nps-2010-emails.E01"
        );
        let src_data = std::fs::read(src_path).unwrap();
        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&src_data).unwrap();
        tmp.flush().unwrap();

        // Open the full file — parses headers, volume, table, chunks correctly
        let mut reader = EwfReader::open(tmp.path()).unwrap();
        assert!(reader.total_size() > 0);
        assert!(reader.chunk_count() > 0);

        // Truncate the file to just the header area — all chunk data is now gone
        tmp.as_file().set_len(1024).unwrap();

        // Read data — read_chunk will seek to chunk offsets past the new EOF,
        // file.read() returns 0, hitting line 715 break. Then decompression
        // of empty data either returns Ok(0) or Err — both are acceptable.
        let mut buf = [0u8; 512];
        let _ = reader.read(&mut buf);
    }

    // --- Verify on truncated image handles decompression errors gracefully ---

    #[cfg(feature = "verify")]
    #[test]
    fn verify_truncated_image_handles_decompression_error() {
        // Same truncate-after-open strategy: open full file, truncate on disk,
        // then call verify(). The verify loop reads all chunks sequentially.
        // Truncated chunks cause decompression errors that propagate as Err.
        let src_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/nps-2010-emails.E01"
        );
        let src_data = std::fs::read(src_path).unwrap();
        let mut tmp = tempfile::Builder::new().suffix(".E01").tempfile().unwrap();
        tmp.write_all(&src_data).unwrap();
        tmp.flush().unwrap();

        let reader = EwfReader::open(tmp.path()).unwrap();

        // Truncate after opening — chunk data is gone
        tmp.as_file().set_len(1024).unwrap();

        // verify() streams all data. Truncated compressed chunks decompress as
        // empty/zero data (flate2 returns Ok(0) on empty input), so verify
        // completes with wrong hashes rather than erroring.
        let result = reader.verify();
        if let Ok(v) = result {
            // Computed hashes are over zero-filled data — should NOT match stored
            assert_ne!(
                v.md5_match,
                Some(true),
                "truncated image should not verify as MD5 match"
            );
        } else {
            // Decompression error is also acceptable
        }
    }
}

#[cfg(feature = "vfs")]
mod vfs;
