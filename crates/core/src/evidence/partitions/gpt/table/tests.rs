use super::*;
use crate::evidence::partitions::gpt::GptPartitionReader;
use crate::evidence::partitions::{
    detect_partitions_with_meta, test_support::BufSource, TableScheme,
};
use std::io::{Read, Seek, SeekFrom};

// Independent bitwise IEEE CRC32 fixture encoder, separate from production's flate2.
fn fixture_crc(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn checksum_header(bytes: &mut [u8], offset: usize) {
    let size = u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as usize;
    put32(bytes, offset + 16, 0);
    let crc = fixture_crc(&bytes[offset..offset + size]);
    put32(bytes, offset + 16, crc);
}

pub(crate) fn fixture(sector: usize, count: usize, stride: usize, populated: usize) -> Vec<u8> {
    let mut bytes = vec![0; 1024 * sector];
    let array_sectors = (count * stride).div_ceil(sector);
    let backup_array = 1023 - array_sectors;
    for index in 0..populated {
        let offset = 2 * sector + index * stride;
        bytes[offset..offset + 16].copy_from_slice(&gpt::partition_types::BASIC.guid.to_bytes_le());
        let unique = Uuid::from_u128(0xaabbccdd_1122_3344_5566_000000000001 + index as u128);
        bytes[offset + 16..offset + 32].copy_from_slice(&unique.to_bytes_le());
        put64(&mut bytes, offset + 32, 200 + index as u64);
        put64(&mut bytes, offset + 40, 200 + index as u64); // Inclusive, single sector.
        put64(&mut bytes, offset + 48, 0x8000_0000_0000_0005);
        for (unit_index, unit) in "证据😀".encode_utf16().enumerate() {
            bytes[offset + 56 + unit_index * 2..offset + 58 + unit_index * 2]
                .copy_from_slice(&unit.to_le_bytes());
        }
    }
    let array = bytes[2 * sector..2 * sector + count * stride].to_vec();
    bytes[backup_array * sector..backup_array * sector + array.len()].copy_from_slice(&array);
    for (lba, other, entries_lba) in [(1, 1023, 2), (1023, 1, backup_array)] {
        let offset = lba * sector;
        bytes[offset..offset + 8].copy_from_slice(b"EFI PART");
        put32(&mut bytes, offset + 8, 0x0001_0000);
        put32(&mut bytes, offset + 12, 92);
        put64(&mut bytes, offset + 24, lba as u64);
        put64(&mut bytes, offset + 32, other as u64);
        put64(&mut bytes, offset + 40, (2 + array_sectors) as u64);
        put64(&mut bytes, offset + 48, (backup_array - 1) as u64);
        bytes[offset + 56..offset + 72].copy_from_slice(
            &Uuid::from_u128(0x12345678_1122_3344_5566_778899aabbcc).to_bytes_le(),
        );
        put64(&mut bytes, offset + 72, entries_lba as u64);
        put32(&mut bytes, offset + 80, count as u32);
        put32(&mut bytes, offset + 84, stride as u32);
        put32(&mut bytes, offset + 88, fixture_crc(&array));
        checksum_header(&mut bytes, offset);
    }
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    bytes[450] = 0xee;
    bytes[454..458].copy_from_slice(&1u32.to_le_bytes());
    bytes[458..462].copy_from_slice(&1023u32.to_le_bytes());
    bytes
}

fn table(bytes: Vec<u8>, sector: usize) -> GptTable {
    GptTable::read(&BufSource(bytes), sector).unwrap().unwrap()
}

#[test]
fn ieee_checksum_known_answer() {
    assert_eq!(fixture_crc(b"123456789"), 0xcbf43926);
    assert_eq!(crc32(b"123456789"), 0xcbf43926);
}

#[test]
fn all_sector_sizes_preserve_metadata_and_single_sector_partitions() {
    for sector in [512, 1024, 2048, 4096] {
        let bytes = fixture(sector, 4, 128, 1);
        let source = BufSource(bytes.clone());
        let table = table(bytes, sector);
        assert_eq!(infer_sector_size(&source, 512).unwrap(), sector);
        assert!(table.header().verified);
        assert_eq!(table.headers.len(), 2);
        assert!(table.warnings.is_empty(), "{:?}", table.warnings);
        let entry = &table.entries[0];
        assert_eq!(entry.type_guid, gpt::partition_types::BASIC.guid);
        assert_eq!(entry.entry_offset, (sector * 2) as u64);
        assert_eq!(entry.entry_index, 0);
        assert_eq!(entry.attributes, 0x8000_0000_0000_0005);
        assert_eq!(entry.name().unwrap(), "证据😀");
        let guid = entry.unique_guid;
        assert_eq!(table.partition_by_guid(guid).unwrap(), Some(entry));
        assert!(table.partition_by_guid(Uuid::nil()).unwrap().is_none());
        assert!(table
            .partition_by_guid(Uuid::from_u128(123))
            .unwrap()
            .is_none());
        let projected = table.project().unwrap();
        assert_eq!(projected[0].length_lba, 1);
        assert_eq!(projected[0].unique_guid, Some(*guid.as_bytes()));
    }
}

#[test]
fn verified_backup_wins_over_primary_header_or_array_crc_corruption() {
    for offset in [512 + 16, 1024 + 56] {
        let mut bytes = fixture(512, 4, 128, 1);
        bytes[offset] ^= 1;
        let result = table(bytes.clone(), 512);
        assert_eq!(result.header().read_lba, 1023);
        assert!(result.header().verified);
        let detected = detect_partitions_with_meta(&BufSource(bytes), 512).unwrap();
        assert_eq!(detected.scheme, Some(TableScheme::Gpt));
        assert_eq!(detected.disk_id, result.disk_id());
    }
}

#[test]
fn backup_disk_identity_comes_from_selected_copy() {
    let mut bytes = fixture(512, 4, 128, 1);
    bytes[512 + 56..512 + 72].fill(0x11); // Primary checksum now invalid.
    let result = table(bytes.clone(), 512);
    assert_eq!(
        result.disk_id().as_deref(),
        Some("12345678-1122-3344-5566-778899AABBCC")
    );
    assert_eq!(
        detect_partitions_with_meta(&BufSource(bytes), 512)
            .unwrap()
            .disk_id,
        result.disk_id()
    );
}

#[test]
fn recovers_wiped_primary_and_pointer_backup_before_resized_tail() {
    let mut bytes = fixture(512, 4, 128, 1);
    bytes.resize(1536 * 512, 0); // Backup remains at old last LBA, referenced by primary.
    assert_eq!(table(bytes.clone(), 512).headers.len(), 2);
    bytes[512 + 16] ^= 1;
    assert_eq!(table(bytes, 512).header().read_lba, 1023);
    let mut bytes = fixture(512, 4, 128, 1);
    bytes[512..1024].fill(0);
    assert_eq!(table(bytes, 512).header().read_lba, 1023);
}

#[test]
fn primary_backup_disagreement_is_auditable_and_not_merged() {
    let mut bytes = fixture(512, 4, 128, 1);
    bytes[1023 * 512 + 56] ^= 1;
    checksum_header(&mut bytes, 1023 * 512);
    let result = table(bytes, 512);
    assert_eq!(result.header().read_lba, 1);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("metadata disagree")));
}

