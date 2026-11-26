//! Locate the game filesystem in bare XISO and complete Xbox DVD images.

use anyhow::{ensure, Result};
use xdvdfs_reader::{inspect_descriptor, DESCRIPTOR_OFFSET, SECTOR_SIZE, VOLUME_HEADER_MAGIC};

use super::volume_probe::{probe_volume, VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartParser, Partition, PartitionKind, PartitionSlice};
use crate::core::common::error::Failure;

const GAME_BASES: &[(u64, &str)] = &[
    (0x18300000, "XGD1"),
    (0x0fd90000, "XGD2"),
    (0x02080000, "XGD3"),
];

fn volume(ctx: &DetectCtx<'_>, base: u64) -> Result<bool> {
    let at = base + DESCRIPTOR_OFFSET;
    if ctx.size < at + SECTOR_SIZE as u64 {
        return Ok(false);
    }
    let magic = ctx.source.read(at, VOLUME_HEADER_MAGIC.len())?;
    if magic != VOLUME_HEADER_MAGIC {
        return Ok(false);
    }
    let bytes = ctx.source.read(at, SECTOR_SIZE)?;
    ensure!(
        bytes.len() == SECTOR_SIZE,
        Failure::corrupt("XDVDFS short volume descriptor")
    );
    Ok(inspect_descriptor(bytes.as_slice().try_into().unwrap(), ctx.size - base)?.is_some())
}

pub(crate) struct XdvdfsProbe;

impl VolumeProbe for XdvdfsProbe {
    fn name(&self) -> &'static str {
        "xdvdfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        Ok(volume(ctx, 0)?.then(|| VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Xdvdfs,
            confidence: 100,
            relative_offset: DESCRIPTOR_OFFSET,
            evidence: vec!["XDVDFS sector 32 descriptor and root extent".into()],
        }))
    }
}

pub(super) struct XboxDiscParser;

impl PartParser for XboxDiscParser {
    fn name(&self) -> &'static str {
        "xbox_disc"
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        let sector = ctx.sector_size as u64;
        for &(base, layout) in GAME_BASES {
            if !base.is_multiple_of(sector) || !volume(ctx, base)? {
                continue;
            }
            let mut partitions = Vec::new();
            let prefix = PartitionSlice::new(ctx.source, 0, base);
            if let Some(hit) = probe_volume(&prefix, ctx.sector_size)?
                .filter(|hit| matches!(hit.kind, PartitionKind::Cdfs | PartitionKind::Udf))
            {
                partitions.push(Partition {
                    id: 0,
                    kind: hit.kind,
                    start_lba: 0,
                    length_lba: base / sector,
                    label: Some("Xbox DVD video".into()),
                    unique_guid: None,
                    amiga_geometry: None,
                });
            }
            partitions.push(Partition {
                id: u8::from(!partitions.is_empty()),
                kind: PartitionKind::Xdvdfs,
                start_lba: base / sector,
                length_lba: (ctx.size - base) / sector,
                label: Some(format!("Xbox {layout}")),
                unique_guid: None,
                amiga_geometry: None,
            });
            return Ok(Some(partitions));
        }
        Ok(None)
    }
}
