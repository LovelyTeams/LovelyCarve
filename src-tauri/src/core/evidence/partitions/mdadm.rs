//! Linux mdadm RAID superblock detection.
//!
//! mdadm has four superblock layouts, distinguished by where the SB sits on
//! the member device:
//!
//! - v1.2 — sector 8 (byte `0x1000`), the modern default.
//! - v1.1 — sector 0 (byte `0x0`).
//! - v1.0 / v0.90 — towards end-of-device, end-aligned.
//!
//! This parser only handles the head-of-device variants: a v1.0 / v0.90
//! member keeps its data from sector 0, so its partition table or filesystem
//! stays recognisable here and RAID detection probes the superblock at the
//! device end (`raid::mdadm_super`). The probe order checks `0x1000`
//! first because it's the variant that shows up in real-world Linux RAID,
//! including the case that motivated this module — mdadm built on LVM LVs
//! where the v1.2 SB lives at LV offset `0x1000` (see
//! `D:\Code\raid\检材3_RAID0_检测报告.md`). Match emits a single whole-device
//! partition with kind `Mdadm` so the consumer knows to stop and assemble
//! the array before further analysis.

use std::io::Cursor;

use binrw::BinRead;

use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, TableScheme,
};

const MDADM_MAGIC_LE: u32 = 0xA92B4EFC;
const MDADM_MAGIC_BE: u32 = 0xFC4E2BA9;
/// v1.2 lives at byte 0x1000 (sector 8). v1.1 at byte 0.
const MDADM_PROBE_OFFSETS: [usize; 2] = [0x1000, 0];

pub struct MdadmParser;

#[derive(Debug, BinRead)]
#[br(little)]
struct MdadmMagicProbe {
    magic: u32,
}

impl PartParser for MdadmParser {
    fn name(&self) -> &'static str {
        "mdadm"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Mdadm
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        for &off in &MDADM_PROBE_OFFSETS {
            let Some(probe) = ctx
                .head
                .get(off..)
                .and_then(|bytes| MdadmMagicProbe::read(&mut Cursor::new(bytes)).ok())
            else {
                continue;
            };
            let magic = probe.magic;
            if magic == MDADM_MAGIC_LE || magic == MDADM_MAGIC_BE {
                return Ok(Some(vec![Partition {
                    id: 0,
                    kind: PartitionKind::Mdadm,
                    start_lba: 0,
                    length_lba: ctx.size / ctx.sector_size as u64,
                    label: Some("MDADM RAID member".into()),
                    unique_guid: None,
                }]));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{detect_partitions, test_support::BufSource};

    #[test]
    fn detects_mdadm_at_offset_8_le() {
        let mut buf = vec![0u8; 0x4000];
        buf[0x1000..0x1004].copy_from_slice(&MDADM_MAGIC_LE.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Mdadm);
        assert_eq!(parts[0].start_lba, 0);
        assert_eq!(parts[0].length_lba, 32);
    }

    #[test]
    fn detects_mdadm_at_offset_8_be() {
        let mut buf = vec![0u8; 0x4000];
        buf[0x1000..0x1004].copy_from_slice(&MDADM_MAGIC_BE.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Mdadm);
    }

    #[test]
    fn detects_mdadm_at_offset_0_fallback() {
        let mut buf = vec![0u8; 0x4000];
        buf[0..4].copy_from_slice(&MDADM_MAGIC_LE.to_le_bytes());
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].kind, PartitionKind::Mdadm);
    }

    #[test]
    fn rejects_non_mdadm_buffer() {
        let buf = vec![0u8; 0x4000];
        let src = BufSource(buf);
        let parts = detect_partitions(&src, 512).expect("detect");
        assert!(parts.is_empty());
    }
}
