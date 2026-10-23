//! UFS2 (FreeBSD Fast Filesystem v2) whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` so it only fires
//! when no real partition table claimed the disk. UFS2 stashes its primary
//! superblock at byte offset `0x10000` (= 65536) within the volume, with the
//! 4-byte magic word `0x19540119` at byte 1372 *inside* that superblock —
//! both little-endian (`19 01 54 19`) and big-endian (`19 54 01 19`) layouts
//! exist on FreeBSD. Synthesizes a single whole-device `Partition` so the rest
//! of the pipeline can treat the bare filesystem the same way as a partitioned
//! one.
//!
//! Partitioned UFS (e.g. FreeBSD installer images that ship the rootfs as a
//! `516e7cb6-6ecf-11d6-8ff8-00022d09712b` GPT partition) is handled by
//! [`crate::core::evidence::partitions::refine_partition_kinds`] via the dedicated UFS
//! probe at byte offset `UFS_SBLOCK_UFS2` of each candidate partition.
//!
//! UFS1 (FreeBSD ≤ 4.x, OpenBSD, NetBSD's older variants) lives at byte
//! offset 8192 with magic `0x11954` and is not currently recognised — `rufs`
//! itself is UFS2-only.

use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// UFS2 superblock byte offset within the volume. Mirrors `rufs::SBLOCK_UFS2`.
pub const UFS_SBLOCK_UFS2: usize = 65536;
/// Offset of the magic word inside the UFS2 superblock. Mirrors
/// `rufs::MAGIC_OFFSET`.
pub const UFS_MAGIC_OFFSET: u64 = 1372;

/// UFS2 magic on disk in little-endian variant. The on-disk magic numerically
/// is `0x19540119` (`FS_UFS2_MAGIC` in FreeBSD), so the LE byte order is
/// `19 01 54 19`.
pub const UFS2_MAGIC_LE_BYTES: [u8; 4] = [0x19, 0x01, 0x54, 0x19];
/// UFS2 magic in big-endian variant: `19 54 01 19`. FreeBSD officially writes
/// LE on x86 but the BE form survives from older PowerPC / SPARC ports and
/// `rufs` accepts both.
pub const UFS2_MAGIC_BE_BYTES: [u8; 4] = [0x19, 0x54, 0x01, 0x19];

/// True when `superblock_bytes` (read starting at byte `UFS_SBLOCK_UFS2` of
/// the volume) carries the UFS2 magic at `UFS_MAGIC_OFFSET`.
pub fn is_ufs2_magic_at_superblock(superblock_bytes: &[u8]) -> bool {
    let off = UFS_MAGIC_OFFSET as usize;
    if superblock_bytes.len() < off + 4 {
        return false;
    }
    let magic = &superblock_bytes[off..off + 4];
    magic == UFS2_MAGIC_LE_BYTES || magic == UFS2_MAGIC_BE_BYTES
}

pub struct UfsParser;

impl PartParser for UfsParser {
    fn name(&self) -> &'static str {
        "ufs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 {
            return Ok(None);
        }
        // The probe buffer cached in `ctx.head` is only `HEADER_PROBE_BYTES`
        // (0x2000) wide; UFS2's magic is much further in (≈ 0x10000 + 1372),
        // so we have to issue an explicit read against the underlying source
        // to check it.
        let needed = UFS_SBLOCK_UFS2 as u64 + UFS_MAGIC_OFFSET + 4;
        if ctx.size < needed {
            return Ok(None);
        }
        let probe = ctx
            .source
            .read(UFS_SBLOCK_UFS2 as u64, 0x800)
            .unwrap_or_default();
        if !is_ufs2_magic_at_superblock(&probe) {
            return Ok(None);
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Ufs,
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

    fn build_ufs_image(total_sectors: usize, sector_size: usize, magic: [u8; 4]) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        let off = UFS_SBLOCK_UFS2 + UFS_MAGIC_OFFSET as usize;
        buf[off..off + 4].copy_from_slice(&magic);
        buf
    }

    #[test]
    fn detects_whole_disk_ufs2_le() {
        let buf = build_ufs_image(256, 512, UFS2_MAGIC_LE_BYTES);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Ufs);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 256);
    }

    #[test]
    fn detects_whole_disk_ufs2_be() {
        let buf = build_ufs_image(256, 512, UFS2_MAGIC_BE_BYTES);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Ufs);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 256 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_image_too_small_for_superblock() {
        // 16 sectors × 512 = 8 KiB, far short of `UFS_SBLOCK_UFS2`.
        let buf = vec![0u8; 16 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }
}
