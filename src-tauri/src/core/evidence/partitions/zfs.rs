//! Probe ZFS leaf labels at both ends of a bounded volume.

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, PartitionSlice, SectorSource,
};
use anyhow::{ensure, Context};
use std::{io, sync::Arc};
use zfs_rs::label::{
    has_label_checksum_magic, has_verified_label, label_offsets, Label, CONFIG_CHECKSUM_OFFSET,
    LABEL_SIZE, UBERBLOCK_OFFSET,
};
use zfs_rs::{read_device_label, RawDevice};

pub(crate) struct PartitionSource {
    image: Arc<DiskImage>,
    pub(crate) base: u64,
    pub(crate) length: u64,
}

impl PartitionSource {
    pub(crate) fn new(image: &Arc<DiskImage>, partition: &Partition) -> anyhow::Result<Self> {
        let sector = image.sector_size as u64;
        let base = partition
            .start_lba
            .checked_mul(sector)
            .context("ZFS member offset overflow")?;
        let length = partition
            .length_lba
            .checked_mul(sector)
            .context("ZFS member length overflow")?;
        ensure!(
            length != 0
                && base
                    .checked_add(length)
                    .is_some_and(|end| end <= image.len()),
            "ZFS member exceeds its source"
        );
        Ok(Self {
            image: Arc::clone(image),
            base,
            length,
        })
    }

    pub(crate) fn label(&self) -> anyhow::Result<Label> {
        Ok(read_device_label(self)?)
    }
}

impl RawDevice for PartitionSource {
    type Block = Vec<u8>;

    fn size_bytes(&self) -> Option<u64> {
        Some(self.length)
    }

    fn read_raw(&self, offset: u64, size: u64) -> io::Result<Self::Block> {
        if offset.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let size = usize::try_from(size).map_err(|_| io::ErrorKind::InvalidInput)?;
        let bytes = self
            .image
            .read_range(self.base + offset, size)
            .map_err(io::Error::other)?;
        if bytes.len() != size {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(bytes)
    }
}

/// Pool GUID in the newest valid label of the leaf device `leaf`.
pub(crate) fn member_pool(leaf: &PartitionSlice<'_>) -> anyhow::Result<u64> {
    Ok(read_device_label(leaf)?.identity.pool_guid)
}

/// A leaf device read through a bounded slice of its parent source, before
/// a disk image exists.
impl RawDevice for PartitionSlice<'_> {
    type Block = Vec<u8>;

    fn size_bytes(&self) -> Option<u64> {
        Some(self.size())
    }

    fn read_raw(&self, offset: u64, size: u64) -> io::Result<Self::Block> {
        if offset.checked_add(size).is_none_or(|end| end > self.size()) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let size = usize::try_from(size).map_err(|_| io::ErrorKind::InvalidInput)?;
        let bytes = self.read(offset, size).map_err(io::Error::other)?;
        if bytes.len() != size {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(bytes)
    }
}

pub(super) fn probe_zfs(ctx: &DetectCtx<'_>) -> anyhow::Result<Option<u64>> {
    for offset in label_offsets(ctx.size).into_iter().flatten() {
        let magic = ctx.source.read(offset + CONFIG_CHECKSUM_OFFSET as u64, 8)?;
        if has_label_checksum_magic(&magic) {
            let label = ctx.source.read(offset, LABEL_SIZE)?;
            if has_verified_label(&label, offset) {
                return Ok(Some(offset + UBERBLOCK_OFFSET as u64));
            }
        }
    }
    Ok(None)
}

pub struct ZfsParser;

impl PartParser for ZfsParser {
    fn name(&self) -> &'static str {
        "zfs"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.sector_size == 0 || probe_zfs(ctx)?.is_none() {
            return Ok(None);
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
            member_of: None,
        }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{detect_partitions, test_support::BufSource};

    use sha2::{Digest, Sha256};

    const ECK: u64 = 0x0210_da7a_b10c_7a11;
    const UBER_MAGIC: u64 = 0x00ba_b10c;

    fn put_word(bytes: &mut [u8], word: u64, big: bool) {
        bytes.copy_from_slice(&if big {
            word.to_be_bytes()
        } else {
            word.to_le_bytes()
        });
    }

