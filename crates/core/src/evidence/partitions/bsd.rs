//! BSD disklabel detection.
//!
//! Validates two magic values: `0x82564557` at the start of the
//! second sector and again 132 bytes into the disklabel structure.

use std::io::{Cursor, SeekFrom};

use binrw::BinRead;

use crate::evidence::partitions::{DetectCtx, PartParser, Partition, PartitionKind, TableScheme};

const BSD_DISKLABEL_MAGIC: u32 = 0x82564557;

pub struct BsdParser;

#[derive(Debug, BinRead)]
#[br(little)]
struct BsdDisklabelProbe {
    magic1: u32,
    #[br(seek_before = SeekFrom::Start(132))]
    magic2: u32,
}

impl PartParser for BsdParser {
    fn name(&self) -> &'static str {
        "bsd"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Bsd
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        let head = ctx.head;
        let sector_size = ctx.sector_size;
        if head.len() < sector_size + 148 {
            return Ok(None);
        }
        let label_start = sector_size;
        let Some(probe) = BsdDisklabelProbe::read(&mut Cursor::new(&head[label_start..])).ok()
        else {
            return Ok(None);
        };
        if probe.magic1 != BSD_DISKLABEL_MAGIC || probe.magic2 != BSD_DISKLABEL_MAGIC {
            return Ok(None);
        }
        Ok(Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Bsd,
            start_lba: 0,
            length_lba: ctx.size / sector_size as u64,
            label: Some("BSD disklabel".into()),
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
    fn detects_whole_disk_bsd_disklabel() {
        let sector = 512usize;
        let mut buf = vec![0u8; sector * 4];
        buf[sector..sector + 4].copy_from_slice(&BSD_DISKLABEL_MAGIC.to_le_bytes());
        buf[sector + 132..sector + 136].copy_from_slice(&BSD_DISKLABEL_MAGIC.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Bsd);
    }
}
