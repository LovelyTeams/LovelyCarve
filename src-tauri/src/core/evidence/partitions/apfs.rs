//! Bounded APFS member sources within one raw or assembled image.

use super::{Partition, PartitionKind};
use crate::core::common::error::Failure;
use crate::core::evidence::image::DiskImage;
use anyhow::{ensure, Context, Result};
use apfs::fusion::validate_fusion_pair;
use apfs::superblock::{read_member_nxsb, NxSuperblock};
use apfs::FusionTier;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

pub(crate) struct PartitionCursor {
    image: Arc<DiskImage>,
    pub base_offset: u64,
    pub length_bytes: u64,
    position: u64,
}

impl PartitionCursor {
    pub(crate) fn new(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition
            .start_lba
            .checked_mul(sector_size)
            .context(Failure::corrupt("APFS partition offset overflow"))?;
        let length_bytes = partition
            .length_lba
            .checked_mul(sector_size)
            .context(Failure::corrupt("APFS partition length overflow"))?;
        ensure!(
            base_offset
                .checked_add(length_bytes)
                .is_some_and(|end| end <= image.len()),
            Failure::corrupt("APFS partition exceeds source")
        );
        Ok(Self {
            image,
            base_offset,
            length_bytes,
            position: 0,
        })
    }

    pub(crate) fn superblock(&mut self) -> Result<NxSuperblock> {
        Ok(read_member_nxsb(self)?)
    }
}

pub(crate) fn open_members(
    image: Arc<DiskImage>,
    partition: &Partition,
) -> Result<(PartitionCursor, Option<PartitionCursor>)> {
    let mut selected = PartitionCursor::new(Arc::clone(&image), partition)?;
    let header = selected.superblock()?;
    let Some((id, tier)) = header.fusion_member() else {
        return Ok((selected, None));
    };
    let mut other = None;
    for candidate in image.partitions.iter().filter(|candidate| {
        candidate.id != partition.id && candidate.kind == PartitionKind::ApfsContainer
    }) {
        let mut cursor = PartitionCursor::new(Arc::clone(&image), candidate)?;
        let Ok(candidate_header) = cursor.superblock() else {
            continue;
        };
        let Some((candidate_id, candidate_tier)) = candidate_header.fusion_member() else {
            continue;
        };
        if candidate_id != id {
            continue;
        }
        ensure!(
            candidate_tier != tier && other.is_none(),
            Failure::corrupt("duplicate APFS Fusion member")
        );
        other = Some((cursor, candidate_header));
    }
    let (other, other_header) = other.context(Failure::unsupported(
        "APFS Fusion requires both member devices",
    ))?;
    let ((main, main_header), (tier2, tier2_header)) = if tier == FusionTier::Main {
        ((selected, header), (other, other_header))
    } else {
        ((other, other_header), (selected, header))
    };
    validate_fusion_pair(
        &main_header,
        main.length_bytes,
        &tier2_header,
        tier2.length_bytes,
    )?;
    Ok((main, Some(tier2)))
}

impl Read for PartitionCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = self
            .length_bytes
            .saturating_sub(self.position)
            .min(buf.len() as u64) as usize;
        if want == 0 {
            return Ok(0);
        }
        let bytes = self
            .image
            .read_range(self.base_offset + self.position, want)
            .map_err(io::Error::other)?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length_bytes) + i128::from(delta),
        };
        self.position = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "APFS seek out of range"))?;
        Ok(self.position)
    }
}
