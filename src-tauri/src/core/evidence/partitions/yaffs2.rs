//! YAFFS2 geometry recognition requires consistent object headers and OOB tags.
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind, SectorSource};
use anyhow::Result;
struct Source<'a>(&'a dyn SectorSource);
impl yaffs2_reader::ReadAt for Source<'_> {
    fn size(&self) -> u64 {
        self.0.size()
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.0.read(offset, length)
    }
}
pub(super) struct Yaffs2Probe;
impl VolumeProbe for Yaffs2Probe {
    fn name(&self) -> &'static str {
        "yaffs2"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        let Some(geometry) = yaffs2_reader::detect(&Source(context.source))? else {
            return Ok(None);
        };
        Ok(Some(VolumeProbeHit {
            probe: "yaffs2".into(),
            kind: PartitionKind::Yaffs2,
            confidence: 99,
            evidence: vec![format!(
                "YAFFS2 page {}, spare {}, tags offset {}, {:?}",
                geometry.page_size, geometry.spare_size, geometry.tags_offset, geometry.order
            )],
            relative_offset: 0,
        }))
    }
}
