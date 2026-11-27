//! UFS1 and UFS2 primary superblock probes for bare and partitioned images.

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

/// Return the on-disk magic offset and the detected revision.
pub(super) fn probe_ufs(ctx: &DetectCtx<'_>) -> anyhow::Result<Option<(u64, &'static str)>> {
    for (start, magic, version) in [
        (UFS_SBLOCK_UFS2 as u64, 0x1954_0119i32, "UFS2"),
        (8192, 0x0001_1954, "UFS1"),
        (8192, 0x1954_0119, "UFS2"),
    ] {
        let offset = start + UFS_MAGIC_OFFSET;
        if ctx.size < offset + 4 {
            continue;
        }
        let read = ctx.source.read(offset, 4)?;
        if read.as_slice() == magic.to_le_bytes() || read.as_slice() == magic.to_be_bytes() {
            return Ok(Some((offset, version)));
        }
    }
    Ok(None)
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
        if probe_ufs(ctx)?.is_none() {
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
            member_of: None,
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
