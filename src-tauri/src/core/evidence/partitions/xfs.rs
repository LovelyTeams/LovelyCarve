//! XFS whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` so it only fires
//! when no real partition table claimed the disk. The XFS primary superblock
//! lives at byte offset 0 of the volume; the 4-byte big-endian magic is
//! `XFSB` (`0x58 0x46 0x53 0x42`). XFS is overwhelmingly used on Linux
//! servers (`/boot` partitions on RHEL/CentOS 7+, root volumes inside LVM,
//! etc.), so most of the time we see it inside an MBR/GPT partition rather
//! than as a bare disk — that path is handled by
//! [`crate::core::evidence::partitions::detect_filesystem`] which the partition-table
//! parsers call to refine the kind.
//!
//! This whole-device parser only catches images that were `mkfs.xfs`'d
//! directly onto a raw block device with no partition table — uncommon, but
//! not impossible (custom appliance disks, unit-test fixtures, some
//! virtualisation setups).

use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// XFS superblock magic, big-endian on disk.
pub const XFS_MAGIC: [u8; 4] = [0x58, 0x46, 0x53, 0x42];

/// True when the first four bytes of `bytes` carry the XFS magic.
pub fn is_xfs_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == XFS_MAGIC
}

pub struct XfsParser;

impl PartParser for XfsParser {
    fn name(&self) -> &'static str {
        "xfs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || ctx.size < ctx.sector_size as u64 {
            return Ok(None);
        }
        if !is_xfs_magic(ctx.head) {
            return Ok(None);
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Xfs,
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

    fn build_xfs_image(total_sectors: usize, sector_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        buf[..4].copy_from_slice(&XFS_MAGIC);
        buf
    }

    #[test]
    fn detects_whole_disk_xfs() {
        let buf = build_xfs_image(256, 512);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Xfs);
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
}
