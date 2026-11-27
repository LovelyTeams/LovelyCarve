use hammer_reader::Header;

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct HammerProbe;

impl VolumeProbe for HammerProbe {
    fn name(&self) -> &'static str {
        "hammer"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        Ok(Header::parse(ctx.head, ctx.size)?.map(|header| {
            let mut evidence = vec![format!(
                "HAMMER v{} volume {}/{}, {:?} byte order, label {:?}",
                header.version, header.volume_id, header.volumes, header.order, header.label
            )];
            if header.checksum_present {
                evidence.push("verified HAMMER volume header CRC".into());
            }
            VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::Hammer,
                confidence: if header.checksum_present { 100 } else { 95 },
                relative_offset: 0,
                evidence,
            }
        }))
    }
}
