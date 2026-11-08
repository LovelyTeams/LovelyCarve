//! OCFS2 full-superblock recognition; OCFS1 is a distinct format.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub struct Ocfs2Probe;

pub fn detect(bytes: &[u8]) -> bool {
    [512, 1024, 2048, 4096].into_iter().any(|block_size| {
        let start = block_size as usize * 2;
        bytes
            .get(start..start + block_size as usize)
            .is_some_and(|raw| {
                ocfs2_reader::Header::inspect_block(raw, block_size, u64::MAX).is_ok()
            })
    })
}

impl VolumeProbe for Ocfs2Probe {
    fn name(&self) -> &'static str {
        "ocfs2"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        for block_size in [512, 1024, 2048, 4096] {
            let start = block_size as usize * 2;
            if context.size < (start + block_size as usize) as u64 {
                continue;
            }
            let raw = if let Some(raw) = context.head.get(start..start + block_size as usize) {
                raw.to_vec()
            } else {
                context.source.read(start as u64, block_size as usize)?
            };
            let Ok(header) = ocfs2_reader::Header::inspect_block(&raw, block_size, context.size)
            else {
                continue;
            };
            return Ok(Some(VolumeProbeHit {
                probe: "ocfs2".into(),
                kind: PartitionKind::Ocfs2,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "OCFS2 0.90, {}-byte blocks, {}-byte clusters, {} slots",
                    header.block_size, header.cluster_size, header.slots
                )],
            }));
        }
        Ok(None)
    }
}
