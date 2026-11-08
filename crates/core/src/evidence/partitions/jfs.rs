//! Native JFS probes at the actual primary/secondary superblock offsets.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct JfsProbe;
pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(32768..33280)
        .is_some_and(|bytes| jfs_reader::Header::inspect(bytes).is_ok())
}
impl VolumeProbe for JfsProbe {
    fn name(&self) -> &'static str {
        "jfs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        for offset in [32768u64, 61440] {
            if offset + 512 > context.size {
                continue;
            }
            let bytes = context.source.read(offset, 512)?;
            let Ok(header) = jfs_reader::Header::inspect(&bytes) else {
                continue;
            };
            if header.validate_source(context.size).is_err() {
                continue;
            }
            return Ok(Some(VolumeProbeHit {
                probe: "jfs".into(),
                kind: PartitionKind::Jfs,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "JFS aggregate superblock at {offset}, version {}, {}-byte blocks, state {:#x}",
                    header.version, header.block_size, header.state
                )],
            }));
        }
        Ok(None)
    }
}