#[test]
fn larger_entries_use_declared_stride_and_fixed_utf16_name() {
    for stride in [256, 512] {
        let mut bytes = fixture(512, 4, stride, 2);
        // Extension bytes are opaque but must be covered by the array CRC.
        bytes[1024 + 128..1024 + stride].fill(0xa5);
        let result = table(bytes, 512); // Valid backup chosen, correct 256/512 stride.
        assert!(result.header().verified);
        assert_eq!(result.entries[1].first_lba, 201);
        assert_eq!(result.entries[1].name().unwrap(), "证据😀");
    }
}

#[test]
fn extended_header_crc_includes_extension_bytes() {
    let mut bytes = fixture(512, 4, 128, 1);
    put32(&mut bytes, 512 + 12, 128);
    bytes[512 + 92..512 + 128].fill(0x5a);
    checksum_header(&mut bytes, 512);
    assert!(table(bytes.clone(), 512).header().verified);
    bytes[512 + 100] ^= 1;
    assert_eq!(table(bytes, 512).header().read_lba, 1023);
}

#[test]
fn valid_empty_gpt_is_recognized_even_at_large_sector_sizes() {
    for sector in [512, 1024, 2048, 4096] {
        let bytes = fixture(sector, 4, 128, 0);
        let source = BufSource(bytes);
        assert_eq!(infer_sector_size(&source, 512).unwrap(), sector);
        let result = detect_partitions_with_meta(&source, sector).unwrap();
        assert_eq!(result.scheme, Some(TableScheme::Gpt));
        assert!(result.partitions.is_empty());
    }
    assert!(table(fixture(512, 0, 128, 0), 512)
        .project()
        .unwrap()
        .is_empty());
}

