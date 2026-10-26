//! Independent byte-built fixtures, including nonstandard and damaged tables.

use super::*;
use crate::core::evidence::partitions::test_support::BufSource;
use std::io::{Read, Seek, SeekFrom};

fn record(bytes: &mut [u8], sector_size: usize, lba: usize) {
    let base = lba * sector_size;
    bytes[base + 510..base + 512].copy_from_slice(&[0x55, 0xaa]);
}

fn entry(
    bytes: &mut [u8],
    sector_size: usize,
    record_lba: usize,
    slot: usize,
    type_byte: u8,
    start: u32,
    length: u32,
) {
    let base = record_lba * sector_size + 446 + slot * 16;
    bytes[base + 4] = type_byte;
    bytes[base + 8..base + 12].copy_from_slice(&start.to_le_bytes());
    bytes[base + 12..base + 16].copy_from_slice(&length.to_le_bytes());
}

fn fixture(sector_size: usize) -> BufSource {
    let mut bytes = vec![0; 64 * sector_size];
    record(&mut bytes, sector_size, 0);
    bytes[440..444].copy_from_slice(&0x1234abcd_u32.to_le_bytes());
    entry(&mut bytes, sector_size, 0, 0, 0x17, 2, 4);
    entry(&mut bytes, sector_size, 0, 1, 0x0f, 16, 48);
    record(&mut bytes, sector_size, 16);
    entry(&mut bytes, sector_size, 16, 0, 0x01, 1, 4);
    entry(&mut bytes, sector_size, 16, 1, 0x0f, 16, 32);
    record(&mut bytes, sector_size, 32);
    entry(&mut bytes, sector_size, 32, 2, 0x1c, 2, 6);
    BufSource(bytes)
}

fn read_table(source: &dyn SectorSource, sector_size: usize) -> MbrTable {
    MbrTable::read(source, sector_size)
        .expect("read table")
        .expect("MBR table")
}

fn ranges(table: &MbrTable) -> Vec<(u8, PartitionKind, u64, u64)> {
    table
        .partitions
        .iter()
        .map(|entry| {
            let partition = &entry.partition;
            (
                partition.id,
                partition.kind,
                partition.start_lba,
                partition.length_lba,
            )
        })
        .collect()
}

#[test]
fn decodes_raw_flags_chs_and_entry_origin() {
    let mut source = fixture(512);
    source.0[446] = 0x80;
    source.0[447..450].copy_from_slice(&[254, 0xff, 0xff]);
    source.0[451..454].copy_from_slice(&[7, 0xc1, 2]);
    let table = read_table(&source, 512);
    let expected = MbrEntry {
        boot_indicator: 0x80,
        first_chs: [254, 0xff, 0xff],
        type_byte: 0x17,
        last_chs: [7, 0xc1, 2],
        start_lba: 2,
        length_lba: 4,
    };
    assert_eq!(
        table.partitions[0],
        MbrPartition {
            partition: Partition {
                id: 0,
                kind: PartitionKind::Ntfs,
                start_lba: 2,
                length_lba: 4,
                label: None,
                unique_guid: None,
                amiga_geometry: None,
            },
            entry: expected,
            record_lba: 0,
            slot: 0
        }
    );
    assert!(expected.is_bootable());
    assert_eq!(
        expected.start_chs(),
        ChsAddress {
            cylinder: 1023,
            head: 254,
            sector: 63
        }
    );
    assert_eq!(
        expected.end_chs(),
        ChsAddress {
            cylinder: 770,
            head: 7,
            sector: 1
        }
    );
    assert_eq!(
        table
            .records
            .iter()
            .map(|record| record.record_lba)
            .collect::<Vec<_>>(),
        vec![0, 16, 32]
    );
    assert_eq!(table.records[0].disk_signature, 0x1234abcd);
    assert_eq!(read_disk_signature(&source.0), Some("0x1234ABCD".into()));
    assert_eq!(table.records[2].entry_offset(2, 512), Some(32 * 512 + 478));
    assert_eq!(table.records[2].entry_offset(4, 512), None);
    assert_eq!(table.records[2].entry_offset(0, usize::MAX), None);
    assert_eq!(table.records[0].entries[1].type_byte, 0x0f);
    assert_eq!(table.records[0].entries[3].type_byte, 0);
    assert!(table.warnings.is_empty());
}

#[test]
fn supports_each_sector_size_and_relative_ebr_links() {
    for sector_size in [512, 1024, 2048, 4096] {
        let source = fixture(sector_size);
        assert_eq!(infer_sector_size(&source, 512).expect("infer"), sector_size);
        assert_eq!(
            ranges(&read_table(&source, sector_size)),
            vec![
                (0, PartitionKind::Ntfs, 2, 4),
                (1, PartitionKind::Fat12, 17, 4),
                (2, PartitionKind::Fat32, 34, 6)
            ]
        );
    }
}

