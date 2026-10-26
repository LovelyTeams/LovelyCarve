//! Classic HFS whole-volume and partition-refinement probe.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct ClassicHfsProbe;
pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(1024..1536)
        .is_some_and(|bytes| classic_hfs::Header::parse(bytes).is_ok())
}
impl VolumeProbe for ClassicHfsProbe {
    fn name(&self) -> &'static str {
        "hfs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let Some(bytes) = context.head.get(1024..1536) else {
            return Ok(None);
        };
        let Ok(header) = classic_hfs::Header::parse(bytes) else {
            return Ok(None);
        };
        if header.validate_source(context.size).is_err() {
            return Ok(None);
        }
        Ok(Some(VolumeProbeHit {
            probe: "hfs".into(),
            kind: PartitionKind::Hfs,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![
                "Classic HFS BD master directory block with bounded allocation geometry".into(),
            ],
        }))
    }
}
