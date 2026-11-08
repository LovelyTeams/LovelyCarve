#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::Read;
use vhdx::{
    header::crc32c,
    metadata::{GUID_FILE_PARAMETERS, GUID_LOGICAL_SECTOR_SIZE, GUID_VIRTUAL_DISK_SIZE},
    region::{BAT_GUID, METADATA_GUID},
    VhdxReader,
};

// ─── Layout of the synthetic test image ────────────────────────────────────
//   0x000000 – 0x010000  File identifier (64 KB)
//   0x010000 – 0x020000  Header 1 slot (64 KB, first 4 KB is the active header)
//   0x020000 – 0x030000  Header 2 slot (64 KB, seq=0 / clean)
//   0x030000 – 0x040000  Region Table 1 (64 KB)
//   0x040000 – 0x050000  Region Table 2 (64 KB)
//   0x100000 – 0x200000  Log region (1 MB) — contains one dirty log entry
//   0x200000 – 0x300000  Metadata region (1 MB)
//   0x300000 – 0x400000  BAT region (1 MB, only first 8 bytes used)
//   0x400000 – 0x500000  Data block 0 (1 MB, byte 0 = 0x00 before log replay)
//
// The log entry patches data[0x400000] = 0xAB.  Without replay the reader
// returns 0x00; after replay it must return 0xAB.
// ───────────────────────────────────────────────────────────────────────────

const FILE_SIZE: usize = 0x0050_0000;
const LOG_OFFSET: u64 = 0x0010_0000;
const LOG_LENGTH: u32 = 0x0010_0000;
const META_OFFSET: u64 = 0x0020_0000;
const META_LENGTH: u32 = 0x0010_0000;
const BAT_OFFSET: u64 = 0x0030_0000;
const BAT_LENGTH: u32 = 0x0010_0000;
const DATA_OFFSET: u64 = 0x0040_0000;
const VIRTUAL_DISK_SIZE: u64 = 0x0010_0000; // 1 MB
const BLOCK_SIZE: u32 = 0x0010_0000; // 1 MB

// Non-zero GUID to mark the log as dirty (must match the log entry's LogGuid).
const LOG_GUID: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
];

fn write_header(slot: &mut [u8], seq: u64, log_guid: [u8; 16], log_off: u64, log_len: u32) {
    // Header occupies the first 4096 bytes of the 64 KB slot (§2.1.2).
    let h = &mut slot[..4096];
    h[0..4].copy_from_slice(b"head");
    h[4..8].fill(0); // CRC placeholder
    h[8..16].copy_from_slice(&seq.to_le_bytes()); // SequenceNumber
                                                  // FileWriteGuid [16..32] and DataWriteGuid [32..48] left as zeros (valid).
    h[48..64].copy_from_slice(&log_guid); // LogGuid
    h[64..66].copy_from_slice(&1u16.to_le_bytes()); // LogVersion
    h[66..68].copy_from_slice(&1u16.to_le_bytes()); // Version
    h[68..72].copy_from_slice(&log_len.to_le_bytes()); // LogLength
    h[72..80].copy_from_slice(&log_off.to_le_bytes()); // LogOffset
    let c = crc32c(h);
    h[4..8].copy_from_slice(&c.to_le_bytes());
}

fn write_region_table(rt: &mut [u8], bat_off: u64, bat_len: u32, meta_off: u64, meta_len: u32) {
    // Header: 16 bytes.  Entries start at offset 16, each 32 bytes (§2.2).
    rt[0..4].copy_from_slice(b"regi");
    rt[4..8].fill(0); // CRC placeholder
    rt[8..12].copy_from_slice(&2u32.to_le_bytes()); // EntryCount
    rt[12..16].fill(0);

    // Entry 0: BAT
    rt[16..32].copy_from_slice(&BAT_GUID);
    rt[32..40].copy_from_slice(&bat_off.to_le_bytes());
    rt[40..44].copy_from_slice(&bat_len.to_le_bytes());
    rt[44..48].copy_from_slice(&1u32.to_le_bytes()); // Required

    // Entry 1: Metadata
    rt[48..64].copy_from_slice(&METADATA_GUID);
    rt[64..72].copy_from_slice(&meta_off.to_le_bytes());
    rt[72..76].copy_from_slice(&meta_len.to_le_bytes());
    rt[76..80].copy_from_slice(&1u32.to_le_bytes());

    // CRC covers the full 64 KB block with [4..8] zeroed.
    let mut tmp = rt[..65536].to_vec();
    tmp[4..8].fill(0);
    let c = crc32c(&tmp);
    rt[4..8].copy_from_slice(&c.to_le_bytes());
}