#[test]
fn primary_only_geometry_is_not_guessed() {
    let mut source = fixture(4096);
    source.0[462..478].fill(0);
    assert_eq!(infer_sector_size(&source, 512).expect("infer"), 512);
    assert_eq!(infer_sector_size(&source, 4096).expect("infer"), 4096);
    for invalid in [0, 1, 256, 768, 8192] {
        assert!(MbrTable::read(&source, invalid).is_err());
        assert!(infer_sector_size(&source, invalid).is_err());
    }
}

#[test]
fn supports_each_extended_type_and_multiple_roots_and_branches() {
    for type_byte in [0x05, 0x0f, 0x85] {
        let mut source = fixture(512);
        source.0[466] = type_byte;
        source.0[16 * 512 + 466] = type_byte;
        // An independent second extended root lives before the first one.
        entry(&mut source.0, 512, 0, 3, type_byte, 8, 8);
        record(&mut source.0, 512, 8);
        entry(&mut source.0, 512, 8, 0, 0x83, 1, 2);
        // The first EBR branches to two records; slots are visited in order.
        entry(&mut source.0, 512, 16, 3, type_byte, 32, 16);
        record(&mut source.0, 512, 48);
        entry(&mut source.0, 512, 48, 0, 0x83, 1, 2);
        assert_eq!(
            ranges(&read_table(&source, 512)),
            vec![
                (0, PartitionKind::Ntfs, 2, 4),
                (1, PartitionKind::Fat12, 17, 4),
                (2, PartitionKind::Fat32, 34, 6),
                (3, PartitionKind::Unknown, 49, 2),
                (4, PartitionKind::Unknown, 9, 2)
            ]
        );
    }
}

#[test]
fn cycles_and_duplicate_links_do_not_duplicate_partitions() {
    let mut source = fixture(512);
    entry(&mut source.0, 512, 16, 3, 0x05, 16, 32);
    entry(&mut source.0, 512, 32, 1, 0x85, 0, 48);
    let table = read_table(&source, 512);
    assert_eq!(table.partitions.len(), 3);
    assert_eq!(table.records.len(), 3);
    assert_eq!(table.warnings.len(), 2);
    assert!(table
        .warnings
        .iter()
        .all(|warning| warning.contains("duplicate")));
}

#[test]
fn rejects_full_ranges_outside_container_or_image() {
    let mut source = fixture(512);
    entry(&mut source.0, 512, 0, 0, 0x17, 2, 63);
    // In-image but outside the extended container's end.
    entry(&mut source.0, 512, 0, 1, 0x05, 16, 32);
    entry(&mut source.0, 512, 16, 0, 0x01, 30, 4);
    entry(&mut source.0, 512, 16, 1, 0x05, 32, 16);
    let table = read_table(&source, 512);
    assert!(table.partitions.is_empty());
    assert_eq!(table.records.len(), 2);
    assert_eq!(table.warnings.len(), 3);
    // Starts in LBA 0 are never published as data partitions.
    entry(&mut source.0, 512, 0, 0, 0x07, 0, 1);
    entry(&mut source.0, 512, 16, 0, 0x01, 0, 1);
    assert!(read_table(&source, 512).partitions.is_empty());
}

#[test]
fn rejects_invalid_ebr_flags_and_signatures_preserving_raw_record() {
    for (offset, value) in [(510, 0), (446, 0x7f), (494, 0x7f)] {
        let mut source = fixture(512);
        source.0[16 * 512 + offset] = value;
        let table = read_table(&source, 512);
        assert_eq!(ranges(&table), vec![(0, PartitionKind::Ntfs, 2, 4)]);
        assert_eq!(table.records.len(), 2);
        assert_eq!(table.warnings.len(), 1);
    }
}

#[test]
fn raw_type_is_lossless_for_all_256_values() {
    for type_byte in 0..=255 {
        let mut source = fixture(512);
        source.0[462..510].fill(0);
        entry(&mut source.0, 512, 0, 0, type_byte, 2, 4);
        let table = read_table(&source, 512);
        assert_eq!(table.records[0].entries[0].type_byte, type_byte);
        if type_byte == 0 || matches!(type_byte, 0x05 | 0x0f | 0x85) {
            assert!(table.partitions.is_empty());
        } else {
            assert_eq!(table.partitions[0].entry.type_byte, type_byte);
        }
    }
    assert_eq!(
        [0x01, 0x11, 0x14, 0x16, 0x1e, 0x1b, 0x1c, 0x17, 0x27, 0xa8, 0xaf, 0xfd]
            .map(mbr_type_to_kind),
        [
            PartitionKind::Fat12,
            PartitionKind::Fat12,
            PartitionKind::Fat16,
            PartitionKind::Fat16,
            PartitionKind::Fat16,
            PartitionKind::Fat32,
            PartitionKind::Fat32,
            PartitionKind::Ntfs,
            PartitionKind::Ntfs,
            PartitionKind::Ufs,
            PartitionKind::HfsPlus,
            PartitionKind::Mdadm
        ]
    );
}

