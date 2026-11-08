//! Fixed Minix superblock probe shared by bare volumes and partition refinement.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct MinixProbe;
pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(1024..1056)
        .is_some_and(|header| minix_reader::Superblock::parse(header).is_ok())
}
impl VolumeProbe for MinixProbe {
    fn name(&self) -> &'static str {
        "minix"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let Some(bytes) = context.head.get(1024..1056) else {
            return Ok(None);
        };
        let Ok(header) = minix_reader::Superblock::parse(bytes) else {
            return Ok(None);
        };
        if header.validate_source(context.size).is_err() {
            return Ok(None);
        }
        Ok(Some(VolumeProbeHit {
            probe: "minix".into(),
            kind: PartitionKind::Minix,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![
                "Minix V1/V2/V3 superblock with validated geometry and bitmap capacity".into(),
            ],
        }))
    }
}
