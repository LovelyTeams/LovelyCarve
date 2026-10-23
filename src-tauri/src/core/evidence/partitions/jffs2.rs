//! CRC-checked new-format JFFS2 common-header recognition.
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};
use anyhow::Result;

pub fn is_jffs2_magic(bytes: &[u8]) -> bool {
    matches!(bytes.get(..2), Some([0x85, 0x19] | [0x19, 0x85]))
}
pub(super) struct Jffs2Probe;
impl VolumeProbe for Jffs2Probe {
    fn name(&self) -> &'static str {
        "jffs2"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if !is_jffs2_magic(context.head) {
            return Ok(None);
        }
        jffs2::validate_header(context.head)?;
        Ok(Some(VolumeProbeHit {
            probe: "jffs2".into(),
            kind: PartitionKind::Jffs2,
            confidence: 100,
            evidence: vec!["JFFS2 common header with verified CRC".into()],
            relative_offset: 0,
        }))
    }
}
