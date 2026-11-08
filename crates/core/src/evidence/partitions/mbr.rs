//! MBR + extended-boot-record detection.
//!
//! Walks the four primary entries at offset 0x1BE, then chains through any
//! extended partition's EBR list to surface logical partitions. Uses an
//! explicit visited-set guard and a follow
//! cap so cyclic EBR chains terminate.

mod metadata;
mod reader;

pub use metadata::{has_boot_signature, ChsAddress, MbrBootRecord, MbrEntry};
pub use reader::MbrPartitionReader;

use anyhow::{ensure, Context};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

use crate::evidence::partitions::{
    volume_probe::probe_volume, DetectCtx, PartParser, Partition, PartitionKind, PartitionSlice,
    SectorSource, TableScheme,
};

const MBR_MAGIC: [u8; 2] = [0x55, 0xAA];
const MBR_PART_TABLE_OFFSET: usize = 0x1BE;
const MBR_PART_ENTRY_SIZE: usize = 16;
const MBR_ENTRY_COUNT: usize = 4;
const MAX_EBR_FOLLOW: usize = 256;

/// Read the 32-bit NT disk signature out of the boot sector. Best effort —
/// `None` when the sector is short or the signature was never stamped (zero).
pub(crate) fn read_disk_signature(head: &[u8]) -> Option<String> {
    let signature = MbrBootRecord::decode(head, 0)?.disk_signature;
    (signature != 0).then(|| format!("0x{signature:08X}"))
}

pub struct MbrParser;

/// Infer geometry only from a structurally valid first EBR. Primary-only MBRs
/// contain no sector-size field, so retain the caller's geometry if ambiguous.
/// Callers with authoritative container/device geometry must not use this.
pub fn infer_sector_size(source: &dyn SectorSource, preferred: usize) -> anyhow::Result<usize> {
    ensure!(
        matches!(preferred, 512 | 1024 | 2048 | 4096),
        "unsupported MBR sector size: {preferred}"
    );
    let head = source.read(0, 512).context("reading MBR geometry probe")?;
    let Some(record) = MbrBootRecord::decode(&head, 0) else {
        return Ok(preferred);
    };
    if !record.has_signature || !record.valid_indicators() {
        return Ok(preferred);
    }
    for sector_size in std::iter::once(preferred).chain(
        [512, 1024, 2048, 4096]
            .into_iter()
            .filter(|size| *size != preferred),
    ) {
        let max_sector = source.size() / sector_size as u64;
        if record
            .entries
            .iter()
            .filter(|entry| entry.is_populated())
            .any(|entry| {
                entry.start_lba == 0
                    || u64::from(entry.start_lba) + u64::from(entry.length_lba) > max_sector
            })
        {
            continue;
        }
        for entry in &record.entries {
            let base = u64::from(entry.start_lba);
            let end = base + u64::from(entry.length_lba);
            if !entry.is_extended() || base == 0 || entry.length_lba == 0 || end > max_sector {
                continue;
            }
            let offset = base
                .checked_mul(sector_size as u64)
                .context("MBR geometry offset overflow")?;
            let bytes = source
                .read(offset, 512)
                .with_context(|| format!("reading EBR geometry probe at {offset}"))?;
            let Some(ebr) = MbrBootRecord::decode(&bytes, base) else {
                continue;
            };
            if ebr.has_signature
                && ebr.valid_indicators()
                && ebr.entries.iter().any(|entry| {
                    entry.is_populated()
                        && entry.start_lba != 0
                        && base + u64::from(entry.start_lba) + u64::from(entry.length_lba) <= end
                })
            {
                return Ok(sector_size);
            }
        }
    }
    Ok(preferred)
}

/// Accepted data partition and its exact on-disk origin. Container entries
/// remain in `MbrTable::records`, not in the browsable partition list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbrPartition {
    pub partition: Partition,
    pub entry: MbrEntry,
    pub record_lba: u64,
    pub slot: u8,
}

/// Read-only MBR analysis. Records preserve all four slots (including empty,
/// extended and rejected entries); warnings describe incomplete recovery.
#[derive(Debug, Clone)]
pub struct MbrTable {
    pub sector_size: usize,
    pub records: Vec<MbrBootRecord>,
    pub partitions: Vec<MbrPartition>,
    pub warnings: Vec<String>,
}

impl MbrTable {
    pub fn read(source: &dyn SectorSource, sector_size: usize) -> anyhow::Result<Option<Self>> {
        Self::read_cancellable(source, sector_size, &CancellationToken::new())
    }

    /// The caller owns cancellation. Blocking traversal polls the token
    /// directly and never publishes a partial result after cancellation.
    pub fn read_cancellable(
        source: &dyn SectorSource,
        sector_size: usize,
        cancel: &CancellationToken,
    ) -> anyhow::Result<Option<Self>> {
        ensure!(!cancel.is_cancelled(), "MBR analysis cancelled");
        let head = source.read(0, 512).context("reading MBR boot record")?;
        parse_table(
            &DetectCtx {
                source,
                head: &head,
                sector_size,
                size: source.size(),
            },
            cancel,
        )
    }

