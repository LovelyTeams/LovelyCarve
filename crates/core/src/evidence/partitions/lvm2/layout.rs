//! Validated linear LV byte maps. No missing member is treated as a hole.
use anyhow::{bail, Context, Result};
use std::io::{self, Read, Seek, SeekFrom};

use super::text::{LogicalVolume, PhysicalVolume, VolumeGroup};
use crate::evidence::partitions::SectorSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteSegment {
    pub logical_offset: u64,
    pub physical_offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearLayout {
    pub size: u64,
    pub segments: Vec<ByteSegment>,
}

/// Resolve every segment against one PV. Other members require VG assembly.
pub fn linear_layout(
    group: &VolumeGroup,
    volume: &LogicalVolume,
    pv: &PhysicalVolume,
    partition_offset: u64,
    partition_size: u64,
) -> Result<LinearLayout> {
    let partition_end = partition_offset
        .checked_add(partition_size)
        .context("LVM partition end overflow")?;
    let extent_bytes = group
        .extent_size_sectors
        .checked_mul(512)
        .context("LVM extent size overflow")?;
    let pe_start = pv
        .pe_start_sectors
        .checked_mul(512)
        .context("LVM PE offset overflow")?;
    let pv_end = pv
        .extent_count
        .checked_mul(extent_bytes)
        .and_then(|size| pe_start.checked_add(size))
        .context("LVM PV extent range overflow")?;
    if pv_end > partition_size
        || pv
            .size_sectors
            .checked_mul(512)
            .is_none_or(|size| pv_end > size)
        || pv
            .size_sectors
            .checked_mul(512)
            .is_none_or(|size| size > partition_size)
    {
        bail!("LVM PV extent geometry exceeds partition");
    }
    let mut segments = Vec::new();
    let mut size = 0u64;
    for segment in &volume.segments {
        if segment.kind != "striped" || segment.stripe_count != 1 || segment.stripes.len() != 1 {
            bail!(
                "LVM {} segment type {} with {} stripes is unsupported",
                volume.name,
                segment.kind,
                segment.stripe_count
            );
        }
        let stripe = &segment.stripes[0];
        if stripe.physical_volume != pv.name {
            bail!(
                "LVM {} requires missing PV {}",
                volume.name,
                stripe.physical_volume
            );
        }
        let extent_end = stripe
            .physical_extent
            .checked_add(segment.extent_count)
            .context("LVM physical extent overflow")?;
        if extent_end > pv.extent_count {
            bail!("LVM {} allocated extents exceed PV", volume.name);
        }
        let logical_offset = segment
            .start_extent
            .checked_mul(extent_bytes)
            .context("LVM logical offset overflow")?;
        if logical_offset != size {
            bail!("LVM segment coverage gap or overlap");
        }
        let physical_offset = stripe
            .physical_extent
            .checked_mul(extent_bytes)
            .and_then(|offset| pe_start.checked_add(offset))
            .and_then(|offset| partition_offset.checked_add(offset))
            .context("LVM physical offset overflow")?;
        let byte_length = segment
            .extent_count
            .checked_mul(extent_bytes)
            .context("LVM segment size overflow")?;
        if physical_offset
            .checked_add(byte_length)
            .is_none_or(|end| end > partition_end)
        {
            bail!("LVM physical segment exceeds partition");
        }
        size = size
            .checked_add(byte_length)
            .context("LVM volume size overflow")?;
        segments.push(ByteSegment {
            logical_offset,
            physical_offset,
            size: byte_length,
        });
    }
    let mut physical_ranges: Vec<_> = segments
        .iter()
        .map(|segment| (segment.physical_offset, segment.size))
        .collect();
    physical_ranges.sort_unstable_by_key(|range| range.0);
    for pair in physical_ranges.windows(2) {
        if pair[0]
            .0
            .checked_add(pair[0].1)
            .is_none_or(|end| end > pair[1].0)
        {
            bail!("LVM {} has overlapping physical extents", volume.name);
        }
    }
    Ok(LinearLayout { size, segments })
}

impl LinearLayout {
    pub fn contiguous_offset(&self) -> Option<u64> {
        let first = self.segments.first()?.physical_offset;
        self.segments
            .iter()
            .all(|segment| {
                first.checked_add(segment.logical_offset) == Some(segment.physical_offset)
            })
            .then_some(first)
    }

    /// Translate a logical range to original source ranges, splitting at PEs.
    pub fn ranges(&self, offset: u64, length: u64) -> Result<Vec<ByteSegment>> {
        let end = offset.checked_add(length).context("LVM range overflow")?;
        if end > self.size {
            bail!("LVM range outside logical volume");
        }
        let mut output = Vec::new();
        let mut covered = 0u64;
        for segment in &self.segments {
            let segment_end = segment
                .logical_offset
                .checked_add(segment.size)
                .context("LVM segment boundary overflow")?;
            let from = offset.max(segment.logical_offset);
            let to = end.min(segment_end);
            if from < to {
                let physical_offset = segment
                    .physical_offset
                    .checked_add(from - segment.logical_offset)
                    .context("LVM source offset overflow")?;
                output.push(ByteSegment {
                    logical_offset: from,
                    physical_offset,
                    size: to - from,
                });
                covered = covered
                    .checked_add(to - from)
                    .context("LVM read length overflow")?;
            }
        }
        if covered != length {
            bail!("LVM incomplete segment map");
        }
        Ok(output)
    }

    /// EOF-clipped read; allocated short reads are errors, never synthesized zeroes.
    pub fn read(&self, source: &dyn SectorSource, offset: u64, length: usize) -> Result<Vec<u8>> {
        if offset >= self.size || length == 0 {
            return Ok(Vec::new());
        }
        let length = (length as u64).min(self.size - offset);
        let mut output = Vec::with_capacity(usize::try_from(length)?);
        for range in self.ranges(offset, length)? {
            let size = usize::try_from(range.size)?;
            let bytes = source.read(range.physical_offset, size)?;
            if bytes.len() != size {
                bail!("LVM short allocated read at {:#x}", range.physical_offset);
            }
            output.extend(bytes);
        }
        Ok(output)
    }
}

/// Independent cursor over a complete LV. Seeking never changes another reader.
pub struct LogicalVolumeReader<'a> {
    source: &'a dyn SectorSource,
    layout: &'a LinearLayout,
    position: u64,
}

impl<'a> LogicalVolumeReader<'a> {
    pub fn new(source: &'a dyn SectorSource, layout: &'a LinearLayout) -> Self {
        Self {
            source,
            layout,
            position: 0,
        }
    }

    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn size(&self) -> u64 {
        self.layout.size
    }
    pub fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.layout.read(self.source, offset, length)
    }
}

impl Read for LogicalVolumeReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes = self
            .read_at(self.position, buffer.len())
            .map_err(io::Error::other)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.position = self
            .position
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("LVM cursor overflow"))?;
        Ok(bytes.len())
    }
}

impl Seek for LogicalVolumeReader<'_> {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let position = match offset {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.layout.size) + i128::from(delta),
        };
        self.position = u64::try_from(position).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "LVM seek outside u64 range")
        })?;
        Ok(self.position)
    }
}
