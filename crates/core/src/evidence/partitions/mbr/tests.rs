use super::*;
use crate::evidence::partitions::{
    detect_partitions, detect_partitions_with_meta, test_support::BufSource, HEADER_PROBE_BYTES,
};

#[test]
fn detects_large_ntfs_volume_with_boot_messages_as_volume() {
    // Logical acquisitions can retain the source disk's 2 TB size while
    // beginning directly with a smaller NTFS volume. These actual Windows
    // boot messages form four nonzero, in-bounds pseudo-partitions.
    struct LargeVolumeSource(BufSource);
    impl SectorSource for LargeVolumeSource {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            self.0.read(offset, len)
        }

        fn size(&self) -> u64 {
            3_907_024_065 * 512
        }
    }

    let mut head = vec![0u8; HEADER_PROBE_BYTES];
    head[..11].copy_from_slice(b"\xeb\x52\x90NTFS    ");
    head[11..13].copy_from_slice(&512u16.to_le_bytes());
    head[13] = 8;
    head[40..48].copy_from_slice(&537_894_911u64.to_le_bytes());
    head[48..56].copy_from_slice(&786_432u64.to_le_bytes());
    head[MBR_PART_TABLE_OFFSET..510].copy_from_slice(
        b"\r\nBOOTMGR is compressed\0\r\nPress Ctrl+Alt+Del to restart\r\n\0\x8c\xa9\xbe\xd6\0\0",
    );
    head[510..512].copy_from_slice(&MBR_MAGIC);
    let source = LargeVolumeSource(BufSource(head));
    let detected = detect_partitions_with_meta(&source, 512).expect("detect NTFS volume");

    assert_eq!(
        (detected.scheme, detected.disk_id, detected.partitions),
        (
            Some(TableScheme::Volume),
            None,
            vec![Partition {
                id: 0,
                kind: PartitionKind::Ntfs,
                start_lba: 0,
                length_lba: 3_907_024_065,
                label: None,
                unique_guid: None,
                amiga_geometry: None,
            }],
        )
    );
}

#[test]
fn rejects_signed_table_with_invalid_boot_indicator_in_any_slot() {
    for invalid_slot in 0..MBR_ENTRY_COUNT {
        let mut head = vec![0u8; 512 * 64];
        head[510..512].copy_from_slice(&MBR_MAGIC);
        for slot in 0..MBR_ENTRY_COUNT {
            let base = MBR_PART_TABLE_OFFSET + slot * MBR_PART_ENTRY_SIZE;
            head[base] = if slot == invalid_slot { 0x7f } else { 0x00 };
            head[base + 4] = 0x07;
            head[base + 8..base + 12].copy_from_slice(&(1 + slot as u32 * 8).to_le_bytes());
            head[base + 12..base + 16].copy_from_slice(&8u32.to_le_bytes());
        }
        let source = BufSource(head);
        let ctx = DetectCtx {
            source: &source,
            head: &source.0,
            sector_size: 512,
            size: source.size(),
        };
        assert_eq!(MbrParser.try_parse(&ctx).expect("parse MBR"), None);
    }
}

#[test]
fn parses_mbr_with_primary_partitions() {
    // Image large enough that LBA 2048 + 10 stays in bounds (P1-11).
    let mut buf = vec![0u8; 512 * 2100];
    buf[510] = 0x55;
    buf[511] = 0xAA;
    let e0 = MBR_PART_TABLE_OFFSET;
    buf[e0] = 0x80;
    buf[e0 + 4] = 0x07;
    buf[e0 + 8..e0 + 12].copy_from_slice(&2048u32.to_le_bytes());
    buf[e0 + 12..e0 + 16].copy_from_slice(&10u32.to_le_bytes());
    let src = BufSource(buf);
    let parts = detect_partitions(&src, 512).expect("detect");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].kind, PartitionKind::Ntfs);
    assert_eq!(parts[0].start_lba, 2048);
}

#[test]
fn follows_ebr_chain() {
    let sector = 512usize;
    let ext_base_lba = 2048u32;
    let mut buf = vec![0u8; sector * (ext_base_lba as usize + 200)];
    buf[510] = 0x55;
    buf[511] = 0xAA;
    let e0 = MBR_PART_TABLE_OFFSET;
    buf[e0 + 4] = 0x05;
    buf[e0 + 8..e0 + 12].copy_from_slice(&ext_base_lba.to_le_bytes());
    buf[e0 + 12..e0 + 16].copy_from_slice(&200u32.to_le_bytes());

    let ebr1 = ext_base_lba as usize * sector;
    buf[ebr1 + 510] = 0x55;
    buf[ebr1 + 511] = 0xAA;
    buf[ebr1 + e0 + 4] = 0x83;
    buf[ebr1 + e0 + 8..ebr1 + e0 + 12].copy_from_slice(&1u32.to_le_bytes());
    buf[ebr1 + e0 + 12..ebr1 + e0 + 16].copy_from_slice(&50u32.to_le_bytes());

    let src = BufSource(buf);
    let parts = detect_partitions(&src, sector).expect("detect");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].start_lba, ext_base_lba as u64 + 1);
    assert_eq!(parts[0].length_lba, 50);
}

