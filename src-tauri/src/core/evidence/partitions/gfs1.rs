use gfs1_reader::{Header, SUPERBLOCK_BYTES, SUPERBLOCK_OFFSET};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(super) struct Gfs1Probe;

pub(super) fn detect(bytes: &[u8]) -> bool {
    let start = SUPERBLOCK_OFFSET as usize;
    bytes
        .get(start..start + SUPERBLOCK_BYTES)
        .is_some_and(|raw| Header::inspect_prefix(raw, u64::MAX).is_ok())
}

impl VolumeProbe for Gfs1Probe {
    fn name(&self) -> &'static str {
        "gfs1"
    }

    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if context.size < SUPERBLOCK_OFFSET + 512 {
            return Ok(None);
        }
        let start = SUPERBLOCK_OFFSET as usize;
        let raw = if let Some(raw) = context.head.get(start..start + SUPERBLOCK_BYTES) {
            raw.to_vec()
        } else {
            context.source.read(SUPERBLOCK_OFFSET, SUPERBLOCK_BYTES)?
        };
        let Ok(header) = Header::inspect_prefix(&raw, context.size) else {
            return Ok(None);
        };
        Ok(Some(VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Gfs1,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "GFS1 1309/1401, {}-byte blocks, lock protocol {}, lock table {}",
                header.block_size, header.lock_protocol, header.lock_table,
            )],
        }))
    }
}
