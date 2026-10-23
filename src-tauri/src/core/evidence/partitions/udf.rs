//! UDF anchors are outside the cached boot-sector header. Validate complete
//! descriptor tags/CRCs; the reader validates the VDS and physical partition map.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct UdfProbe;
impl VolumeProbe for UdfProbe {
    fn name(&self) -> &'static str {
        "udf"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        for block_size in lamfold_udf::BLOCK_SIZES {
            for location in lamfold_udf::anchor_locations(context.size, block_size) {
                let Ok(location) = u32::try_from(location) else {
                    continue;
                };
                let offset = u64::from(location) * u64::from(block_size);
                if offset + 512 > context.size {
                    continue;
                }
                let bytes = context.source.read(offset, 512)?;
                if lamfold_udf::valid_anchor(&bytes, location) {
                    return Ok(Some(VolumeProbeHit {
                        probe: "udf".into(), kind: PartitionKind::Udf,
                        confidence: 100, relative_offset: 0,
                        evidence: vec![format!("UDF anchor with tag checksum and CRC at {offset:#x}; block size {block_size}")],
                    }));
                }
            }
        }
        Ok(None)
    }
}
