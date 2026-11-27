//! Detect HPFS with the sector 16 superblock and sector 17 spare block.

use hpfs_reader::{Header, HEADER_LENGTH, HEADER_OFFSET};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(crate) struct HpfsProbe;

impl VolumeProbe for HpfsProbe {
    fn name(&self) -> &'static str {
        "hpfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if ctx.size < HEADER_OFFSET + HEADER_LENGTH as u64 {
            return Ok(None);
        }
        let bytes = ctx.source.read(HEADER_OFFSET, HEADER_LENGTH)?;
        Ok(
            Header::inspect(&bytes, ctx.size)?.map(|header| VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::Hpfs,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "HPFS version {}, functional version {}, {} sectors, {} hotfixes, clean={}",
                    header.version,
                    header.functional_version,
                    header.sectors,
                    header.hotfix_used,
                    header.clean
                )],
            }),
        )
    }
}
