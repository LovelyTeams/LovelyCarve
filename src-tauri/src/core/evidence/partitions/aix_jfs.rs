//! Original AIX JFS, whose on-disk magic differs from Linux JFS2.

use aix_jfs_reader::Header;

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(crate) struct AixJfsProbe;

impl VolumeProbe for AixJfsProbe {
    fn name(&self) -> &'static str {
        "aixjfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let header = Header::select(ctx.size, |offset, destination| {
            let bytes = ctx
                .source
                .read(offset, destination.len())
                .map_err(|error| lamfold::FoldError::Source(format!("{error:#}")))?;
            if bytes.len() != destination.len() {
                return Err(lamfold::FoldError::Io("short AIX JFS superblock read"));
            }
            destination.copy_from_slice(&bytes);
            Ok(())
        })?;
        Ok(header.map(|header| VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::AixJfs,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "AIX JFS {:?}, {}-byte fragments, superblock at {}, state {}",
                header.layout, header.fragment_size, header.offset, header.state
            )],
        }))
    }
}