fn write_metadata(region: &mut [u8]) {
    // Metadata table header (§2.5.4): 32-byte header + entries.
    region[0..8].copy_from_slice(b"metadata");
    region[10..12].copy_from_slice(&3u16.to_le_bytes()); // EntryCount

    // Item data sits at 0x200 within the region (well past the table header).
    const FP_OFF: u32 = 0x200;
    const VDS_OFF: u32 = 0x210;
    const LSS_OFF: u32 = 0x220;

    // Entry 0: FileParameters
    region[32..48].copy_from_slice(&GUID_FILE_PARAMETERS);
    region[48..52].copy_from_slice(&FP_OFF.to_le_bytes());
    region[52..56].copy_from_slice(&8u32.to_le_bytes());

    // Entry 1: VirtualDiskSize
    region[64..80].copy_from_slice(&GUID_VIRTUAL_DISK_SIZE);
    region[80..84].copy_from_slice(&VDS_OFF.to_le_bytes());
    region[84..88].copy_from_slice(&8u32.to_le_bytes());

    // Entry 2: LogicalSectorSize
    region[96..112].copy_from_slice(&GUID_LOGICAL_SECTOR_SIZE);
    region[112..116].copy_from_slice(&LSS_OFF.to_le_bytes());
    region[116..120].copy_from_slice(&4u32.to_le_bytes());

    // FileParameters: BlockSize + Flags
    region[FP_OFF as usize..FP_OFF as usize + 4].copy_from_slice(&BLOCK_SIZE.to_le_bytes());
    region[FP_OFF as usize + 4..FP_OFF as usize + 8].fill(0); // has_parent=false

    // VirtualDiskSize
    region[VDS_OFF as usize..VDS_OFF as usize + 8]
        .copy_from_slice(&VIRTUAL_DISK_SIZE.to_le_bytes());

    // LogicalSectorSize
    region[LSS_OFF as usize..LSS_OFF as usize + 4].copy_from_slice(&512u32.to_le_bytes());
}

