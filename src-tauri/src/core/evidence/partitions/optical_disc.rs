//! Recognize raw optical tracks before disk partition tables. Their physical
//! sector geometry is not a valid MBR/GPT logical block size.
use anyhow::Result;

use super::{DetectCtx, PartParser, Partition, PartitionKind};
use crate::core::evidence::readers::optical_disc::{DataTrackLayout, RAW_SECTOR_BYTES};

pub struct OpticalDiscParser;

impl PartParser for OpticalDiscParser {
    fn name(&self) -> &'static str {
        "optical_disc"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        if ctx.sector_size != RAW_SECTOR_BYTES {
            return Ok(None);
        }
        Ok(DataTrackLayout::probe(ctx.source)?.map(|layout| {
            vec![Partition {
                id: 0,
                kind: PartitionKind::Cdfs,
                start_lba: 0,
                length_lba: layout.physical_length() / RAW_SECTOR_BYTES as u64,
                label: None,
                unique_guid: None,
            }]
        }))
    }
}