#[test]
fn rejected_complete_ranges_keep_raw_entries() {
    for (start, end) in [
        (200, 1024),
        (200, u64::MAX),
        (0, 200),
        (200, 199),
        (2, 200),
        (200, 1022),
    ] {
        let mut bytes = fixture(512, 4, 128, 1);
        // Remove backup so primary damage exercises unverified recovery.
        bytes[1023 * 512..].fill(0);
        put64(&mut bytes, 1024 + 32, start);
        put64(&mut bytes, 1024 + 40, end);
        // Also include a valid entry to keep the table recoverable.
        bytes[1152..1280].copy_from_slice(&fixture(512, 4, 128, 2)[1152..1280]);
        let result = table(bytes, 512);
        assert!(!result.entries[0].valid, "range {start}..={end}");
        assert_eq!(result.project().unwrap().len(), 1);
        assert!(
            GptPartitionReader::new(&BufSource(vec![0; 1024 * 512]), &result.entries[0], 512)
                .is_err()
        );
    }
}

#[test]
fn sparse_high_indexes_and_256_partition_id_boundary() {
    let mut bytes = fixture(512, 300, 128, 1);
    let first = bytes[1024..1152].to_vec();
    bytes[1024..1152].fill(0);
    bytes[1024 + 299 * 128..1024 + 300 * 128].copy_from_slice(&first);
    bytes[1023 * 512..].fill(0);
    let result = table(bytes, 512);
    assert_eq!(result.entries[0].entry_index, 299);
    assert_eq!(result.partition_count(), 1);
    assert_eq!(
        result.partition_by_index(0),
        result.partition_by_entry_index(299)
    );
    assert!(result.partition_by_index(1).is_none());
    assert!(result.partition_by_entry_index(0).is_none());
    assert_eq!(result.project().unwrap()[0].id, 0);
    assert_eq!(
        table(fixture(512, 300, 128, 256), 512).project().unwrap()[255].id,
        255
    );
    let bytes = fixture(512, 300, 128, 257);
    let result = table(bytes.clone(), 512);
    assert!(result.project().unwrap_err().to_string().contains("256"));
    assert!(detect_partitions_with_meta(&BufSource(bytes), 512).is_err());
}

#[test]
fn duplicate_guid_overlap_and_invalid_utf16_are_preserved_and_reported() {
    let mut bytes = fixture(512, 4, 128, 2);
    bytes[1023 * 512..].fill(0);
    let guid = bytes[1040..1056].to_vec();
    bytes[1152 + 16..1152 + 32].copy_from_slice(&guid);
    put64(&mut bytes, 1152 + 32, 200);
    put64(&mut bytes, 1152 + 40, 200);
    bytes[1152 + 56..1152 + 60].copy_from_slice(&[0, 0xd8, 0, 0]);
    let result = table(bytes, 512);
    assert!(result
        .partition_by_guid(result.entries[0].unique_guid)
        .is_err());
    assert!(result.entries[1].name().is_err());
    assert_eq!(result.project().unwrap().len(), 2);
    for text in ["duplicate", "overlapping", "UTF-16"] {
        assert!(result.warnings.iter().any(|warning| warning.contains(text)));
    }
}

