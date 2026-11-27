//! HFS+ / HFSX signatures and the byte window of an embedded HFS+ volume.

use ::hfsplus::volume::{VolumeHeader, VOLUME_HEADER_SIZE};
use anyhow::{ensure, Result};

use crate::core::common::error::Failure;
use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// Byte offset of the HFS+ volume header from the start of the volume.
pub(crate) const HFS_VOLUME_HEADER_OFFSET: usize = 1024;
pub(crate) const HFS_MDB_SIZE: usize = 512;
/// Big-endian "H+" — case-insensitive HFS+.
pub(crate) const HFS_PLUS_MAGIC: [u8; 2] = [0x48, 0x2B];
/// Big-endian "HX" — case-sensitive HFSX.
pub(crate) const HFSX_MAGIC: [u8; 2] = [0x48, 0x58];

const HFS_SECTOR_BYTES: u64 = 512;
const MDB_END: u64 = (HFS_VOLUME_HEADER_OFFSET + HFS_MDB_SIZE) as u64;
const VOLUME_SPARED: u16 = 1 << 9;
const VOLUME_SOFTWARE_LOCK: u16 = 1 << 15;

#[derive(Clone, Copy)]
pub(crate) struct WrappedHfsPlus {
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) signature: u16,
}

impl WrappedHfsPlus {
    pub(crate) fn parse(mdb: &[u8], source_size: u64) -> Result<Option<Self>> {
        if !mdb.starts_with(b"BD") {
            return Ok(None);
        }
        ensure!(mdb.len() >= 130, Failure::corrupt("short HFS wrapper MDB"));
        let word = |offset| u16::from_be_bytes([mdb[offset], mdb[offset + 1]]);
        let signature = word(124);
        if !matches!(signature.to_be_bytes(), HFS_PLUS_MAGIC | HFSX_MAGIC) {
            return Ok(None);
        }
        let attributes = word(10);
        let total_blocks = u64::from(word(18));
        let block_size = u64::from(u32::from_be_bytes([mdb[20], mdb[21], mdb[22], mdb[23]]));
        let allocation_base = u64::from(word(28)) * HFS_SECTOR_BYTES;
        let start = u64::from(word(126));
        let count = u64::from(word(128));
        ensure!(
            attributes & (VOLUME_SPARED | VOLUME_SOFTWARE_LOCK)
                == (VOLUME_SPARED | VOLUME_SOFTWARE_LOCK)
                && block_size >= HFS_SECTOR_BYTES
                && block_size.is_multiple_of(HFS_SECTOR_BYTES)
                && allocation_base >= MDB_END
                && allocation_base + total_blocks * block_size <= source_size
                && start + count <= total_blocks
                && count * block_size >= 5 * HFS_SECTOR_BYTES,
            Failure::corrupt("invalid HFS wrapper allocation geometry")
        );
        Ok(Some(Self {
            offset: allocation_base + start * block_size,
            length: count * block_size,
            signature,
        }))
    }

    pub(crate) fn probe(self, context: &DetectCtx<'_>) -> Result<Option<PartitionKind>> {
        for relative_offset in [
            HFS_VOLUME_HEADER_OFFSET as u64,
            self.length - HFS_VOLUME_HEADER_OFFSET as u64,
        ] {
            let bytes = context
                .source
                .read(self.offset + relative_offset, VOLUME_HEADER_SIZE)?;
            if let Ok(header) = VolumeHeader::from_bytes(&bytes, self.length) {
                return Ok(
                    (header.signature == self.signature).then_some(if header.is_hfsx {
                        PartitionKind::HfsX
                    } else {
                        PartitionKind::HfsPlus
                    }),
                );
            }
        }
        Ok(None)
    }
}

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
            member_of: None,
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
