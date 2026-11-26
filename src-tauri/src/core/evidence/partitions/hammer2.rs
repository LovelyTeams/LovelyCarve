use hammer2_reader::Header;
use lamfold::FoldError;

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct Hammer2Probe;

impl VolumeProbe for Hammer2Probe {
    fn name(&self) -> &'static str {
        "hammer2"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let header = Header::locate(ctx.head, ctx.size, |offset, out| {
            let bytes = ctx.source.read(offset, out.len()).map_err(|error| {
                FoldError::Source(format!("reading HAMMER2 volume headers: {error:#}"))
            })?;
            if bytes.len() != out.len() {
                return Err(FoldError::Io("short HAMMER2 volume header"));
            }
            out.copy_from_slice(&bytes);
            Ok(())
        })?;
        Ok(header.map(|header| VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Hammer2,
            confidence: 100,
            relative_offset: header.offset,
            evidence: vec![format!(
                "HAMMER2 v{} volume {}/{}, verified CRC32C, transaction {}, {:?} byte order",
                header.version,
                header.volume_id,
                header.volume_count,
                header.mirror_tid,
                header.order
            )],
        }))
    }
}