#[test]
fn oversized_truncated_or_overlapping_arrays_are_never_partially_parsed() {
    for (count, stride, array_lba) in [
        (u32::MAX, 128, 2),
        (4, 64, 2),
        (4, 129, 2),
        (4, 128, 1023),
        (4, 128, 1),
        (4, 128, u64::MAX),
    ] {
        let mut bytes = fixture(512, 4, 128, 1);
        bytes[1023 * 512..].fill(0);
        put32(&mut bytes, 512 + 80, count);
        put32(&mut bytes, 512 + 84, stride);
        put64(&mut bytes, 512 + 72, array_lba);
        assert!(GptTable::read(&BufSource(bytes), 512).unwrap().is_none());
    }
}

struct FaultSource {
    source: BufSource,
    offset: u64,
    short: bool,
    cancel: Option<CancellationToken>,
}
impl SectorSource for FaultSource {
    fn size(&self) -> u64 {
        self.source.size()
    }
    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        if offset == self.offset {
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            } else if self.short {
                return self.source.read(offset, length.saturating_sub(1));
            } else {
                anyhow::bail!("injected source failure");
            }
        }
        self.source.read(offset, length)
    }
}

#[test]
fn metadata_io_errors_and_short_reads_cannot_become_successful_recovery() {
    for offset in [512, 1024, 1023 * 512] {
        for short in [false, true] {
            let source = FaultSource {
                source: BufSource(fixture(512, 4, 128, 1)),
                offset,
                short,
                cancel: None,
            };
            assert!(GptTable::read(&source, 512).is_err());
        }
    }
}

#[test]
fn cancellation_before_and_during_array_read_is_terminal() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(GptTable::read_cancellable(&BufSource(fixture(512, 4, 128, 1)), 512, &cancel).is_err());
    let cancel = CancellationToken::new();
    let source = FaultSource {
        source: BufSource(fixture(512, 4, 128, 1)),
        offset: 1024,
        short: false,
        cancel: Some(cancel.clone()),
    };
    assert!(GptTable::read_cancellable(&source, 512, &cancel).is_err());
}

#[test]
fn reader_cursor_eof_range_and_seek_overflow() {
    let mut bytes = fixture(512, 4, 128, 1);
    for (index, byte) in bytes[200 * 512..201 * 512].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    let source = BufSource(bytes);
    let result = GptTable::read(&source, 512).unwrap().unwrap();
    let mut reader = GptPartitionReader::new(&source, &result.entries[0], 512).unwrap();
    assert_eq!(reader.volume_offset(), 200 * 512);
    assert_eq!(reader.size(), 512);
    assert_eq!(reader.read_at(510, 10).unwrap(), vec![8, 9]);
    assert_eq!(reader.stream_position().unwrap(), 0);
    let mut data = Vec::new();
    reader.read_to_end(&mut data).unwrap();
    assert_eq!(data, source.0[200 * 512..201 * 512]);
    assert_eq!(reader.seek(SeekFrom::End(-2)).unwrap(), 510);
    let mut tail = [0; 8];
    assert_eq!(reader.read(&mut tail).unwrap(), 2);
    assert_eq!(reader.read(&mut tail).unwrap(), 0);
    assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_ok());
    assert!(reader.seek(SeekFrom::Current(1)).is_err());
    assert_eq!(reader.stream_position().unwrap(), u64::MAX);
    assert!(reader.seek(SeekFrom::End(-513)).is_err());
    assert!(reader.read_at(u64::MAX, 10).unwrap().is_empty());
    let short = FaultSource {
        source,
        offset: 200 * 512,
        short: true,
        cancel: None,
    };
    let mut reader = GptPartitionReader::new(&short, &result.entries[0], 512).unwrap();
    assert!(reader.read(&mut tail).is_err());
    assert_eq!(reader.stream_position().unwrap(), 0);
}
