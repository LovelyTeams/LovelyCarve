//! Validated labels, metadata descriptors and committed circular-buffer copies.
use anyhow::{bail, Context, Result};

use super::text::{parse_volume_group, VolumeGroup};
use crate::evidence::partitions::SectorSource;

const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Area {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct PhysicalVolumeHeader {
    pub label_offset: u64,
    /// Compact 32-byte on-disk identifier, without formatting hyphens.
    pub identifier: String,
    pub size_bytes: u64,
    pub data_areas: Vec<Area>,
    pub metadata_areas: Vec<Area>,
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub header: PhysicalVolumeHeader,
    pub volume_group: VolumeGroup,
    pub diagnostics: Vec<String>,
    /// Original PV-relative ranges, in logical text order (two on ring wrap).
    pub text_ranges: Vec<Area>,
}

/// LVM weak CRC-32: seed 0xf597a6cf, reflected polynomial, no final XOR.
pub fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = 0xf597_a6cfu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .context("LVM truncated header")?
            .try_into()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("LVM truncated header")?
            .try_into()?,
    ))
}

fn read_exact(
    source: &dyn SectorSource,
    partition_offset: u64,
    partition_size: u64,
    offset: u64,
    size: usize,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(size as u64)
        .context("LVM read range overflow")?;
    let absolute = partition_offset
        .checked_add(offset)
        .context("LVM partition offset overflow")?;
    if end > partition_size {
        bail!("LVM read outside physical volume");
    }
    let bytes = source.read(absolute, size)?;
    if bytes.len() != size {
        bail!("LVM short source read at {absolute:#x}");
    }
    Ok(bytes)
}

/// Validate a label at any of the first four *512-byte* sectors.
pub fn parse_label(
    bytes: &[u8],
    label_offset: u64,
    partition_size: u64,
) -> Result<PhysicalVolumeHeader> {
    if label_offset >= 2048
        || !label_offset.is_multiple_of(512)
        || bytes.len() != 512
        || bytes.get(..8) != Some(b"LABELONE")
        || bytes.get(24..32) != Some(b"LVM2 001")
    {
        bail!("LVM label signature mismatch");
    }
    if u64_at(bytes, 8)? != label_offset / 512 || u32_at(bytes, 16)? != checksum(&bytes[20..]) {
        bail!("LVM label sector number or checksum mismatch");
    }
    let offset = u32_at(bytes, 20)? as usize;
    if !(32..=440).contains(&offset) {
        bail!("LVM invalid PV header offset");
    }
    let identifier = bytes
        .get(offset..offset + 32)
        .context("LVM truncated PV identifier")?;
    if !identifier.iter().all(u8::is_ascii_alphanumeric) {
        bail!("LVM invalid PV identifier");
    }
    let size_bytes = u64_at(bytes, offset + 32)?;
    if size_bytes == 0 || size_bytes > partition_size {
        bail!("LVM PV size exceeds source partition");
    }
    let mut position = offset + 40;
    let mut lists = Vec::new();
    for metadata in [false, true] {
        let mut areas = Vec::new();
        loop {
            let area_offset = u64_at(bytes, position)?;
            let size = u64_at(bytes, position + 8)?;
            position += 16;
            if area_offset == 0 && size == 0 {
                break;
            }
            if area_offset == 0
                || area_offset >= size_bytes
                || (metadata && size < 512)
                || area_offset
                    .checked_add(size)
                    .is_none_or(|end| end > size_bytes)
            {
                bail!("LVM invalid area descriptor");
            }
            areas.push(Area {
                offset: area_offset,
                size,
            });
        }
        lists.push(areas);
    }
    let metadata_areas = lists.pop().context("LVM missing metadata descriptors")?;
    let data_areas = lists.pop().context("LVM missing data descriptors")?;
    if data_areas.is_empty() {
        bail!("LVM missing data area");
    }
    Ok(PhysicalVolumeHeader {
        label_offset,
        identifier: std::str::from_utf8(identifier)?.to_owned(),
        size_bytes,
        data_areas,
        metadata_areas,
    })
}

