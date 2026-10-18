//! Verified LVM metadata shared with native filesystem browsing.
use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::lvm2::{header::read_metadata, text::VolumeGroup};
use crate::core::evidence::partitions::{Partition, PartitionKind, SectorSource};

#[derive(Debug, Clone)]
pub struct LvmPvMetadata {
    /// Loaded image holding the PV, or the source id of the assembled array
    /// it sits on.
    pub image_id: String,
    pub partition_id: u8,
    pub partition_byte_offset: u64,
    pub partition_byte_length: u64,
    pub pv_size_bytes: u64,
    pub vg_uuid: String,
    pub vg_name: String,
    pub pv_local_id: String,
    pub pv_uuid: String,
    pub extent_size_sectors: u64,
    pub pe_start_sectors: u64,
    pub volume_group: VolumeGroup,
}

pub fn read_pv_metadata(
    image: &DiskImage,
    partition: &Partition,
) -> anyhow::Result<Option<LvmPvMetadata>> {
    read_pv_metadata_from(image, &image.id, partition, image.sector_size)
}

/// Read the PV on `partition` of any block source; `source_id` names the
/// source in the result.
pub fn read_pv_metadata_from(
    source: &dyn SectorSource,
    source_id: &str,
    partition: &Partition,
    sector_size: usize,
) -> anyhow::Result<Option<LvmPvMetadata>> {
    if partition.kind != PartitionKind::Lvm {
        return Ok(None);
    }
    let partition_byte_offset = partition
        .start_lba
        .checked_mul(sector_size as u64)
        .ok_or_else(|| anyhow::anyhow!("LVM partition offset overflow"))?;
    let partition_byte_length = partition
        .length_lba
        .checked_mul(sector_size as u64)
        .ok_or_else(|| anyhow::anyhow!("LVM partition length overflow"))?;
    let metadata = read_metadata(source, partition_byte_offset, partition_byte_length)?;
    for diagnostic in &metadata.diagnostics {
        tracing::warn!("LVM: {diagnostic}");
    }
    let group = metadata.volume_group;
    let pv = group
        .physical_volumes
        .iter()
        .find(|pv| pv.identifier.replace('-', "") == metadata.header.identifier)
        .ok_or_else(|| anyhow::anyhow!("LVM PV label UUID not found in volume group"))?;
    Ok(Some(LvmPvMetadata {
        image_id: source_id.to_string(),
        partition_id: partition.id,
        partition_byte_offset,
        partition_byte_length,
        pv_size_bytes: metadata.header.size_bytes,
        vg_uuid: group.identifier.clone(),
        vg_name: group.name.clone(),
        pv_local_id: pv.name.clone(),
        pv_uuid: pv.identifier.clone(),
        extent_size_sectors: group.extent_size_sectors,
        pe_start_sectors: pv.pe_start_sectors,
        volume_group: group,
    }))
}
