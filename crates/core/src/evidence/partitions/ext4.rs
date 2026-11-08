//! Ext2 / Ext3 / Ext4 whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` (next to
//! `BtrfsParser`) so it only fires when no real partition table claimed
//! the disk. Validates the 2-byte ext{2,3,4} superblock magic `0xEF53` at
//! file offset `0x438` (= 1080 within the primary superblock that lives
//! at offset `0x400`). The probe re-uses `DetectCtx::head` — no extra IO
//! needed since `HEADER_PROBE_BYTES = 0x2000` already covers the
//! superblock window. Synthesizes a single whole-device `Partition` so
//! the rest of the pipeline can treat the bare filesystem the same way
//! as a partitioned one.

use crate::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// Byte offset of the low byte of the ext superblock magic
/// (`s_magic = 0xEF53` at superblock-internal offset `0x38`, where the
/// superblock itself starts at file offset `0x400`).
const EXT_MAGIC_OFFSET: usize = 0x400 + 0x38;

pub struct Ext4Parser;

impl PartParser for Ext4Parser {
    fn name(&self) -> &'static str {
        "ext4"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || ctx.head.len() < EXT_MAGIC_OFFSET + 2 {
            return Ok(None);
        }
        if ctx.head[EXT_MAGIC_OFFSET] != 0x53 || ctx.head[EXT_MAGIC_OFFSET + 1] != 0xEF {
            return Ok(None);
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Ext4,
            start_lba: 0,
            length_lba,
            label: None,
            unique_guid: None,
            amiga_geometry: None,
        }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::partitions::{detect_partitions, test_support::BufSource};

    fn build_ext4_image(total_sectors: usize, sector_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        // Magic bytes (LE 0xEF53) at file offset 0x438 — anything else stays zeroed.
        buf[EXT_MAGIC_OFFSET] = 0x53;
        buf[EXT_MAGIC_OFFSET + 1] = 0xEF;
        buf
    }

    #[test]
    fn detects_whole_disk_ext4() {
        let buf = build_ext4_image(64, 512);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Ext4);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 64);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 64 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_image_smaller_than_superblock() {
        let buf = vec![0u8; 0x400];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn yields_to_mbr_when_partition_table_present() {
        let mut buf = build_ext4_image(256, 512);
        // Plant a real MBR at offset 0 so MbrParser claims the disk first.
        buf[510] = 0x55;
        buf[511] = 0xAA;
        buf[0x1BE + 4] = 0x07; // NTFS type
        buf[0x1BE + 8..0x1BE + 12].copy_from_slice(&8u32.to_le_bytes());
        buf[0x1BE + 12..0x1BE + 16].copy_from_slice(&64u32.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Ntfs);
    }
}
