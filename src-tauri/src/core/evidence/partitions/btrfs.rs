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

use super::SectorSource;
use crate::core::common::error::Failure;
use anyhow::{bail, ensure, Context};

const SUPER_LEN: usize = 4096;
const DEVICE_ITEM: usize = 201;
const DEVICE_UUID: usize = DEVICE_ITEM + 66;
const MAX_DEVICES: u64 = 256;
const REQUIRED_PHYSICAL_MAP_FEATURES: u64 = (1 << 14) | (1 << 17);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemberHeader {
    pub(crate) fsid: [u8; 16],
    pub(crate) devid: u64,
    pub(crate) uuid: [u8; 16],
    pub(crate) device_bytes: u64,
    pub(crate) num_devices: u64,
    pub(crate) generation: u64,
    root: u64,
    chunk_root: u64,
}

impl MemberHeader {
    pub(crate) fn identifies(superblock: &[u8], fsid: [u8; 16]) -> bool {
        superblock.get(64..72) == Some(BTRFS_MAGIC)
            && superblock.get(32..48) == Some(fsid.as_slice())
    }

    pub(crate) fn parse(superblock: &[u8], available: u64) -> anyhow::Result<Self> {
        ensure!(
            superblock.len() == SUPER_LEN
                && superblock.get(64..72) == Some(b"_BHRfS_M")
                && read_u64(superblock, 48)? == BTRFS_PRIMARY_SUPERBLOCK,
            Failure::corrupt("btrfs: invalid primary superblock")
        );
        let num_devices = read_u64(superblock, 136)?;
        let device_bytes = read_u64(superblock, DEVICE_ITEM + 8)?;
        let devid = read_u64(superblock, DEVICE_ITEM)?;
        ensure!(
            (1..=MAX_DEVICES).contains(&num_devices)
                && devid != 0
                && (BTRFS_PRIMARY_SUPERBLOCK + SUPER_LEN as u64..=available)
                    .contains(&device_bytes),
            Failure::corrupt("btrfs: invalid device identity or length")
        );
        ensure!(
            read_u64(superblock, 188)? & REQUIRED_PHYSICAL_MAP_FEATURES == 0,
            Failure::unsupported("btrfs: allocation requires the stripe or remap tree")
        );
        if num_devices > 1 {
            verify_checksum(superblock)?;
        }
        let fsid = field(superblock, 32)?;
        let uuid = field(superblock, DEVICE_UUID)?;
        ensure!(
            num_devices == 1 || (fsid != [0; 16] && uuid != [0; 16]),
            Failure::corrupt("btrfs: multi-device superblock has no UUID")
        );
        Ok(Self {
            fsid,
            devid,
            uuid,
            device_bytes,
            num_devices,
            generation: read_u64(superblock, 72)?,
            root: read_u64(superblock, 80)?,
            chunk_root: read_u64(superblock, 88)?,
        })
    }

    pub(crate) fn same_commit(self, other: Self) -> bool {
        self.fsid == other.fsid
            && self.generation == other.generation
            && self.num_devices == other.num_devices
            && self.root == other.root
            && self.chunk_root == other.chunk_root
    }
}

fn verify_checksum(superblock: &[u8]) -> anyhow::Result<()> {
    match u16::from_le_bytes(field(superblock, 196)?) {
        0 => {
            ensure!(
                read_u32(superblock, 0)? == superblock_crc32c(&superblock[32..]),
                Failure::corrupt("btrfs: member superblock CRC32C mismatch")
            );
        }
        2 => {
            use sha2::{Digest, Sha256};
            ensure!(
                superblock[..32] == Sha256::digest(&superblock[32..])[..],
                Failure::corrupt("btrfs: member superblock SHA-256 mismatch")
            );
        }
        other => bail!(Failure::unsupported(format!(
            "btrfs: member checksum type {other}"
        ))),
    }
    Ok(())
}

pub(crate) fn superblock_crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub(crate) fn read_member(
    source: &dyn SectorSource,
    base: u64,
    length: u64,
) -> anyhow::Result<MemberHeader> {
    let superblock = read_member_superblock(source, base, length)?;
    MemberHeader::parse(&superblock, length)
}

pub(crate) fn read_member_superblock(
    source: &dyn SectorSource,
    base: u64,
    length: u64,
) -> anyhow::Result<Vec<u8>> {
    source.read(superblock_offset(base, length)?, SUPER_LEN)
}

pub(crate) fn superblock_offset(base: u64, length: u64) -> anyhow::Result<u64> {
    ensure!(
        length >= BTRFS_PRIMARY_SUPERBLOCK + SUPER_LEN as u64,
        Failure::corrupt("btrfs: device shorter than primary superblock")
    );
    base.checked_add(BTRFS_PRIMARY_SUPERBLOCK)
        .context(Failure::corrupt(
            "btrfs: primary superblock offset overflow",
        ))
}

/// The `N` superblock bytes at `offset`; a superblock too short for them is corrupt.
fn field<const N: usize>(bytes: &[u8], offset: usize) -> anyhow::Result<[u8; N]> {
    bytes
        .get(offset..)
        .and_then(<[u8]>::first_chunk::<N>)
        .copied()
        .context(Failure::corrupt("btrfs: superblock field out of range"))
}

fn read_u64(bytes: &[u8], offset: usize) -> anyhow::Result<u64> {
    field(bytes, offset).map(u64::from_le_bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    field(bytes, offset).map(u32::from_le_bytes)
}

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
            member_of: None,
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
