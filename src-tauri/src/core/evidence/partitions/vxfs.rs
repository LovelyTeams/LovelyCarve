//! Veritas VxFS boot-volume probe, separate from XFS and AIX JFS.

use std::borrow::Cow;

use vxfs_reader::{Header, HEADER_SIZE, SUPERBLOCKS};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

pub(crate) struct VxfsProbe;

impl VolumeProbe for VxfsProbe {
    fn name(&self) -> &'static str {
        "vxfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        for (offset, endian) in SUPERBLOCKS {
            if offset + HEADER_SIZE as u64 > ctx.size {
                continue;
            }
            let bytes = match ctx.head.get(offset as usize..offset as usize + HEADER_SIZE) {
                Some(header) => Cow::Borrowed(header),
                None => Cow::Owned(ctx.source.read(offset, HEADER_SIZE)?),
            };
            if let Some(header) = Header::inspect(&bytes, offset, endian, ctx.size)? {
                return Ok(Some(VolumeProbeHit {
                    probe: self.name().into(),
                    kind: PartitionKind::Vxfs,
                    confidence: 100,
                    relative_offset: 0,
                    evidence: vec![format!(
                        "VxFS layout {}, {:?} byte order, {}-byte blocks, clean={}",
                        header.layout, header.endian, header.block_size, header.clean
                    )],
                }));
            }
        }
        Ok(None)
    }
}