/// Build a 5 MB VHDX with:
///  - one 1 MB data block whose byte 0 is 0x00 in the file
///  - a dirty log entry that writes 0xAB to file offset `DATA_OFFSET+0`
///
/// A reader that applies log replay must return 0xAB on the first virtual
/// byte; one that ignores the log returns 0x00.
fn build_dirty_log_vhdx() -> Vec<u8> {
    let mut buf = vec![0u8; FILE_SIZE];

    // File identifier
    buf[0..8].copy_from_slice(b"vhdxfile");

    // Header 1 (active, seq=1, dirty LogGuid)
    write_header(
        &mut buf[0x10000..0x20000],
        1,
        LOG_GUID,
        LOG_OFFSET,
        LOG_LENGTH,
    );

    // Header 2 (inactive, seq=0)
    write_header(&mut buf[0x20000..0x30000], 0, [0u8; 16], 0, 0);

    // Region tables
    write_region_table(
        &mut buf[0x30000..0x40000],
        BAT_OFFSET,
        BAT_LENGTH,
        META_OFFSET,
        META_LENGTH,
    );
    write_region_table(
        &mut buf[0x40000..0x50000],
        BAT_OFFSET,
        BAT_LENGTH,
        META_OFFSET,
        META_LENGTH,
    );

    // Metadata region
    write_metadata(&mut buf[META_OFFSET as usize..(META_OFFSET + u64::from(META_LENGTH)) as usize]);

    // BAT: block 0 FULLY_PRESENT at DATA_OFFSET.
    // bat_entry = (file_offset_in_mb << 20) | state
    // DATA_OFFSET = 0x400000 → file_offset_in_mb = 4 → bat_entry = 0x400000 | 6
    let bat_entry: u64 = (DATA_OFFSET >> 20) << 20 | 6;
    buf[BAT_OFFSET as usize..BAT_OFFSET as usize + 8].copy_from_slice(&bat_entry.to_le_bytes());

    // Data block at DATA_OFFSET: byte 0 = 0x00 (pre-replay value, already zero).

    // Log entry: one "desc" descriptor writing a 4096-byte sector to DATA_OFFSET.
    //   Entry layout: 64-byte header | 32-byte descriptor | 4096-byte data sector
    const ENTRY_LEN: usize = 64 + 32 + 4096; // 4192
    let log_base = LOG_OFFSET as usize;
    {
        let e = &mut buf[log_base..log_base + ENTRY_LEN];

        // Entry header (§2.3.3)
        e[0..4].copy_from_slice(b"loge");
        // e[4..8] CRC = 0 for now (fill after)
        e[8..12].copy_from_slice(&(ENTRY_LEN as u32).to_le_bytes()); // EntryLength
                                                                     // e[12..16] Tail = 0
        e[16..24].copy_from_slice(&1u64.to_le_bytes()); // SequenceNumber
        e[24..28].copy_from_slice(&1u32.to_le_bytes()); // DescriptorCount
                                                        // e[28..32] Reserved
        e[32..48].copy_from_slice(&LOG_GUID); // LogGuid
        let file_size = FILE_SIZE as u64;
        e[48..56].copy_from_slice(&file_size.to_le_bytes()); // FlushedFileOffset
        e[56..64].copy_from_slice(&file_size.to_le_bytes()); // LastFileOffset

        // Data descriptor (§2.3.3.1) at offset 64
        e[64..68].copy_from_slice(b"desc");
        // e[68] TrailingByte = 0, e[72] LeadingByte = 0 (full-sector write)
        e[80..88].copy_from_slice(&1u64.to_le_bytes()); // SequenceNumber
        e[88..96].copy_from_slice(&DATA_OFFSET.to_le_bytes()); // FileOffset

        // Data sector at offset 96: byte 0 = 0xAB, rest = 0x00
        e[96] = 0xAB;

        // Compute and store CRC32C (over all ENTRY_LEN bytes with [4..8] = 0).
        let c = crc32c(e);
        e[4..8].copy_from_slice(&c.to_le_bytes());
    }

    buf
}

#[test]
fn log_replay_patches_data_byte() {
    let image = build_dirty_log_vhdx();
    let mut reader = VhdxReader::from_bytes(image).expect("dirty-log VHDX must open");
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf).expect("must read byte 0");
    assert_eq!(
        buf[0], 0xAB,
        "log replay must have patched byte 0 from 0x00 to 0xAB"
    );
}

#[test]
fn clean_image_unaffected_by_log_module() {
    // Ensure log::apply is a no-op on an image with LogGuid = zeros.
    // Uses the clean dirty-log image but with LogGuid cleared → header 1 wins
    // with seq=1 but LogGuid=zeros → no replay → byte 0 stays 0x00.
    let mut image = build_dirty_log_vhdx();
    // Clear LogGuid in header 1 slot [48..64] and recompute CRC.
    {
        let h = &mut image[0x10000..0x10000 + 4096];
        h[48..64].fill(0); // clear LogGuid
        h[4..8].fill(0);
        let c = crc32c(h);
        h[4..8].copy_from_slice(&c.to_le_bytes());
    }
    let mut reader = VhdxReader::from_bytes(image).expect("clean-guid image must open");
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf).expect("must read byte 0");
    assert_eq!(buf[0], 0x00, "no log replay when LogGuid is zeros");
}
