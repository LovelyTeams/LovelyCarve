//! LUKS encrypted-volume container detection.
//!
//! LUKS is
//! recognised by its 6-byte fixed magic at offset 0; the dispatcher places
//! this parser ahead of GPT/MBR because the magic cannot collide and the
//! device must be unwrapped before any further partition recognition.

use crate::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind, TableScheme};

pub(crate) const LUKS_MAGIC: [u8; 6] = [b'L', b'U', b'K', b'S', 0xBA, 0xBE];

pub struct LuksParser;

impl PartParser for LuksParser {
    fn name(&self) -> &'static str {
        "luks"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Luks
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.head.len() < LUKS_MAGIC.len() || ctx.head[0..LUKS_MAGIC.len()] != LUKS_MAGIC {
            return Ok(None);
        }
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Luks,
            start_lba: 0,
            length_lba: ctx.size / 512,
            label: Some("LUKS container".into()),
            unique_guid: None,
            amiga_geometry: None,
        }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::partitions::{detect_partitions, test_support::BufSource};

    #[test]
    fn detects_luks_container_first() {
        let mut buf = vec![0u8; 4096];
        buf[0..6].copy_from_slice(&LUKS_MAGIC);
        buf[510] = 0x55;
        buf[511] = 0xAA;
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Luks);
        assert_eq!(parts[0].start_lba, 0);
    }
}