    fn sign(bytes: &mut [u8], offset: usize, big: bool) {
        let tail = bytes.len() - 40;
        put_word(&mut bytes[tail..tail + 8], ECK, big);
        put_word(&mut bytes[tail + 8..tail + 16], offset as u64, big);
        let digest = Sha256::digest(&*bytes);
        for (stored, input) in bytes[tail + 8..]
            .chunks_exact_mut(8)
            .zip(digest.chunks_exact(8))
        {
            put_word(stored, u64::from_be_bytes(input.try_into().unwrap()), big);
        }
    }

    fn build_zfs_image(base: usize, stride: usize, big: bool) -> Vec<u8> {
        build_zfs_leaf(base, stride, big, 7, 23)
    }

    /// One verified label of leaf `leaf` in pool `pool`, without a vdev tree.
    fn build_zfs_leaf(base: usize, stride: usize, big: bool, pool: u64, leaf: u64) -> Vec<u8> {
        let mut buf = vec![0; 4 * LABEL_SIZE];
        const CONFIG: usize = 16 * 1024;
        let config = &mut buf[base + CONFIG..base + UBERBLOCK_OFFSET];
        config[0] = 1;
        let mut cursor = 12;
        for (name, guid) in [(b"pool_guid".as_slice(), pool), (b"guid".as_slice(), leaf)] {
            let name_end = 12 + (name.len() + 3) / 4 * 4;
            let length = name_end + 16;
            let pair = &mut config[cursor..cursor + length];
            for (offset, value) in [
                (0, length),
                (4, length),
                (8, name.len()),
                (name_end, 8),
                (name_end + 4, 1),
            ] {
                pair[offset..offset + 4].copy_from_slice(&(value as u32).to_be_bytes());
            }
            pair[12..12 + name.len()].copy_from_slice(name);
            pair[name_end + 8..name_end + 16].copy_from_slice(&guid.to_be_bytes());
            cursor += length;
        }
        sign(config, base + CONFIG, big);
        let offset = base + UBERBLOCK_OFFSET + stride * 3;
        let slot = &mut buf[offset..offset + stride];
        put_word(&mut slot[..8], UBER_MAGIC, big);
        put_word(&mut slot[16..24], 7, big);
        sign(slot, offset, big);
        buf
    }

    #[test]
    fn leaves_of_one_pool_point_at_the_first_member_partition() {
        use crate::core::evidence::partitions::device_sets::mark_members;

        let leaves = [(7, 23), (9, 31), (7, 29)]
            .map(|(pool, leaf)| build_zfs_leaf(0, 4096, false, pool, leaf));
        let mut bytes = Vec::new();
        let mut partitions = Vec::new();
        for (id, leaf) in leaves.iter().enumerate() {
            partitions.push(Partition {
                id: id as u8,
                kind: PartitionKind::Zfs,
                start_lba: bytes.len() as u64 / 512,
                length_lba: leaf.len() as u64 / 512,
                label: None,
                unique_guid: None,
                amiga_geometry: None,
                member_of: None,
            });
            bytes.extend_from_slice(leaf);
        }
        mark_members(&BufSource(bytes), &mut partitions, 512);
        // Pool 9 is a volume of its own; the second leaf of pool 7 belongs to the first.
        assert_eq!(
            partitions
                .iter()
                .map(|partition| partition.member_of)
                .collect::<Vec<_>>(),
            vec![None, None, Some(0)]
        );
    }

    #[test]
    fn detects_whole_disk_zfs_little_endian() {
        // 1 MiB image — large enough for one full L0 label (256 KiB).
        let buf = build_zfs_image(0, 4096, false);
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Zfs);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 2048);
    }

    #[test]
    fn detects_whole_disk_zfs_big_endian() {
        let buf = build_zfs_image(3 * LABEL_SIZE, 8192, true);
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
        let off = UBERBLOCK_OFFSET;
        buf[off..off + 8].copy_from_slice(&UBER_MAGIC.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn rejects_a_valid_zfs_label_moved_to_a_different_offset() {
        let mut buf = build_zfs_image(0, 2048, false);
        buf.copy_within(..LABEL_SIZE, LABEL_SIZE);
        buf[..LABEL_SIZE].fill(0);
        let parts = detect_partitions(&BufSource(buf), 512).expect("detect");
        assert!(parts.is_empty());
    }

    #[test]
    fn yields_to_gpt_when_partition_table_present() {
        // Plant a verified label AND a real GPT header — GPT runs first in the
        // priority table and must claim the disk.
        let sector = 512usize;
        let mut buf = build_zfs_image(0, 4096, false);
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
