//! Structural QNX4 root and QNX6 endian-aware superblock signatures.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
pub struct QnxProbe;
pub fn detect(bytes: &[u8]) -> Option<PartitionKind> {
    for offset in [8192, 0] {
        if matches!(
            bytes.get(offset..offset + 4),
            Some([0x22, 0x11, 0x19, 0x68]) | Some([0x68, 0x19, 0x11, 0x22])
        ) {
            return Some(PartitionKind::Qnx6);
        }
    }
    let root = bytes.get(512..576)?;
    if &root[..2] == b"/\0"
        && u16::from_le_bytes([root[50], root[51]]) & 0xf000 == 0x4000
        && root[63] & 1 != 0
    {
        return Some(PartitionKind::Qnx4);
    }
    None
}
impl VolumeProbe for QnxProbe {
    fn name(&self) -> &'static str {
        "qnx"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let mut kind = detect(context.head);
        if kind.is_none() && context.size >= 8196 {
            let superblock = context.source.read(8192, 4)?;
            if detect(&superblock) == Some(PartitionKind::Qnx6) {
                kind = Some(PartitionKind::Qnx6);
            }
        }
        if kind.is_none() && context.size >= 8192 {
            let backup = context.source.read(context.size - 4096, 4)?;
            if detect(&backup) == Some(PartitionKind::Qnx6) {
                kind = Some(PartitionKind::Qnx6);
            }
        }
        Ok(kind.map(|kind| VolumeProbeHit {
            probe: "qnx".into(),
            kind,
            confidence: 100,
            relative_offset: 0,
            evidence: vec!["QNX root/superblock structure; reader validates volume".into()],
        }))
    }
}
