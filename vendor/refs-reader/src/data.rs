// Ported from refsprogs inline $DATA and extent parsing.
use crate::bytes::{slice, u16_at, u64_at};
use crate::metadata::BlockRange;
use crate::{corrupt, BootSector, FileEntry, ReadAt, Result};

pub(crate) use crate::extents::Extent;

pub(crate) enum DataPlan {
    Resident(Vec<u8>),
    Extents(Vec<Extent>),
}

impl DataPlan {
    pub(crate) fn parse(
        entry: &FileEntry,
        boot: &BootSector,
        mappings: &[BlockRange],
        source: &dyn ReadAt,
    ) -> Result<Self> {
        if entry.is_directory() {
            return Err(corrupt("cannot read directory as a file"));
        }
        if let Some(target) = &entry.symlink_target {
            return Ok(Self::Resident(target.as_bytes().to_vec()));
        }
        if entry.size == 0 {
            return Ok(Self::Resident(Vec::new()));
        }
        if entry.entry_type != 1 {
            return Err(corrupt("short file entry data mapping is not implemented"));
        }
        let attributes = crate::attributes::read_attributes(entry, boot, mappings, source)?;
        Self::parse_data_attributes(&attributes, 0x1000, entry.size, boot, mappings, source)
    }

    pub(crate) fn parse_data_attributes(
        attributes: &[crate::table::Row],
        stream_id: u64,
        size: u64,
        boot: &BootSector,
        mappings: &[BlockRange],
        source: &dyn ReadAt,
    ) -> Result<Self> {
        let mut selected = None;
        let mut shared_holder = false;
        for attribute in attributes {
            if crate::attributes::attribute_type(attribute, boot)? != Some(0x80) {
                continue;
            }
            let stream_type = u16_at(&attribute.key, 8)?;
            if stream_type == 0x8002 {
                continue;
            }
            let id = if attribute.key.len() >= 24 {
                u64_at(&attribute.key, 16)?
            } else {
                0
            };
            if id != stream_id && !(stream_id == 0x1000 && id == 0) {
                continue;
            }
            if selected.is_some() {
                return Err(corrupt("duplicate data stream attribute"));
            }
            let plan = if boot.modern() && stream_type == 1 {
                let length = usize::try_from(u64_at(&attribute.value, 32)?)
                    .map_err(|_| corrupt("resident length overflow"))?;
                let data = slice(&attribute.value, 60, length)?.to_vec();
                if data.len() as u64 != size {
                    return Err(corrupt("resident data size differs from stream size"));
                }
                Self::Resident(data)
            } else {
                shared_holder =
                    boot.major >= 3 && crate::bytes::u32_at(&attribute.value, 4)? == 0x10028;
                let list = slice(
                    &attribute.value,
                    0x88,
                    attribute
                        .value
                        .len()
                        .checked_sub(0x88)
                        .ok_or_else(|| corrupt("truncated extent list"))?,
                )?;
                Self::Extents(crate::extents::parse_list(list, boot, mappings, source)?)
            };
            selected = Some(plan);
        }
        let mut selected = selected
            .ok_or_else(|| corrupt(format!("data stream {stream_id:#x} attribute missing")))?;
        if shared_holder {
            if let Self::Extents(extents) = &mut selected {
                // Live data shares unchanged blocks with the newest saved version.
                // A historical version can only inherit from earlier versions.
                let mut holders = Vec::new();
                for row in attributes {
                    if crate::attributes::attribute_type(row, boot)? != Some(0x80)
                        || row.key.len() < 24
                        || u16_at(&row.key, 8)? == 0x8002
                        || crate::bytes::u32_at(&row.value, 4)? != 0x10028
                    {
                        continue;
                    }
                    let id = u64_at(&row.key, 16)?;
                    if id > 0x1000 && (stream_id == 0x1000 || id < stream_id) {
                        holders.push((id, row));
                    }
                }
                holders.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
                for (_, holder) in holders {
                    let list = slice(
                        &holder.value,
                        0x88,
                        holder
                            .value
                            .len()
                            .checked_sub(0x88)
                            .ok_or_else(|| corrupt("truncated shared extent list"))?,
                    )?;
                    let shared = crate::extents::parse_list(list, boot, mappings, source)?;
                    crate::shared_extents::fill_unmapped(extents, &shared, size)?;
                }
            }
        }
        Ok(selected)
    }

    pub(crate) fn read_range(
        &self,
        source: &dyn ReadAt,
        entry: &FileEntry,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize> {
        let count = entry.size.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        match self {
            Self::Resident(bytes) => {
                let start =
                    usize::try_from(offset).map_err(|_| corrupt("resident offset overflow"))?;
                buffer[..count].copy_from_slice(slice(bytes, start, count)?);
            }
            Self::Extents(extents) => {
                let mut cursor = offset;
                let end = offset
                    .checked_add(count as u64)
                    .ok_or_else(|| corrupt("read range overflow"))?;
                let sparse = entry.flags & 0x200 != 0;
                for extent in extents {
                    let extent_end = extent
                        .logical
                        .checked_add(extent.length)
                        .ok_or_else(|| corrupt("extent range overflow"))?;
                    if extent_end <= cursor {
                        continue;
                    }
                    if extent.logical >= end {
                        break;
                    }
                    if extent.logical > cursor {
                        if !sparse {
                            return Err(corrupt("missing extent in non-sparse file"));
                        }
                        buffer[(cursor - offset) as usize..(extent.logical - offset) as usize]
                            .fill(0);
                        cursor = extent.logical;
                    }
                    let piece_end = end.min(extent_end);
                    let output =
                        &mut buffer[(cursor - offset) as usize..(piece_end - offset) as usize];
                    if let Some(physical) = extent.physical {
                        let physical = physical
                            .checked_add(cursor - extent.logical)
                            .ok_or_else(|| corrupt("range physical offset overflow"))?;
                        source.read_exact_at(physical, output)?;
                    } else {
                        output.fill(0);
                    }
                    cursor = piece_end;
                    if cursor == end {
                        break;
                    }
                }
                if cursor < end {
                    if !sparse {
                        return Err(corrupt("missing trailing extent in non-sparse file"));
                    }
                    buffer[(cursor - offset) as usize..count].fill(0);
                }
            }
        }
        Ok(count)
    }
}
