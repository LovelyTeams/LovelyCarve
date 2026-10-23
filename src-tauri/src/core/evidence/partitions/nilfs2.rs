//! NILFS2 CRC-validated primary/backup recognition; opening checks checkpoint state.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub struct Nilfs2Probe;
pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(1024..2048)
        .is_some_and(|raw| nilfs2_reader::Header::inspect_block(raw, u64::MAX, 1024).is_ok())
}
impl VolumeProbe for Nilfs2Probe {
    fn name(&self) -> &'static str {
        "nilfs2"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if context.size < 20 * 1024 {
            return Ok(None);
        }
        for at in [1024, (context.size / 4096 - 1) * 4096] {
            let cached = usize::try_from(at).ok().and_then(|start| {
                start
                    .checked_add(1024)
                    .and_then(|end| context.head.get(start..end))
            });
            let raw = if let Some(raw) = cached {
                raw.to_vec()
            } else {
                context.source.read(at, 1024)?
            };
            let Ok(header) = nilfs2_reader::Header::inspect_block(&raw, context.size, at) else {
                continue;
            };
            return Ok(Some(VolumeProbeHit {
                probe: "nilfs2".into(),
                kind: PartitionKind::Nilfs2,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "NILFS2 2.0, {}-byte blocks, CRC-valid superblock at {at}, checkpoint {}",
                    header.block_size, header.last_checkpoint
                )],
            }));
        }
        Ok(None)
    }
}
