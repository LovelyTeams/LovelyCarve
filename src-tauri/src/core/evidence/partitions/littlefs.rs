use lamfold::FoldError;

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct LittlefsProbe;

impl VolumeProbe for LittlefsProbe {
    fn name(&self) -> &'static str {
        "littlefs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let header = littlefs_reader::Header::locate(ctx.head, ctx.size, |offset, bytes| {
            let read = ctx.source.read(offset, bytes.len()).map_err(|error| {
                FoldError::Source(format!("reading LittleFS metadata anchor: {error:#}"))
            })?;
            if read.len() != bytes.len() {
                return Err(FoldError::Io("short LittleFS metadata anchor"));
            }
            bytes.copy_from_slice(&read);
            Ok(())
        })?;
        Ok(header.map(|header| {
            let sb = header.superblock;
            VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::Littlefs,
                confidence: 100,
                relative_offset: header.anchor_offset,
                evidence: vec![format!(
                    "LittleFS {}.{}, committed metadata CRC, {} blocks of {} bytes",
                    sb.version >> 16,
                    sb.version & 0xffff,
                    sb.block_count,
                    sb.block_size
                )],
            }
        }))
    }
}
