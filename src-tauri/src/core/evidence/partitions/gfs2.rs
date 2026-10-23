//! GFS2 versioned superblock recognition; legacy GFS1 remains separate.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct Gfs2Probe;
pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(65536..65536 + 272)
        .is_some_and(|raw| gfs2_reader::Header::inspect_prefix(raw, u64::MAX).is_ok())
}
impl VolumeProbe for Gfs2Probe {
    fn name(&self) -> &'static str {
        "gfs2"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if context.size < 65536 + 512 {
            return Ok(None);
        }
        let raw = if let Some(raw) = context.head.get(65536..65536 + 272) {
            raw.to_vec()
        } else {
            context.source.read(65536, 272)?
        };
        let Ok(header) = gfs2_reader::Header::inspect_prefix(&raw, context.size) else {
            return Ok(None);
        };
        Ok(Some(VolumeProbeHit {
            probe: "gfs2".into(),
            kind: PartitionKind::Gfs2,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "GFS2 {}, {}-byte blocks, lock table {}",
                header.format, header.block_size, header.lock_table
            )],
        }))
    }
}
