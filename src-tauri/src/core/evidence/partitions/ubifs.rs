//! UBIFS superblock recognition; full geometry/index checks belong to the reader.
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};
use anyhow::{Context, Result};

pub fn is_ubifs_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(&ubidump::filesystem::UBIFS_MAGIC)
}
pub(super) struct UbifsProbe;
impl VolumeProbe for UbifsProbe {
    fn name(&self) -> &'static str {
        "ubifs"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        if !is_ubifs_magic(context.head) {
            return Ok(None);
        }
        let header = context
            .head
            .get(..4096)
            .context("truncated UBIFS superblock")?;
        ubidump::filesystem::validate_superblock(header)?;
        Ok(Some(VolumeProbeHit {
            probe: "ubifs".into(),
            kind: PartitionKind::Ubifs,
            confidence: 100,
            evidence: vec!["UBIFS superblock with verified CRC".into()],
            relative_offset: 0,
        }))
    }
}
