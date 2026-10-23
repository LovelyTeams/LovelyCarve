//! SquashFS whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` so it only fires
//! when no real partition table claimed the disk. SquashFS v4 puts its 4-byte
//! magic at byte offset 0 of the superblock — `hsqs` (`0x73717368`) for the
//! standard little-endian variant, `sqsh` for big-endian. Synthesizes a
//! single whole-device `Partition` so the rest of the pipeline can treat the
//! bare filesystem the same way as a partitioned one.
//!
//! For images whose squashfs lives inside a real MBR / GPT partition (e.g.
//! the OpenWRT `*-squashfs-combined.img` layout: kernel partition + rootfs
//! partition), the MBR parser runs first and the per-partition probe in
//! [`crate::core::evidence::partitions::detect_filesystem`] handles the magic detection
//! per-slice.

use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// SquashFS v4 little-endian magic at offset 0 of the superblock.
pub(crate) const SQUASHFS_MAGIC_LE: &[u8; 4] = b"hsqs";
/// SquashFS v4 big-endian magic at offset 0 of the superblock.
pub(crate) const SQUASHFS_MAGIC_BE: &[u8; 4] = b"sqsh";

pub fn is_squashfs_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(SQUASHFS_MAGIC_LE)
        || bytes.starts_with(SQUASHFS_MAGIC_BE)
        || bytes.starts_with(b"hsqt")
        || bytes.starts_with(b"tqsh")
}

pub struct SquashfsParser;

impl PartParser for SquashfsParser {
    fn name(&self) -> &'static str {
        "squashfs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || ctx.head.len() < 4 {
            return Ok(None);
        }
        if !is_squashfs_magic(&ctx.head[..4]) {
            return Ok(None);
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Squashfs,
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

    fn build_squashfs_image(total_sectors: usize, sector_size: usize, magic: &[u8; 4]) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        buf[..4].copy_from_slice(magic);
        buf
    }

    #[test]
    fn detects_whole_disk_squashfs_le() {
        let buf = build_squashfs_image(256, 512, SQUASHFS_MAGIC_LE);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Squashfs);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 256);
    }

    #[test]
    fn detects_whole_disk_squashfs_be() {
        let buf = build_squashfs_image(64, 512, SQUASHFS_MAGIC_BE);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Squashfs);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 256 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }
}
