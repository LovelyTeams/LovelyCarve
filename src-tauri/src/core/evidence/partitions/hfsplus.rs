//! HFS+ / HFSX whole-device detection.
//!
//! Sits at the end of the `PARSERS` table in `partitions.rs` so it only fires
//! when no real partition table (MBR / GPT / APM) claimed the disk. HFS+
//! places its volume header at byte offset 1024 (sector 2 with a 512-byte
//! sector size) — a 2-byte big-endian magic that is either `H+` (`0x482B`)
//! for the case-insensitive variant or `HX` (`0x4858`) for HFSX
//! (case-sensitive).
//!
//! Synthesizes a single whole-device `Partition` so the rest of the pipeline
//! can treat the bare filesystem the same way as a partitioned one.
//! This is the path that fires for DMGs whose main partition was extracted
//! to a temp file by [`crate::core::evidence::image::ImageSource::Dmg`]: the extracted
//! HFS+/HFSX partition has no MBR/GPT around it.

use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// Byte offset of the HFS+ volume header from the start of the volume.
pub(crate) const HFS_VOLUME_HEADER_OFFSET: usize = 1024;
/// Big-endian "H+" — case-insensitive HFS+.
pub(crate) const HFS_PLUS_MAGIC: [u8; 2] = [0x48, 0x2B];
/// Big-endian "HX" — case-sensitive HFSX.
pub(crate) const HFSX_MAGIC: [u8; 2] = [0x48, 0x58];

/// Returns true when `bytes` (read from offset 0 of the volume) carries an
/// HFS+ or HFSX magic at the canonical volume-header offset.
pub fn is_hfs_plus_magic(bytes: &[u8]) -> bool {
    if bytes.len() < HFS_VOLUME_HEADER_OFFSET + 2 {
        return false;
    }
    let magic = &bytes[HFS_VOLUME_HEADER_OFFSET..HFS_VOLUME_HEADER_OFFSET + 2];
    magic == HFS_PLUS_MAGIC || magic == HFSX_MAGIC
}

pub struct HfsPlusParser;

impl PartParser for HfsPlusParser {
    fn name(&self) -> &'static str {
        "hfsplus"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 {
            return Ok(None);
        }
        if !is_hfs_plus_magic(ctx.head) {
            return Ok(None);
        }
        let kind = match &ctx.head[HFS_VOLUME_HEADER_OFFSET..HFS_VOLUME_HEADER_OFFSET + 2] {
            magic if magic == HFSX_MAGIC => PartitionKind::HfsX,
            _ => PartitionKind::HfsPlus,
        };
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind,
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

    fn build_hfs_image(total_sectors: usize, sector_size: usize, magic: [u8; 2]) -> Vec<u8> {
        let mut buf = vec![0u8; total_sectors * sector_size];
        buf[HFS_VOLUME_HEADER_OFFSET..HFS_VOLUME_HEADER_OFFSET + 2].copy_from_slice(&magic);
        // The volume reader checks the version word right after the magic;
        // 0x0004 = HFS+, 0x0005 = HFSX. Plant 0x0004 so probe-only callers
        // that try to construct a `hfsplus::VolumeHeader` over the same
        // bytes don't reject the fixture for an unsupported version.
        buf[HFS_VOLUME_HEADER_OFFSET + 2..HFS_VOLUME_HEADER_OFFSET + 4]
            .copy_from_slice(&[0x00, 0x04]);
        buf
    }

    #[test]
    fn detects_whole_disk_hfs_plus() {
        let buf = build_hfs_image(64, 512, HFS_PLUS_MAGIC);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::HfsPlus);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 64);
    }

    #[test]
    fn detects_whole_disk_hfsx() {
        let buf = build_hfs_image(64, 512, HFSX_MAGIC);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::HfsX);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 64 * 512];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_image_smaller_than_volume_header() {
        let buf = vec![0u8; HFS_VOLUME_HEADER_OFFSET];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }
}
