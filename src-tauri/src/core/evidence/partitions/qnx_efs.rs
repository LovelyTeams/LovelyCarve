use qnx_reader::EfsLayout;

use super::qnx::read_flash_probe;
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct EfsProbe;

impl VolumeProbe for EfsProbe {
    fn name(&self) -> &'static str {
        "qnxefs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let layout = EfsLayout::locate(ctx.size, |offset, bytes| {
            read_flash_probe(ctx, offset, bytes)
        })?;
        Ok(layout.map(|layout| VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::QnxEfs,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "QNX EFS 3.0 with {} units of {} bytes, {} spare units, {:?} byte order",
                layout.units, layout.unit_size, layout.spares, layout.endian,
            )],
        }))
    }
}
