use qnx_reader::EtfsLayout;

use super::qnx::read_flash_probe;
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct EtfsProbe;

impl VolumeProbe for EtfsProbe {
    fn name(&self) -> &'static str {
        "qnxetfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let layout = EtfsLayout::locate(ctx.size, |offset, bytes| {
            read_flash_probe(ctx, offset, bytes)
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
