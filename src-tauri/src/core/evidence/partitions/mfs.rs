//! Macintosh MFS with raw and Disk Copy byte ranges kept on the evidence source.

use lamfold::FoldError;

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};

pub(super) struct MfsProbe;

impl VolumeProbe for MfsProbe {
    fn name(&self) -> &'static str {
        "mfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let header = mfs_reader::Header::locate(ctx.head, ctx.size, |offset, bytes| {
            let read = ctx.source.read(offset, bytes.len()).map_err(|error| {
                FoldError::Source(format!("reading MFS master directory copy: {error:#}"))
            })?;
            if read.len() != bytes.len() {
                return Err(FoldError::Io("short MFS master directory copy"));
            }
            bytes.copy_from_slice(&read);
            Ok(())
        })?;
        Ok(header.map(|header| VolumeProbeHit {
            probe: self.name().into(), kind: PartitionKind::Mfs, confidence: 100,
            relative_offset: header.mdb_offset(), evidence: vec![format!(
                "MFS master directory block, {} allocation bytes per block, Disk Copy 4.2={}, backup={}",
                header.block_size, header.disk_copy(), header.backup()
            )],
        }))
    }
}
