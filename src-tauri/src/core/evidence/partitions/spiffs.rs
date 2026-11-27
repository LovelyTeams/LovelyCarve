use lamfold::FoldError;
use spiffs_reader::{Geometry, Signature};

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct SpiffsProbe;

impl VolumeProbe for SpiffsProbe {
    fn name(&self) -> &'static str {
        "spiffs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let layout = Geometry::locate(ctx.head, ctx.size, |offset, out| {
            let bytes = ctx.source.read(offset, out.len()).map_err(|error| {
                FoldError::Source(format!("reading SPIFFS lookup blocks: {error:#}"))
            })?;
            if bytes.len() != out.len() {
                return Err(FoldError::Io("short SPIFFS lookup block"));
            }
            out.copy_from_slice(&bytes);
            Ok(())
        })?;
        Ok(layout.map(|layout| {
            let signed = layout.signature != Signature::None;
            VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::Spiffs,
                confidence: if signed { 100 } else { 95 },
                relative_offset: if signed { layout.signature_offset() } else { 0 },
                evidence: vec![format!(
                    "SPIFFS {:?} geometry, {:?} byte order, {} byte pages, {} blocks of {} bytes",
                    layout.signature,
                    layout.order,
                    layout.page_bytes,
                    layout.blocks,
                    layout.block_bytes
                )],
            }
        }))
    }
}
