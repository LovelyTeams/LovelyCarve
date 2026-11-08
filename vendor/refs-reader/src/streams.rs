// Ported from refsprogs named stream and linked snapshot data attributes.
use crate::attributes::{attribute_type, read_attributes};
use crate::bytes::{slice, u16_at, u32_at, u64_at, utf16};
use crate::data::DataPlan;
use crate::metadata::BlockRange;
use crate::table::Row;
use crate::{corrupt, BootSector, FileEntry, ReadAt, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamEntry {
    pub name: String,
    pub size: u64,
    pub is_snapshot: bool,
    pub resident: bool,
}

enum Location {
    Resident(Vec<u8>),
    NonResident { stream_id: u64, linked_id: u64 },
}

pub(crate) fn list(
    entry: &FileEntry,
    boot: &BootSector,
    mappings: &[BlockRange],
    source: &dyn ReadAt,
) -> Result<Vec<StreamEntry>> {
    if entry.entry_type != 1 {
        return Ok(Vec::new());
    }
    let attributes = read_attributes(entry, boot, mappings, source)?;
    let mut streams = Vec::new();
    for row in &attributes {
        if attribute_type(row, boot)? == Some(0xB0) {
            let (metadata, _) = descriptor(row, boot)?;
            if streams
                .iter()
                .any(|stream: &StreamEntry| stream.name == metadata.name)
            {
                return Err(corrupt("duplicate named stream"));
            }
            streams.push(metadata);
        }
    }
    Ok(streams)
}

pub(crate) fn open(
    entry: &FileEntry,
    name: &str,
    boot: &BootSector,
    mappings: &[BlockRange],
    source: &dyn ReadAt,
) -> Result<(StreamEntry, DataPlan)> {
    let attributes = read_attributes(entry, boot, mappings, source)?;
    for row in &attributes {
        if attribute_type(row, boot)? != Some(0xB0) {
            continue;
        }
        let (metadata, location) = descriptor(row, boot)?;
        if metadata.name != name {
            continue;
        }
        let plan = match location {
            Location::Resident(bytes) => DataPlan::Resident(bytes),
            Location::NonResident {
                stream_id,
                linked_id,
            } => {
                if metadata.size == 0 {
                    DataPlan::Resident(Vec::new())
                } else if stream_id != 0
                    && attributes.iter().any(|row| {
                        row.key.len() == 0x50
                            && u64_at(&row.key, 0x30).is_ok_and(|id| id == stream_id)
                    })
                {
                    classic_extents(&attributes, stream_id, boot, mappings)?
                } else if linked_id != 0
                    && attributes.iter().any(|row| {
                        attribute_type(row, boot).is_ok_and(|kind| kind == Some(0x80))
                            && row.key.len() >= 24
                            && u64_at(&row.key, 16).is_ok_and(|id| id == linked_id)
                    })
                {
                    DataPlan::parse_data_attributes(
                        &attributes,
                        linked_id,
                        metadata.size,
                        boot,
                        mappings,
                        source,
                    )?
                } else {
                    classic_extents(&attributes, stream_id, boot, mappings)?
                }
            }
        };
        return Ok((metadata, plan));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("ReFS stream {name} not found"),
    ))
}

fn descriptor(row: &Row, boot: &BootSector) -> Result<(StreamEntry, Location)> {
    let start = if boot.modern() { 16 } else { 12 };
    let name = utf16(slice(
        &row.key,
        start,
        row.key
            .len()
            .checked_sub(start)
            .ok_or_else(|| corrupt("truncated stream name"))?,
    )?)?;
    if name.is_empty() {
        return Err(corrupt("empty named stream name"));
    }
    let size = u64_at(&row.value, 32)?;
    let (is_snapshot, location) = if u32_at(&row.value, 0)? & 0x10000000 == 0 {
        let length = usize::try_from(size).map_err(|_| corrupt("resident stream size overflow"))?;
        (
            false,
            Location::Resident(slice(&row.value, 60, length)?.to_vec()),
        )
    } else {
        let stream_id = u64_at(&row.value, 60)?;
        let linked_id = if row.value.len() >= 76 {
            u64_at(&row.value, 68)?
        } else {
            u64::from(u32_at(&row.value, 68)?)
        };
        (
            u16_at(&row.value, 16)? == 2,
            Location::NonResident {
                stream_id,
                linked_id,
            },
        )
    };
    Ok((
        StreamEntry {
            name,
            size,
            is_snapshot,
            resident: matches!(location, Location::Resident(_)),
        },
        location,
    ))
}

fn classic_extents(
    attributes: &[Row],
    stream_id: u64,
    boot: &BootSector,
    mappings: &[BlockRange],
) -> Result<DataPlan> {
    if stream_id == 0 {
        return Err(corrupt("named stream has no usable data reference"));
    }
    let mut extents = Vec::new();
    let mut found = false;
    for row in attributes {
        if row.key.len() != 0x50 || u64_at(&row.key, 0x30)? != stream_id {
            continue;
        }
        // The 0x8 key is a descriptor; the 0x1000 key carries the data map.
        if u64_at(&row.key, 0x38)? == 8 {
            continue;
        }
        found = true;
        let count = u32_at(&row.value, 0x9C)? as usize;
        let bytes = slice(
            &row.value,
            0xB0,
            count
                .checked_mul(24)
                .ok_or_else(|| corrupt("named extent count overflow"))?,
        )?;
        for extent in bytes.as_chunks::<24>().0 {
            crate::extents::append_extent(extent, boot, mappings, &mut extents)?;
        }
    }
    if !found {
        return Err(corrupt(format!(
            "named stream {stream_id:#x} extents missing"
        )));
    }
    extents.sort_by_key(|extent| extent.logical);
    let mut end = 0;
    for extent in &extents {
        if extent.logical < end {
            return Err(corrupt("overlapping named stream extents"));
        }
        end = extent
            .logical
            .checked_add(extent.length)
            .ok_or_else(|| corrupt("named extent logical overflow"))?;
    }
    Ok(DataPlan::Extents(extents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_distinguishes_snapshot_flags_and_bounds_resident_bytes() -> Result<()> {
        let boot = BootSector {
            major: 3,
            minor: 14,
            sector_size: 512,
            cluster_size: 4096,
            volume_size: 409600,
            serial_number: 0,
        };
        let mut key = vec![0; 16];
        for word in "历史:流".encode_utf16() {
            key.extend_from_slice(&word.to_le_bytes());
        }
        let mut row = Row {
            key,
            value: vec![0; 76],
            inode: None,
        };
        row.value[0..4].copy_from_slice(&0x10000000u32.to_le_bytes());
        row.value[16..18].copy_from_slice(&2u16.to_le_bytes());
        row.value[68..76].copy_from_slice(&0x1001u64.to_le_bytes());
        assert_eq!(
            descriptor(&row, &boot)?.0,
            StreamEntry {
                name: "历史:流".into(),
                size: 0,
                is_snapshot: true,
                resident: false
            }
        );
        row.value[16..18].fill(0);
        assert!(!descriptor(&row, &boot)?.0.is_snapshot);
        row.value[0..4].fill(0);
        row.value[32..40].copy_from_slice(&3u64.to_le_bytes());
        row.value[60..63].copy_from_slice(b"abc");
        let (_, Location::Resident(bytes)) = descriptor(&row, &boot)? else {
            return Err(corrupt("expected resident stream"));
        };
        assert_eq!(bytes, b"abc");
        row.value.truncate(62);
        assert!(descriptor(&row, &boot).is_err());
        Ok(())
    }
}
