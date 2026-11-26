use ocfs1_reader::{Header, SECTOR_BYTES};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(super) struct Ocfs1Probe;

pub(super) fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(..SECTOR_BYTES)
        .is_some_and(|header| Header::inspect(header, u64::MAX).is_ok())
}

impl VolumeProbe for Ocfs1Probe {
    fn name(&self) -> &'static str {
        "ocfs1"
    }

    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let Some(raw) = context.head.get(..SECTOR_BYTES) else {
            return Ok(None);
        };
        let Ok(header) = Header::inspect(raw, context.size) else {
            return Ok(None);
        };
        Ok(Some(VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Ocfs1,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "OracleCFS 1.2, {} clusters of {} bytes",
                header.clusters, header.cluster_bytes,
            )],
        }))
    }
}
