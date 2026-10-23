//! ZFS whole-device detection.
//!
//! ZFS vdevs (whether on a partition or directly on a disk/file) carry four
//! 256 KiB labels: L0 at byte 0, L1 at 0x40000, L2 at end-512K, L3 at
//! end-256K. Each label is laid out as:
//!
//!   * `0x0000 .. 0x2000`   blank
//!   * `0x2000 .. 0x4000`   boot header (mostly blank in practice)
//!   * `0x4000 .. 0x20000`  XDR-encoded nvlist (pool / vdev metadata)
//!   * `0x20000 .. 0x40000` uberblock array (128 entries × 1 KiB)
//!
//! Each uberblock starts with the 8-byte magic `0x00bab10c` written in the
//! host's native byte order at the time of the txg. We confirm L0 by reading
//! the first three uberblocks (offsets `0x20000`, `0x20400`, `0x20800`) and
//! requiring all three to carry the magic — a single hit could be coincidence
//! but three consecutive 1-KiB-aligned hits cannot.
//!
//! Sits at the end of the `PARSERS` table so it only fires when no real
//! partition table claimed the disk. Whole-device ZFS pools occur when the
//! pool was created over a file vdev (`zpool create … /path/to/file`) or
//! against a raw block device with `-f`, neither of which leaves an MBR/GPT
//! behind.

use crate::core::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind};

/// L0 label total size; minimum image we'll inspect.
const LABEL_SIZE: u64 = 256 * 1024;
/// L0 uberblock array starts 128 KiB into the label.
const UBERBLOCK_ARRAY_OFFSET: u64 = 128 * 1024;
/// Each uberblock occupies 1 KiB (the default `ub_shift = 10`).
const UBERBLOCK_STRIDE: u64 = 1024;
/// ZFS uberblock magic stored little-endian (x86/x86_64/aarch64-LE writers).
const UBERBLOCK_MAGIC_LE: [u8; 8] = [0x0c, 0xb1, 0xba, 0x00, 0x00, 0x00, 0x00, 0x00];
/// Big-endian variant — rare today (SPARC), included for completeness.
const UBERBLOCK_MAGIC_BE: [u8; 8] = [0x00, 0x00, 0x00, 0x00, 0x00, 0xba, 0xb1, 0x0c];
/// Number of consecutive uberblocks required to confirm a valid L0.
const UBERBLOCK_PROBE_COUNT: usize = 3;

pub struct ZfsParser;

impl PartParser for ZfsParser {
    fn name(&self) -> &'static str {
        "zfs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || ctx.size < LABEL_SIZE {
            return Ok(None);
        }
        let probe_len = (UBERBLOCK_STRIDE as usize) * UBERBLOCK_PROBE_COUNT;
        let probe = ctx.source.read(UBERBLOCK_ARRAY_OFFSET, probe_len)?;
        if probe.len() < probe_len {
            return Ok(None);
        }
        for i in 0..UBERBLOCK_PROBE_COUNT {
            let off = i * UBERBLOCK_STRIDE as usize;
            let magic = &probe[off..off + 8];
            if magic != UBERBLOCK_MAGIC_LE && magic != UBERBLOCK_MAGIC_BE {
                return Ok(None);
            }
        }
        let length_lba = ctx.size / ctx.sector_size as u64;
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Zfs,
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

    fn build_zfs_image(sectors: usize, sector_size: usize, magic: [u8; 8]) -> Vec<u8> {
        let mut buf = vec![0u8; sectors * sector_size];
        for i in 0..UBERBLOCK_PROBE_COUNT {
            let off = UBERBLOCK_ARRAY_OFFSET as usize + i * UBERBLOCK_STRIDE as usize;
            buf[off..off + 8].copy_from_slice(&magic);
        }
        buf
    }

    #[test]
    fn detects_whole_disk_zfs_little_endian() {
        // 1 MiB image — large enough for one full L0 label (256 KiB).
        let buf = build_zfs_image(2048, 512, UBERBLOCK_MAGIC_LE);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Zfs);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 2048);
    }

    #[test]
    fn detects_whole_disk_zfs_big_endian() {
        let buf = build_zfs_image(2048, 512, UBERBLOCK_MAGIC_BE);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Zfs);
    }

    #[test]
    fn rejects_image_without_magic() {
        let buf = vec![0u8; 1024 * 1024];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_image_smaller_than_label() {
        // Less than 256 KiB — the L0 label can't fit, so we abstain.
        let buf = vec![0u8; 128 * 1024];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_single_stray_magic() {
        // Plant only one magic; uberblocks 2 and 3 stay zero. A coincidental
        // 8-byte match somewhere on the disk must not trigger a ZFS claim.
        let mut buf = vec![0u8; 1024 * 1024];
        let off = UBERBLOCK_ARRAY_OFFSET as usize;
        buf[off..off + 8].copy_from_slice(&UBERBLOCK_MAGIC_LE);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn yields_to_gpt_when_partition_table_present() {
        // Plant uberblock magics AND a real GPT header — GPT runs first in the
        // priority table and must claim the disk.
        let sector = 512usize;
        let mut buf = build_zfs_image(2048, sector, UBERBLOCK_MAGIC_LE);
        buf[510] = 0x55;
        buf[511] = 0xAA;
        buf[sector..sector + 8].copy_from_slice(b"EFI PART");
        buf[sector + 72..sector + 80].copy_from_slice(&2u64.to_le_bytes());
        buf[sector + 80..sector + 84].copy_from_slice(&1u32.to_le_bytes());
        buf[sector + 84..sector + 88].copy_from_slice(&128u32.to_le_bytes());
        let entries_off = sector * 2;
        buf[entries_off..entries_off + 16].copy_from_slice(&[1u8; 16]);
        buf[entries_off + 32..entries_off + 40].copy_from_slice(&100u64.to_le_bytes());
        buf[entries_off + 40..entries_off + 48].copy_from_slice(&199u64.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_lba, 100);
    }
}