#[test]
fn ignores_boot_sector_without_partition_entries() {
    let mut buf = vec![0u8; 4096];
    buf[510] = 0x55;
    buf[511] = 0xAA;
    let src = BufSource(buf);

    let parts = detect_partitions(&src, 512).expect("detect");

    assert!(parts.is_empty());
}

/// P1-12: a logical partition placed in EBR slot 2 (not the conventional
/// data slot 0) must still be surfaced, alongside the slot-0 data entry.
#[test]
fn follows_ebr_chain_with_data_entry_in_slot_two() {
    let sector = 512usize;
    let ext_base_lba = 2048usize;
    let mut buf = vec![0u8; sector * (ext_base_lba + 400)];
    buf[510] = 0x55;
    buf[511] = 0xAA;
    let e0 = MBR_PART_TABLE_OFFSET;
    buf[e0 + 4] = 0x05;
    buf[e0 + 8..e0 + 12].copy_from_slice(&(ext_base_lba as u32).to_le_bytes());
    buf[e0 + 12..e0 + 16].copy_from_slice(&400u32.to_le_bytes());

    let ebr1 = ext_base_lba * sector;
    buf[ebr1 + 510] = 0x55;
    buf[ebr1 + 511] = 0xAA;
    // Slot 0: a normal logical FAT16 partition.
    buf[ebr1 + e0 + 4] = 0x06;
    buf[ebr1 + e0 + 8..ebr1 + e0 + 12].copy_from_slice(&1u32.to_le_bytes());
    buf[ebr1 + e0 + 12..ebr1 + e0 + 16].copy_from_slice(&50u32.to_le_bytes());
    // Slot 2: a second logical partition that the old single-slot reader
    // dropped. Entry stride is MBR_PART_ENTRY_SIZE (16 bytes).
    let s2 = e0 + 2 * MBR_PART_ENTRY_SIZE;
    buf[ebr1 + s2 + 4] = 0x83;
    buf[ebr1 + s2 + 8..ebr1 + s2 + 12].copy_from_slice(&60u32.to_le_bytes());
    buf[ebr1 + s2 + 12..ebr1 + s2 + 16].copy_from_slice(&70u32.to_le_bytes());

    let src = BufSource(buf);
    let parts = detect_partitions(&src, sector).expect("detect");
    assert_eq!(parts.len(), 2, "both logical entries must be surfaced");
    assert_eq!(parts[0].start_lba, ext_base_lba as u64 + 1);
    assert_eq!(parts[0].kind, PartitionKind::Fat16);
    assert_eq!(parts[1].start_lba, ext_base_lba as u64 + 60);
}

/// P1-11: a stray sector ending in 0x55AA whose leading entry starts past
/// the image must not be accepted as a partition table.
#[test]
fn rejects_primary_table_with_out_of_bounds_leading_entry() {
    let sector = 512usize;
    let mut buf = vec![0u8; sector * 64];
    buf[510] = 0x55;
    buf[511] = 0xAA;
    let e0 = MBR_PART_TABLE_OFFSET;
    buf[e0 + 4] = 0x07;
    // start_lba 9_000 is well past the 64-sector image.
    buf[e0 + 8..e0 + 12].copy_from_slice(&9_000u32.to_le_bytes());
    buf[e0 + 12..e0 + 16].copy_from_slice(&10u32.to_le_bytes());
    let src = BufSource(buf);
    let parts = detect_partitions(&src, sector).expect("detect");
    assert!(
        parts.is_empty(),
        "out-of-bounds MBR must not yield partitions"
    );
}

#[test]
fn recovers_unsigned_mbr_when_partition_signature_confirms_it() {
    let sector = 512usize;
    let start_lba = 2048u32;
    let length_lba = 2048u32;
    let mut buf = vec![0u8; sector * 4096];
    let entry = MBR_PART_TABLE_OFFSET;
    buf[entry] = 0x80;
    buf[entry + 4] = 0x83;
    buf[entry + 8..entry + 12].copy_from_slice(&start_lba.to_le_bytes());
    buf[entry + 12..entry + 16].copy_from_slice(&length_lba.to_le_bytes());
    let ext_magic = start_lba as usize * sector + 0x438;
    buf[ext_magic..ext_magic + 2].copy_from_slice(&[0x53, 0xEF]);

    let parts = detect_partitions(&BufSource(buf), sector).expect("detect");

    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].start_lba, u64::from(start_lba));
    assert_eq!(parts[0].length_lba, u64::from(length_lba));
}

#[test]
fn rejects_unsigned_table_without_confirmed_partition_signature() {
    let sector = 512usize;
    let mut buf = vec![0u8; sector * 4096];
    let entry = MBR_PART_TABLE_OFFSET;
    buf[entry] = 0x80;
    buf[entry + 4] = 0x83;
    buf[entry + 8..entry + 12].copy_from_slice(&2048u32.to_le_bytes());
    buf[entry + 12..entry + 16].copy_from_slice(&2048u32.to_le_bytes());

    let parts = detect_partitions(&BufSource(buf), sector).expect("detect");

    assert!(parts.is_empty());
}
