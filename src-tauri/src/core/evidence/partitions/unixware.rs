//! Bounded projection of classic UnixWare VTOC entries inside an MBR partition.

use anyhow::{ensure, Context};

use super::{Partition, PartitionKind, SectorSource};
use crate::core::common::error::Failure;

pub(super) const MBR_TYPE: u8 = 0x63;
const LABEL_SECTOR: u64 = 29;
const SECTOR: u64 = 512;
const HEADER_SIZE: usize = 216;
const SLICE_COUNT: usize = 16;
const SLICE_SIZE: usize = 12;
const LABEL_SIZE: usize = HEADER_SIZE + SLICE_COUNT * SLICE_SIZE;
const DISK_MAGIC: u32 = 0xca5e600d;
const VTOC_MAGIC: u32 = 0x600ddeee;
const VALID: u16 = 0x0200;

pub(super) fn slices(
    source: &dyn SectorSource,
    sector_size: usize,
    owner: &Partition,
) -> anyhow::Result<Vec<Partition>> {
    if sector_size as u64 != SECTOR || owner.length_lba <= LABEL_SECTOR {
        return Ok(Vec::new());
    }
    let absolute_label = owner.start_lba + LABEL_SECTOR;
    let bytes = source
        .read(absolute_label * SECTOR, LABEL_SIZE)
        .with_context(|| format!("reading UnixWare disklabel at LBA {absolute_label}"))?;
    ensure!(
        bytes.len() == LABEL_SIZE,
        Failure::corrupt("UnixWare short disklabel read")
    );
    if le32(&bytes, 4) != DISK_MAGIC || le32(&bytes, 156) != VTOC_MAGIC {
        return Ok(Vec::new());
    }
    if u64::from(le32(&bytes, 36)) != SECTOR || u64::from(le32(&bytes, 40)) != owner.start_lba {
        tracing::warn!(
            absolute_label,
            "UnixWare disklabel geometry disagrees with its MBR partition"
        );
        return Ok(Vec::new());
    }
    let declared = usize::from(le16(&bytes, 172));
    if !(1..=184).contains(&declared) {
        tracing::warn!(
            absolute_label,
            declared,
            "invalid UnixWare VTOC slice count"
        );
        return Ok(Vec::new());
    }
    if declared > SLICE_COUNT {
        tracing::warn!(
            absolute_label,
            declared,
            "UnixWare VTOC extension beyond the first 16 slices is not interpreted"
        );
    }
    let end = owner
        .start_lba
        .checked_add(owner.length_lba)
        .context(Failure::corrupt("UnixWare MBR owner range overflow"))?;
    let mut found = Vec::new();
    for (slot, record) in bytes[HEADER_SIZE..HEADER_SIZE + declared.min(SLICE_COUNT) * SLICE_SIZE]
        .as_chunks::<SLICE_SIZE>()
        .0
        .iter()
        .enumerate()
        .skip(1)
    {
        let tag = le16(record, 0);
        let flags = le16(record, 2);
        if matches!(tag, 0 | 1 | 5 | 6 | 8 | 13) || flags & VALID == 0 {
            continue;
        }
        let start_lba = u64::from(le32(record, 4));
        let length_lba = u64::from(le32(record, 8));
        if length_lba == 0
            || start_lba < owner.start_lba
            || start_lba + length_lba > end
            || (start_lba..start_lba + length_lba).contains(&absolute_label)
        {
            tracing::warn!(
                absolute_label,
                slot,
                start_lba,
                length_lba,
                "UnixWare slice outside usable MBR owner range"
            );
            continue;
        }
        let label = if tag == 9 {
            format!("/stand (UnixWare slice {slot})")
        } else {
            format!("UnixWare slice {slot}")
        };
        found.push(Partition {
            id: owner.id,
            kind: if tag == 3 {
                PartitionKind::Swap
            } else {
                PartitionKind::Unknown
            },
            start_lba,
            length_lba,
            label: Some(label),
            unique_guid: None,
            amiga_geometry: None,
        });
    }
    let mut sorted: Vec<_> = found.iter().collect();
    sorted.sort_unstable_by_key(|slice| slice.start_lba);
    if sorted
        .windows(2)
        .any(|pair| pair[0].start_lba + pair[0].length_lba > pair[1].start_lba)
    {
        tracing::warn!(
            absolute_label,
            "overlapping UnixWare VTOC slices; keeping original MBR range"
        );
        return Ok(Vec::new());
    }
    Ok(found)
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