    fn warn(&mut self, message: String) {
        tracing::warn!("{message}");
        self.warnings.push(message);
    }
}

impl PartParser for MbrParser {
    fn name(&self) -> &'static str {
        "mbr"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Mbr
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        Ok(
            parse_table(ctx, &CancellationToken::new())?.and_then(|table| {
                (!table.partitions.is_empty()).then(|| {
                    table
                        .partitions
                        .into_iter()
                        .map(|entry| entry.partition)
                        .collect()
                })
            }),
        )
    }
}

fn parse_table(
    ctx: &DetectCtx<'_>,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<MbrTable>> {
    ensure!(
        matches!(ctx.sector_size, 512 | 1024 | 2048 | 4096),
        "unsupported MBR sector size: {}",
        ctx.sector_size
    );
    let Some(record) = MbrBootRecord::decode(ctx.head, 0) else {
        return Ok(None);
    };
    // Volume boot sectors also end in 0x55AA. Their boot code and text
    // overlap the primary table, and on large images even those bogus
    // LBAs can fall in bounds. Every table slot must have a legal boot
    // indicator before interpreting any bytes as partitions.
    if !record.valid_indicators() {
        return Ok(None);
    }
    let has_magic = record.has_signature;
    if !has_magic && !plausible_unsigned_mbr(ctx)? {
        return Ok(None);
    }
    // Largest addressable sector. A real primary table never starts a
    // partition at or past this; a random sector that merely ends in
    // 0x55AA does (P1-11).
    let max_sector = ctx.size / ctx.sector_size as u64;
    let mut table = MbrTable {
        sector_size: ctx.sector_size,
        records: vec![record.clone()],
        partitions: Vec::new(),
        warnings: Vec::new(),
    };
    if !has_magic {
        table.warn(
            "recovering MBR with missing 0x55AA signature after structural and volume validation"
                .into(),
        );
    }
    let mut extended = Vec::new();

    for (index, parsed) in record.entries.iter().copied().enumerate() {
        ensure!(!cancel.is_cancelled(), "MBR analysis cancelled");
        if !parsed.is_populated() {
            continue;
        }
        // Leading-entry bounds sanity: if either of the first two primary entries
        // starts past the image, reject the whole table so the dispatcher
        // moves on rather than minting garbage from a stray boot sector.
        if index < 2 && u64::from(parsed.start_lba) >= max_sector {
            return Ok(None);
        }
        let start = u64::from(parsed.start_lba);
        let end = start + u64::from(parsed.length_lba);
        if start == 0 || end > max_sector {
            table.warn(format!(
                "MBR slot {index} range {start}..{end} exceeds usable evidence range"
            ));
            continue;
        }
        if parsed.is_extended() {
            extended.push((start, end));
            continue;
        }
        append_partition(&mut table, parsed, 0, index, start)?;
    }
    let mut visited = HashSet::new();
    for (base, end) in extended {
        follow_ebr_chain(ctx.source, base, end, &mut table, &mut visited, cancel)?;
    }
    ensure!(!cancel.is_cancelled(), "MBR analysis cancelled");
    Ok(Some(table))
}

/// Recover a damaged MBR whose final `0x55AA` marker was erased. Without the
/// marker, table-shaped bytes in an ordinary boot sector are too weak to trust,
/// so every populated entry must be structurally valid and at least one normal
/// partition must independently match a supported volume signature.
fn plausible_unsigned_mbr(ctx: &DetectCtx<'_>) -> anyhow::Result<bool> {
    if ctx.sector_size == 0 {
        return Ok(false);
    }
    let max_sector = ctx.size / ctx.sector_size as u64;
    let mut populated = 0usize;
    let mut active = 0usize;
    let mut confirmed_volume = false;
    let mut ranges = Vec::with_capacity(MBR_ENTRY_COUNT);

    for index in 0..MBR_ENTRY_COUNT {
        let base = MBR_PART_TABLE_OFFSET + index * MBR_PART_ENTRY_SIZE;
        let entry = &ctx.head[base..base + MBR_PART_ENTRY_SIZE];
        let status = entry[0];
        if !matches!(status, 0x00 | 0x80) {
            return Ok(false);
        }
        active += usize::from(status == 0x80);

        let type_byte = entry[4];
        let start_lba = u64::from(u32::from_le_bytes(entry[8..12].try_into()?));
        let length_lba = u64::from(u32::from_le_bytes(entry[12..16].try_into()?));
        if type_byte == 0 {
            if entry.iter().any(|byte| *byte != 0) {
                return Ok(false);
            }
            continue;
        }
        let Some(end_lba) = start_lba.checked_add(length_lba) else {
            return Ok(false);
        };
        if start_lba == 0 || length_lba == 0 || end_lba > max_sector {
            return Ok(false);
        }

        populated += 1;
        ranges.push((start_lba, end_lba));
        if !matches!(type_byte, 0x05 | 0x0f | 0x85) {
            let slice = PartitionSlice::new(
                ctx.source,
                start_lba * ctx.sector_size as u64,
                length_lba * ctx.sector_size as u64,
            );
            confirmed_volume |= probe_volume(&slice, ctx.sector_size)?.is_some();
        }
    }

    if populated == 0 || active > 1 || !confirmed_volume {
        return Ok(false);
    }
    ranges.sort_unstable_by_key(|range| range.0);
    Ok(ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0))
}

