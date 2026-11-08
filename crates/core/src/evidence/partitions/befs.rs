//! BeOS/Haiku BFS recognition, distinct from UnixWare Boot File System.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct BeFsProbe;
pub fn detect(bytes: &[u8]) -> bool {
    [512, 0].into_iter().any(|offset| {
        bytes.get(offset..offset + 132).is_some_and(|raw| {
            befs_reader::Header::inspect_at(raw, offset as u64, u64::MAX).is_ok()
        })
    })
}
impl VolumeProbe for BeFsProbe {
    fn name(&self) -> &'static str {
        "befs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let bytes = if let Some(head) = context.head.get(..644) {
            head.to_vec()
        } else {
            context.source.read(0, 644)?
        };
        for offset in [512usize, 0] {
            let Some(raw) = bytes.get(offset..offset + 132) else {
                continue;
            };
            let Ok(header) = befs_reader::Header::inspect_at(raw, offset as u64, context.size)
            else {
                continue;
            };
            return Ok(Some(VolumeProbeHit {
                probe: "befs".into(),
                kind: PartitionKind::Befs,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "BeFS {:?}, {}-byte blocks, superblock at {offset}, clean={}",
                    header.endian, header.block_size, header.clean
                )],
            }));
        }
        Ok(None)
    }
}
