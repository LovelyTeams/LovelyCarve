use lamfold::FoldError;
use qnx_reader::EtfsLayout;

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct EtfsProbe;

impl VolumeProbe for EtfsProbe {
    fn name(&self) -> &'static str {
        "qnxetfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let layout = EtfsLayout::locate(ctx.size, |offset, bytes| {
            if offset == 0 {
                if let Some(head) = ctx.head.get(..bytes.len()) {
                    bytes.copy_from_slice(head);
                    return Ok(());
                }
            }
            let part = ctx.source.read(offset, bytes.len()).map_err(|error| {
                FoldError::Source(format!("reading QNX ETFS file table: {error:#}"))
            })?;
            if part.len() != bytes.len() {
                return Err(FoldError::Io("short QNX ETFS probe read"));
            }
            bytes.copy_from_slice(&part);
            Ok(())
        })?;
        Ok(layout.map(|layout| VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::QnxEtfs,
            confidence: 95,
            relative_offset: 0,
            evidence: vec![format!(
                "QNX ETFS file table and root with {}-byte clusters, {:?} byte order",
                layout.cluster_size, layout.endian,
            )],
        }))
    }
}
