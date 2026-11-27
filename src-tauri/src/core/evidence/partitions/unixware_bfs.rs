//! Probe the UnixWare Boot File System, distinct from the Be File System.

use unixware_bfs_reader::{Header, HEADER_SIZE};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(crate) struct UnixwareBfsProbe;

impl VolumeProbe for UnixwareBfsProbe {
    fn name(&self) -> &'static str {
        "unixwarebfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if ctx.size < HEADER_SIZE as u64 {
            return Ok(None);
        }
        Ok(
            Header::inspect(ctx.head, ctx.size)?.map(|header| VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::UnixwareBfs,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "UnixWare BFS volume {}, {} inode slots, interrupted compaction={}",
                    header.volume, header.inode_count, header.interrupted_compaction
                )],
            }),
        )
    }
}
