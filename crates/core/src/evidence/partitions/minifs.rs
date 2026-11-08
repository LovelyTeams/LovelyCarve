//! MINIFS signature recognition. The native reader validates tables and chunks.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub struct MinifsProbe;

impl VolumeProbe for MinifsProbe {
    fn name(&self) -> &'static str {
        "minifs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        Ok(::minifs::has_magic(context.head).then(|| VolumeProbeHit {
            probe: "minifs".into(),
            kind: PartitionKind::Minifs,
            confidence: 100,
            evidence: vec!["16-byte MINIFS signature at volume offset 0".into()],
            relative_offset: 0,
        }))
    }
}