/// Read verified current metadata, including backup MDAs beyond a head probe.
/// Invalid copies are reported; historical ring bytes never become active data.
pub fn read_metadata(
    source: &dyn SectorSource,
    partition_offset: u64,
    partition_size: u64,
) -> Result<Metadata> {
    if partition_offset
        .checked_add(partition_size)
        .is_none_or(|end| end > source.size())
    {
        bail!("LVM partition exceeds source");
    }
    let head = read_exact(
        source,
        partition_offset,
        partition_size,
        0,
        partition_size.min(2048) as usize,
    )?;
    let mut diagnostics = Vec::new();
    let mut header = None;
    for sector in 0..4 {
        let start = sector * 512;
        let Some(label) = head.get(start..start + 512) else {
            break;
        };
        if label.get(..8) != Some(b"LABELONE") {
            continue;
        }
        match parse_label(label, start as u64, partition_size) {
            Ok(parsed) => {
                header = Some(parsed);
                break;
            }
            Err(error) => diagnostics.push(format!("label at {start:#x}: {error:#}")),
        }
    }
    let header = header.with_context(|| {
        format!(
            "LVM valid physical volume label not found: {}",
            diagnostics.join("; ")
        )
    })?;
    let mut selected: Option<(VolumeGroup, Vec<Area>)> = None;
    for area in &header.metadata_areas {
        match read_metadata_area(source, partition_offset, partition_size, area) {
            Ok(Some((group, ranges))) => {
                let identifier = header.identifier.as_str();
                if !group
                    .physical_volumes
                    .iter()
                    .any(|pv| pv.identifier.replace('-', "") == identifier)
                {
                    diagnostics.push(format!(
                        "MDA at {:#x} does not contain label PV UUID",
                        area.offset
                    ));
                    continue;
                }
                if let Some((previous, _)) = &selected {
                    if previous.identifier != group.identifier {
                        bail!("LVM metadata copies disagree on VG identity");
                    }
                    if previous.sequence_number == group.sequence_number && previous != &group {
                        bail!("LVM conflicting metadata copies at the same sequence number");
                    }
                    if previous.sequence_number >= group.sequence_number {
                        continue;
                    }
                }
                selected = Some((group, ranges));
            }
            Ok(None) => diagnostics.push(format!(
                "MDA at {:#x} has no committed active copy",
                area.offset
            )),
            Err(error) => diagnostics.push(format!("MDA at {:#x}: {error:#}", area.offset)),
        }
    }
    let (volume_group, text_ranges) = selected.with_context(|| {
        format!(
            "LVM no valid committed metadata: {}",
            diagnostics.join("; ")
        )
    })?;
    let pv = volume_group
        .physical_volumes
        .iter()
        .find(|pv| pv.identifier.replace('-', "") == header.identifier)
        .context("LVM current PV missing from metadata")?;
    let start = pv
        .pe_start_sectors
        .checked_mul(512)
        .context("LVM PE start overflow")?;
    let end = pv
        .extent_count
        .checked_mul(volume_group.extent_size_sectors)
        .and_then(|size| size.checked_mul(512))
        .and_then(|size| start.checked_add(size))
        .context("LVM PE area overflow")?;
    if header.data_areas.len() != 1 {
        bail!("LVM multiple data areas are unsupported");
    }
    let data_area = &header.data_areas[0];
    let data_end = if data_area.size == 0 {
        header.size_bytes
    } else {
        data_area
            .offset
            .checked_add(data_area.size)
            .context("LVM data area overflow")?
    };
    if start != data_area.offset
        || end > data_end
        || start < 2048
        || pv
            .size_sectors
            .checked_mul(512)
            .is_none_or(|size| size > header.size_bytes || end > size)
        || header
            .metadata_areas
            .iter()
            .any(|area| area.offset < end && area.offset + area.size > start)
    {
        bail!("LVM text PE geometry disagrees with physical volume header");
    }
    Ok(Metadata {
        header,
        volume_group,
        diagnostics,
        text_ranges,
    })
}

fn read_metadata_area(
    source: &dyn SectorSource,
    partition_offset: u64,
    partition_size: u64,
    area: &Area,
) -> Result<Option<(VolumeGroup, Vec<Area>)>> {
    let header = read_exact(source, partition_offset, partition_size, area.offset, 512)?;
    if header.get(4..20) != Some(b" LVM2 x[5A%r0N*>")
        || u32_at(&header, 20)? != 1
        || u64_at(&header, 24)? != area.offset
        || u64_at(&header, 32)? != area.size
        || u32_at(&header, 0)? != checksum(&header[4..])
    {
        bail!("LVM metadata area header identity, version or checksum mismatch");
    }
    // raw_locns[0] is the committed copy. Later entries may be precommitted
    // transactions: never choose them simply because they have a larger seqno.
    let offset = u64_at(&header, 40)?;
    let size = u64_at(&header, 48)?;
    let flags = u32_at(&header, 60)?;
    if flags & !1 != 0 {
        bail!("LVM unknown metadata location flags {flags:#x}");
    }
    if flags & 1 != 0 || (offset == 0 && size == 0) {
        return Ok(None);
    }
    if offset < 512
        || offset >= area.size
        || size == 0
        || size > MAX_METADATA_BYTES
        || size > area.size - 512
    {
        bail!("LVM active metadata range invalid or exceeds limit");
    }
    let first_size = size.min(area.size - offset);
    let first_offset = area
        .offset
        .checked_add(offset)
        .context("LVM metadata offset overflow")?;
    let mut ranges = vec![Area {
        offset: first_offset,
        size: first_size,
    }];
    let mut bytes = read_exact(
        source,
        partition_offset,
        partition_size,
        first_offset,
        usize::try_from(first_size)?,
    )?;
    if first_size < size {
        let second_offset = area
            .offset
            .checked_add(512)
            .context("LVM ring offset overflow")?;
        ranges.push(Area {
            offset: second_offset,
            size: size - first_size,
        });
        bytes.extend(read_exact(
            source,
            partition_offset,
            partition_size,
            second_offset,
            usize::try_from(size - first_size)?,
        )?);
    }
    if u32_at(&header, 56)? != checksum(&bytes) {
        bail!("LVM active metadata checksum mismatch");
    }
    Ok(Some((parse_volume_group(&bytes)?, ranges)))
}
