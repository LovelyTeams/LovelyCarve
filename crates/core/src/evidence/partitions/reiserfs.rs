//! ReiserFS v3 superblock recognition, independent of journal replay support.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct ReiserFsProbe;
pub fn detect(bytes: &[u8]) -> bool {
    [8192, 65536].into_iter().any(|offset| {
        bytes
            .get(offset..offset + 116)
            .is_some_and(|raw| reiserfs_reader::Header::inspect(raw, offset as u64).is_ok())
    })
}
impl VolumeProbe for ReiserFsProbe {
    fn name(&self) -> &'static str {
        "reiserfs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        for offset in [8192u64, 65536] {
            if offset + 116 > context.size {
                continue;
            }
            let bytes = context.source.read(offset, 116)?;
            let Ok(header) = reiserfs_reader::Header::inspect(&bytes, offset) else {
                continue;
            };
            if u64::from(header.blocks) * u64::from(header.block_size) > context.size {
                continue;
            }
            return Ok(Some(VolumeProbeHit {
                probe: "reiserfs".into(),
                kind: PartitionKind::Reiserfs,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "ReiserFS {:?} superblock at {offset}, {}-byte blocks, clean={}",
                    header.format, header.block_size, header.clean
                )],
            }));
        }
        Ok(None)
    }
}
