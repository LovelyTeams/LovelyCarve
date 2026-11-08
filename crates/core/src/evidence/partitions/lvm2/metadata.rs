//! Verified metadata compatibility helpers and contiguous LV geometry.
//! Shared by native volume browsing and cross-image RAID reconstruction.
use crate::evidence::image::{detect_filesystem, PartitionKind};
use crate::evidence::partitions::SectorSource;
/// One logical volume's complete contiguous byte range plus the filesystem detected
/// at its start. Resolved directly from a `SectorSource`, so the RAID
/// metadata-less heuristic can descend into LVM on a candidate's assembled
/// device without first registering it as a `DiskImage`.
#[derive(Debug, Clone)]
pub struct LvRegion {
    pub name: String,
    /// Byte offset of the LV data area, relative to the LVM partition start
    /// (i.e. relative to `part_offset` passed to [`lv_regions_from_source`]).
    pub offset: u64,
    pub size_bytes: u64,
    pub kind: PartitionKind,
}

/// Enumerate the logical volumes of an LVM2 PV that begins at `part_offset`
/// within `source` (the PV occupies `[part_offset, part_offset + part_len)`).
/// Returns only complete, contiguous linear LVs and detected filesystems.
///
/// Fragmented or incomplete LVs are omitted because this API cannot express
/// multiple physical ranges. Native readers use the complete `LinearLayout`.
pub fn lv_regions_from_source(
    source: &(dyn SectorSource + Send + Sync),
    part_offset: u64,
    part_len: u64,
) -> Vec<LvRegion> {
    let Ok(metadata) = super::header::read_metadata(source, part_offset, part_len) else {
        return Vec::new();
    };
    let group = &metadata.volume_group;
    let Some(pv) = group
        .physical_volumes
        .iter()
        .find(|pv| pv.identifier.replace('-', "") == metadata.header.identifier)
    else {
        return Vec::new();
    };
    group
        .logical_volumes
        .iter()
        .filter_map(|volume| {
            let layout =
                super::layout::linear_layout(group, volume, pv, part_offset, part_len).ok()?;
            // This API describes a single physical range. A fragmented LV must
            // not be presented as a contiguous filesystem to RAID heuristics.
            let absolute = layout.contiguous_offset()?;
            let signature = layout
                .read(
                    source,
                    0,
                    crate::evidence::partitions::BTRFS_PROBE_END_OFFSET as usize,
                )
                .ok()?;
            Some(LvRegion {
                name: volume.name.clone(),
                offset: absolute.checked_sub(part_offset)?,
                size_bytes: layout.size,
                kind: detect_filesystem(&signature, PartitionKind::Unknown),
            })
        })
        .collect()
}

/// Return a verified contiguous active text range inside a complete PV buffer.
/// Wrapped copies have two source ranges; use `header::read_metadata` for them.
/// This compatibility helper is for probes, not production source reads.
pub fn locate_active_metadata_region(probe: &[u8]) -> Option<(usize, usize)> {
    struct Probe<'a>(&'a [u8]);
    impl SectorSource for Probe<'_> {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
            let start = usize::try_from(offset)?;
            let end = start
                .checked_add(length)
                .ok_or_else(|| anyhow::anyhow!("LVM probe range overflow"))?;
            Ok(self
                .0
                .get(start..end)
                .ok_or_else(|| anyhow::anyhow!("LVM probe truncated"))?
                .to_vec())
        }
    }
    let metadata = super::header::read_metadata(&Probe(probe), 0, probe.len() as u64).ok()?;
    if metadata.text_ranges.len() != 1 {
        return None;
    }
    let range = &metadata.text_ranges[0];
    Some((
        usize::try_from(range.offset).ok()?,
        usize::try_from(range.offset.checked_add(range.size)?).ok()?,
    ))
}
