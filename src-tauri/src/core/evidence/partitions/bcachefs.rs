//! Checksum-validated primary and layout-listed superblock recognition.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};
use bcachefs_reader::{Layout, Superblock, BCACHEFS_MAGIC, SB_HEADER_BYTES};

pub struct BcachefsProbe;

pub fn detect(bytes: &[u8]) -> bool {
    bytes
        .get(4096..)
        .is_some_and(|raw| Superblock::parse(raw).is_ok())
}

impl VolumeProbe for BcachefsProbe {
    fn name(&self) -> &'static str {
        "bcachefs"
    }

    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if context.size < 4096 + SB_HEADER_BYTES as u64 {
            return Ok(None);
        }
        let mut copies = vec![8];
        if let Some(raw) = context.head.get(3584..4096) {
            if let Ok(layout) = Layout::parse(raw) {
                copies.extend(layout.sb_offsets);
            }
        }
        copies.sort_unstable();
        copies.dedup();
        let mut best: Option<Superblock> = None;
        for sector in copies {
            let Some(offset) = sector.checked_mul(512) else {
                continue;
            };
            if offset
                .checked_add(SB_HEADER_BYTES as u64)
                .is_none_or(|end| end > context.size)
            {
                continue;
            }
            let header = context.source.read(offset, SB_HEADER_BYTES)?;
            if header.get(24..40) != Some(BCACHEFS_MAGIC.as_slice()) {
                continue;
            }
            let words = u32::from_le_bytes(header[124..128].try_into()?);
            let Some(length) = (words as usize)
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(SB_HEADER_BYTES))
                .filter(|&bytes| bytes <= 1024 * 1024)
            else {
                continue;
            };
            if offset
                .checked_add(length as u64)
                .is_none_or(|end| end > context.size)
            {
                continue;
            }
            let raw = context.source.read(offset, length)?;
            let Ok(header) = Superblock::parse(&raw) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|previous| header.seq >= previous.seq)
            {
                best = Some(header);
            }
        }
        Ok(best.map(|header| VolumeProbeHit {
            probe: "bcachefs".into(),
            kind: PartitionKind::Bcachefs,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "bcachefs {}.{}, {}-byte blocks, checksum-valid superblock, sequence {}",
                header.version.major(),
                header.version.minor(),
                u32::from(header.block_size) * 512,
                header.seq
            )],
        }))
    }
}