fn append_partition(
    table: &mut MbrTable,
    entry: MbrEntry,
    record_lba: u64,
    slot: usize,
    start_lba: u64,
) -> anyhow::Result<()> {
    let id = u8::try_from(table.partitions.len())
        .context("MBR has more than 256 partitions; partition IDs cannot represent this table")?;
    table.partitions.push(MbrPartition {
        partition: Partition {
            id,
            kind: mbr_type_to_kind(entry.type_byte),
            start_lba,
            length_lba: u64::from(entry.length_lba),
            label: None,
            unique_guid: None,
            amiga_geometry: None,
        },
        entry,
        record_lba,
        slot: slot as u8,
    });
    Ok(())
}

fn follow_ebr_chain(
    source: &dyn SectorSource,
    extended_base: u64,
    extended_end: u64,
    table: &mut MbrTable,
    visited: &mut HashSet<u64>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut pending = vec![extended_base];
    while let Some(current) = pending.pop() {
        ensure!(!cancel.is_cancelled(), "MBR analysis cancelled");
        if current < extended_base || current >= extended_end {
            table.warn(format!("EBR at LBA {current} is outside extended partition {extended_base}..{extended_end}"));
            continue;
        }
        if visited.contains(&current) {
            table.warn(format!("cyclic or duplicate EBR link at LBA {current}"));
            continue;
        }
        if visited.len() >= MAX_EBR_FOLLOW {
            table.warn(format!(
                "EBR traversal stopped after {MAX_EBR_FOLLOW} records"
            ));
            break;
        }
        visited.insert(current);
        let sector_offset = current
            .checked_mul(table.sector_size as u64)
            .context("EBR offset overflow")?;
        let sector = source
            .read(sector_offset, 512)
            .with_context(|| format!("reading EBR at LBA {current}"))?;
        let Some(record) = MbrBootRecord::decode(&sector, current) else {
            table.warn(format!("short EBR read at LBA {current}"));
            continue;
        };
        let valid = record.has_signature && record.valid_indicators();
        table.records.push(record.clone());
        if !valid {
            table.warn(format!(
                "invalid EBR signature or boot indicator at LBA {current}"
            ));
            continue;
        }

        // Walk all four entries in the EBR rather than only the first data +
        // first link slot (P1-12): some partitioning tools place a second
        // logical partition in slot 2/3, and those were being silently
        // dropped. Data entries are addressed relative to this EBR sector;
        // the link (extended) entry is relative to the primary extended base.
        let mut links = Vec::new();
        for (index, parsed) in record.entries.iter().copied().enumerate() {
            if !parsed.is_populated() {
                continue;
            }
            if parsed.is_extended() {
                let next = extended_base + u64::from(parsed.start_lba);
                if next < extended_end && next + u64::from(parsed.length_lba) <= extended_end {
                    links.push(next);
                } else {
                    table.warn(format!(
                        "EBR link in LBA {current} slot {index} exceeds extended partition"
                    ));
                }
                continue;
            }
            let start_lba = current + u64::from(parsed.start_lba);
            if parsed.start_lba == 0 || start_lba + u64::from(parsed.length_lba) > extended_end {
                table.warn(format!("logical partition in LBA {current} slot {index} exceeds extended partition or overlaps its EBR"));
                continue;
            }
            append_partition(table, parsed, current, index, start_lba)?;
        }
        pending.extend(links.into_iter().rev());
    }
    Ok(())
}

fn mbr_type_to_kind(type_byte: u8) -> PartitionKind {
    match type_byte {
        0x01 | 0x11 => PartitionKind::Fat12,
        0x07 | 0x17 | 0x27 => PartitionKind::Ntfs,
        0x0B | 0x0C | 0x1B | 0x1C => PartitionKind::Fat32,
        0x04 | 0x06 | 0x0E | 0x14 | 0x16 | 0x1E => PartitionKind::Fat16,
        0x05 | 0x0F | 0x85 => PartitionKind::Extended,
        0x82 => PartitionKind::Swap,
        0x83 => PartitionKind::Unknown,
        0x8E => PartitionKind::Lvm,
        0x9F | 0xA5 | 0xA6 | 0xA9 | 0xB7 => PartitionKind::Bsd,
        0xA8 => PartitionKind::Ufs,
        0xAF => PartitionKind::HfsPlus,
        0xFD => PartitionKind::Mdadm,
        _ => PartitionKind::Unknown,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod regressions;