fn chain(count: usize, with_primary: bool) -> BufSource {
    let mut bytes = vec![0; (count * 2 + 16) * 512];
    record(&mut bytes, 512, 0);
    if with_primary {
        for slot in 0..3 {
            entry(&mut bytes, 512, 0, slot, 0x07, slot as u32 + 1, 1);
        }
    }
    entry(&mut bytes, 512, 0, 3, 0x05, 8, count as u32 * 2);
    for index in 0..count {
        let lba = 8 + index * 2;
        record(&mut bytes, 512, lba);
        entry(&mut bytes, 512, lba, 0, 0x83, 1, 1);
        if index + 1 < count {
            entry(
                &mut bytes,
                512,
                lba,
                1,
                0x05,
                (index as u32 + 1) * 2,
                (count as u32 - index as u32 - 1) * 2,
            );
        }
    }
    BufSource(bytes)
}

#[test]
fn long_chains_keep_unique_ids_and_report_limits() {
    let table = read_table(&chain(256, false), 512);
    assert_eq!(
        table
            .partitions
            .iter()
            .map(|entry| entry.partition.id)
            .collect::<Vec<_>>(),
        (0..=255).collect::<Vec<_>>()
    );
    assert_eq!(
        table.partitions.last().expect("last").partition.start_lba,
        519
    );
    assert!(table.warnings.is_empty());
    let capped = read_table(&chain(257, false), 512);
    assert_eq!(capped.partitions.len(), 256);
    assert!(capped.warnings[0].contains("256 records"));
    assert!(MbrTable::read(&chain(254, true), 512)
        .expect_err("ID exhaustion")
        .to_string()
        .contains("256 partitions"));
}

#[test]
fn empty_signed_table_is_available_without_claiming_a_filesystem() {
    let mut bytes = vec![0; 512];
    record(&mut bytes, 512, 0);
    let source = BufSource(bytes);
    assert!(read_table(&source, 512).partitions.is_empty());
    assert!(!has_boot_signature(&source.0[..511]));
    assert!(MbrBootRecord::decode(&source.0[..511], 0).is_none());
    assert_eq!(read_disk_signature(&source.0), None);
}

#[test]
fn partition_reader_clamps_reads_and_supports_all_seek_origins() {
    let mut source = fixture(512);
    source.0[2 * 512..6 * 512].fill(0x5a);
    let table = read_table(&source, 512);
    let mut reader = MbrPartitionReader::new(&source, &table.partitions[0], 512).expect("reader");
    assert_eq!((reader.volume_offset(), reader.size()), (1024, 2048));
    assert_eq!(reader.seek(SeekFrom::End(-2)).expect("seek"), 2046);
    assert_eq!(reader.read_at(0, 3).expect("read at"), vec![0x5a; 3]);
    assert_eq!(reader.stream_position().expect("position"), 2046);
    let mut buffer = [0; 4];
    assert_eq!(reader.read(&mut buffer).expect("clamp"), 2);
    assert_eq!(buffer, [0x5a, 0x5a, 0, 0]);
    assert_eq!(reader.read(&mut buffer).expect("EOF"), 0);
    assert_eq!(reader.seek(SeekFrom::Current(-2048)).expect("current"), 0);
    assert!(reader.seek(SeekFrom::Current(-1)).is_err());
    assert_eq!(reader.stream_position().expect("position after error"), 0);
    assert_eq!(
        reader
            .seek(SeekFrom::Start(u64::MAX))
            .expect("seek past EOF"),
        u64::MAX
    );
    assert!(reader.seek(SeekFrom::Current(1)).is_err());
    assert!(reader.read_at(u64::MAX, 1).expect("past EOF").is_empty());
    let mut partition = table.partitions[0].clone();
    partition.partition.start_lba = u64::MAX;
    assert!(MbrPartitionReader::new(&source, &partition, 512).is_err());
    partition.partition.start_lba = 63;
    assert!(MbrPartitionReader::new(&source, &partition, 512).is_err());
}

#[test]
fn propagates_io_errors_short_reads_and_cancellation() {
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
                    anyhow::bail!("injected read failure");
                }
            }
            self.source.read(offset, length)
        }
    }
    let fault = FaultSource {
        source: fixture(512),
        offset: 16 * 512,
        short: false,
        cancel: None,
    };
    assert!(format!(
        "{:#}",
        MbrTable::read(&fault, 512).expect_err("I/O failure")
    )
    .contains("injected read failure"));
    let short = FaultSource {
        short: true,
        ..fault
    };
    let table = read_table(&short, 512);
    assert_eq!(table.partitions.len(), 1);
    assert!(table.warnings[0].contains("short EBR"));
    let short = FaultSource {
        source: fixture(512),
        offset: 2 * 512,
        short: true,
        cancel: None,
    };
    let reader = MbrPartitionReader::new(&short, &table.partitions[0], 512).expect("reader");
    assert!(reader
        .read_at(0, 2)
        .expect_err("short read")
        .to_string()
        .contains("short MBR"));
    let cancel = CancellationToken::new();
    let cancelling = FaultSource {
        source: fixture(512),
        offset: 16 * 512,
        short: false,
        cancel: Some(cancel.clone()),
    };
    assert!(MbrTable::read_cancellable(&cancelling, 512, &cancel)
        .expect_err("cancel during read")
        .to_string()
        .contains("cancelled"));
    assert!(MbrTable::read_cancellable(&cancelling, 512, &cancel).is_err());
}
