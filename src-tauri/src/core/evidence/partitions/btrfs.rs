//! Btrfs whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` so it only fires
//! when no real partition table claimed the disk. Reads the primary
//! superblock at byte offset `0x10000` and validates the 8-byte
//! `_BHRfS_M` magic at internal offset `0x40`. Synthesizes a single
//! whole-device `Partition` so the rest of the pipeline can treat the bare
//! filesystem the same way as a partitioned one.

use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, BTRFS_MAGIC, BTRFS_PRIMARY_SUPERBLOCK,
};

pub struct BtrfsParser;

impl PartParser for BtrfsParser {
    fn name(&self) -> &'static str {
        "btrfs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || ctx.size < BTRFS_PRIMARY_SUPERBLOCK + 0x48 {
            return Ok(None);
        }
        let probe = ctx.source.read(BTRFS_PRIMARY_SUPERBLOCK, 0x100)?;
        if probe.len() < 0x48 || &probe[0x40..0x48] != BTRFS_MAGIC {
            return Ok(None);
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Btrfs,
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
    use crate::core::evidence::partitions::{detect_partitions, test_support::BufSource};

    fn build_btrfs_image(total_sectors: usize, sector_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        let sb_off = BTRFS_PRIMARY_SUPERBLOCK as usize;
        // Magic at internal offset 0x40 — anything else stays zeroed.
        buf[sb_off + 0x40..sb_off + 0x48].copy_from_slice(BTRFS_MAGIC);
        buf
    }

    #[test]
    fn detects_whole_disk_btrfs() {
        let buf = build_btrfs_image(256, 512);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Btrfs);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 256);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 256 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_image_smaller_than_superblock() {
        let buf = vec![0u8; 0x10000];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn yields_to_mbr_when_partition_table_present() {
        let mut buf = build_btrfs_image(256, 512);
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
