use reiser4_reader::{Header, MASTER_OFFSET};

use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    DetectCtx, PartitionKind,
};

const FORMAT_OFFSET: u64 = MASTER_OFFSET + 4096;
const MASTER_BYTES: usize = 52;
const FORMAT_BYTES: usize = 88;

pub(super) struct Reiser4Probe;

pub(super) fn detect(bytes: &[u8]) -> bool {
    let master = MASTER_OFFSET as usize;
    let format = FORMAT_OFFSET as usize;
    bytes
        .get(master..master + MASTER_BYTES)
        .zip(bytes.get(format..format + FORMAT_BYTES))
        .is_some_and(|(master, format)| Header::inspect(master, format, u64::MAX).is_ok())
}

impl VolumeProbe for Reiser4Probe {
    fn name(&self) -> &'static str {
        "reiser4"
    }

    fn probe(&self, context: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        if context.size < FORMAT_OFFSET + FORMAT_BYTES as u64 {
            return Ok(None);
        }
        let master = context.source.read(MASTER_OFFSET, MASTER_BYTES)?;
        if !master.starts_with(b"ReIsEr4\0") {
            return Ok(None);
        }
        let format = context.source.read(FORMAT_OFFSET, FORMAT_BYTES)?;
        let Ok(header) = Header::inspect(&master, &format, context.size) else {
            return Ok(None);
        };
        Ok(Some(VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Reiser4,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "Reiser4 4.0.{}, {} 4096-byte blocks, node{}, {} keys",
                header.release,
                header.blocks,
                if header.protected_nodes { 41 } else { 40 },
                if header.large_keys { "large" } else { "short" },
            )],
        }))
    }
}
