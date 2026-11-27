//! Recognize FATX volumes and Xbox hard-disk and memory-unit layouts.

use anyhow::{ensure, Result};
use fatx_reader::{Format, Header, HEADER_BYTES, SECTOR_BYTES};

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartParser, Partition, PartitionKind, TableScheme};
use crate::core::common::error::Failure;

#[cfg(test)]
#[path = "fatx_tests.rs"]
mod tests;

const XBOX: &[(u64, u64, &str)] = &[
    (0x00080000, 0x2ee00000, "Xbox X"),
    (0x2ee80000, 0x2ee00000, "Xbox Y"),
    (0x5dc80000, 0x2ee00000, "Xbox Z"),
    (0x8ca80000, 0x1f400000, "Xbox C"),
    (0xabe80000, 0x1312d6000, "Xbox E"),
    (0x1dd156000, 0, "Xbox F"),
];

const XBOX_360: &[(u64, u64, &str)] = &[
    (0x007ff000, 0, "Xbox 360 Memory Unit"),
    (0x10c080000, 0x0ce30000, "Xbox 360 System Extended"),
    (0x118eb0000, 0x08000000, "Xbox 360 System Auxiliary"),
    (0x120eb0000, 0x10000000, "Xbox 360 Compatibility"),
    (0x130eb0000, 0, "Xbox 360 Content"),
];

pub(crate) struct FatxProbe;

impl VolumeProbe for FatxProbe {
    fn name(&self) -> &'static str {
        "fatx"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        Ok(
            Header::inspect(ctx.head, ctx.size)?.map(|header| VolumeProbeHit {
                probe: self.name().into(),
                kind: PartitionKind::Fatx,
                confidence: 100,
                relative_offset: 0,
                evidence: vec![format!(
                    "{:?} serial {:08X}, {}-byte clusters",
                    header.format, header.volume_id, header.cluster_size
                )],
            }),
        )
    }
}

pub(super) struct XboxDiskParser;

impl PartParser for XboxDiskParser {
    fn name(&self) -> &'static str {
        "xbox_disk"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Xbox
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        if ctx.head.starts_with(b"****PARTINFO****") {
            return Ok(Some(lba48(ctx)?));
        }
        if ctx.head.starts_with(&[0, 2, 0, 0]) {
            let parts = devkit(ctx)?;
            if !parts.is_empty() {
                return Ok(Some(parts));
            }
        }

        let mut parts = Vec::new();
        for (layout, format) in [(XBOX, Format::Fatx), (XBOX_360, Format::Xtaf)] {
            for &(start, length, label) in layout {
                // A memory-unit image has no fixed end. The HDD landmarks
                // take precedence on devices large enough for them.
                if start == 0x007ff000 && ctx.size >= 0x120eb0000 {
                    continue;
                }
                let length = if length == 0 {
                    ctx.size.saturating_sub(start)
                } else {
                    length
                };
                if let Some(part) = detect(
                    ctx,
                    start,
                    length,
                    format,
                    parts.len() as u8,
                    label.to_owned(),
                )? {
                    parts.push(part);
                }
            }
        }
        if parts.len() > 1 {
            require_disjoint(&parts)?;
        }
        Ok((!parts.is_empty()).then_some(parts))
    }
}

fn lba48(ctx: &DetectCtx<'_>) -> Result<Vec<Partition>> {
    let rows = ctx
        .head
        .get(48..496)
        .ok_or_else(|| Failure::corrupt("short Xbox LBA48 table"))?;
    let mut table = Vec::new();
    let mut parts = Vec::new();
    for (slot, row) in rows.as_chunks::<32>().0.iter().enumerate() {
        let flags = u32::from_le_bytes(word(row, 16));
        if flags & 0x8000_0000 == 0 {
            continue;
        }
        let start = u32::from_le_bytes(word(row, 20));
        let length = u32::from_le_bytes(word(row, 24));
        let (start, length) = (
            u64::from(start) * SECTOR_BYTES,
            u64::from(length) * SECTOR_BYTES,
        );
        if length == 0 || start < SECTOR_BYTES || start + length > ctx.size {
            tracing::warn!(slot, start, length, "Xbox LBA48 entry outside the image");
            continue;
        }
        table.push((start, length));
        let raw = row[..16]
            .split(|&byte| byte == 0)
            .next()
            .unwrap_or_default();
        let label = std::str::from_utf8(raw)
            .ok()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let label = label
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Xbox LBA48 partition {}", slot + 1));
        if let Some(part) = detect(ctx, start, length, Format::Fatx, slot as u8, label)? {
            parts.push(part);
        }
    }
    table.sort_unstable();
    ensure!(
        table
            .windows(2)
            .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0),
        Failure::corrupt("overlapping Xbox LBA48 entries")
    );
    Ok(parts)
}

fn devkit(ctx: &DetectCtx<'_>) -> Result<Vec<Partition>> {
    let Some(bytes) = ctx.head.get(8..24) else {
        return Ok(Vec::new());
    };
    let mut parts = Vec::new();
    for (slot, (row, label)) in bytes
        .as_chunks::<8>()
        .0
        .iter()
        .zip(["Xbox 360 Devkit Content", "Xbox 360 Devkit Dashboard"])
        .enumerate()
    {
        let start = u32::from_be_bytes(word(row, 0));
        let length = u32::from_be_bytes(word(row, 4));
        if let Some(part) = detect(
            ctx,
            u64::from(start) * SECTOR_BYTES,
            u64::from(length) * SECTOR_BYTES,
            Format::Xtaf,
            slot as u8,
            label.to_owned(),
        )? {
            parts.push(part);
        }
    }
    require_disjoint(&parts)?;
    Ok(parts)
}

/// Bytes `offset..offset + 4` of a fixed-size partition table row.
fn word<const N: usize>(row: &[u8; N], offset: usize) -> [u8; 4] {
    std::array::from_fn(|index| row[offset + index])
}

fn detect(
    ctx: &DetectCtx<'_>,
    start: u64,
    length: u64,
    format: Format,
    id: u8,
    label: String,
) -> Result<Option<Partition>> {
    let sector = ctx.sector_size as u64;
    if start < SECTOR_BYTES
        || length < 2 * fatx_reader::FAT_START + SECTOR_BYTES
        || !start.is_multiple_of(sector)
        || !length.is_multiple_of(sector)
        || start.checked_add(length).is_none_or(|end| end > ctx.size)
    {
        return Ok(None);
    }
    let bytes = ctx.source.read(start, HEADER_BYTES)?;
    match Header::inspect(&bytes, length) {
        Ok(Some(header)) if header.format == format => (),
        Ok(_) => return Ok(None),
        Err(error) => {
            tracing::warn!(start, length, %error, "unusable FATX volume header at Xbox partition");
            return Ok(None);
        }
    }
    Ok(Some(Partition {
        id,
        kind: PartitionKind::Fatx,
        start_lba: start / sector,
        length_lba: length / sector,
        label: Some(label),
        unique_guid: None,
        amiga_geometry: None,
    }))
}

fn require_disjoint(partitions: &[Partition]) -> Result<()> {
    let mut ranges = partitions
        .iter()
        .map(|p| (p.start_lba, p.length_lba))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    ensure!(
        ranges
            .windows(2)
            .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0),
        Failure::corrupt("overlapping Xbox partitions")
    );
    Ok(())
}
