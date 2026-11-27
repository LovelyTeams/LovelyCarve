//! Bounded APFS member sources within one raw or assembled image.

pub(crate) use super::partition_cursor::PartitionCursor;
use super::partition_cursor::SourceCursor;
use super::{Partition, PartitionKind, SectorSource};
use crate::core::common::error::Failure;
use crate::core::evidence::image::DiskImage;
use anyhow::{ensure, Context, Result};
use apfs::fusion::validate_fusion_pair;
use apfs::superblock::{read_member_nxsb, NxSuperblock};
use apfs::FusionTier;
use std::sync::Arc;

impl PartitionCursor {
    pub(crate) fn superblock(&mut self) -> Result<NxSuperblock> {
        Ok(read_member_nxsb(self)?)
    }
}

/// Fusion set UUID of the APFS container tier on `source`, which both tiers
/// share; `None` for an ordinary container.
pub(crate) fn fusion_set(source: &dyn SectorSource) -> Result<Option<[u8; 16]>> {
    Ok(read_member_nxsb(&mut SourceCursor::new(source))?
        .fusion_member()
        .map(|(id, _)| id))
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
