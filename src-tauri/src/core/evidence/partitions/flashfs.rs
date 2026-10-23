//! Fixed on-disk signatures for EROFS and both F2FS superblock copies.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct FlashFsProbe;
pub fn detect(bytes: &[u8]) -> Option<PartitionKind> {
    if bytes.get(1024..1028) == Some(&0xe0f5e1e2u32.to_le_bytes()) {
        return Some(PartitionKind::Erofs);
    }
    if [1024, 5120]
        .iter()
        .any(|offset| bytes.get(*offset..*offset + 4) == Some(&0xf2f52010u32.to_le_bytes()))
    {
        return Some(PartitionKind::F2fs);
    }
    None
}
impl VolumeProbe for FlashFsProbe {
    fn name(&self) -> &'static str {
        "flashfs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        // Some callers supply only 4 KiB; inspect the backup F2FS superblock too.
        let extra = if context.head.len() < 5124 && context.size >= 5124 {
            Some(context.source.read(0, 8192)?)
        } else {
            None
        };
        Ok(
            detect(extra.as_deref().unwrap_or(context.head)).map(|kind| VolumeProbeHit {
                probe: "flashfs".into(),
                kind,
                confidence: 100,
                relative_offset: 0,
                evidence: vec!["EROFS/F2FS superblock signature".into()],
            }),
        )
    }
}
