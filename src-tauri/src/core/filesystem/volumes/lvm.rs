//! Read-only LVM navigation over verified metadata and complete linear LV maps.
//! Fragmented single-PV LVs retain their logical size and source ranges.

use crate::core::evidence::image::{
    detect_filesystem, find_partition, DiskImage, Partition, PartitionKind,
};
use crate::core::evidence::partitions::lvm2::{
    header::read_metadata,
    layout::{linear_layout, LinearLayout},
    text::VolumeGroup,
};
use crate::core::filesystem::tree::fs_tree::{FsDataRun, FsNode, Mactb, NodeKind};
use crate::core::filesystem::unalloc;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct LvmVolume {
    partition_id: u8,
    /// Absolute byte offset of the PV data area start (`pe_start` from the LVM2
    /// metadata, in image bytes). Below this lies the LVM label + metadata,
    /// not allocatable PE space.
    pe_start_bytes: u64,
    /// Absolute byte offset after the last declared PE, bounding the data
    /// area. `[pe_start_bytes, pv_end_bytes)` is the allocatable region.
    pv_end_bytes: u64,
    allocation_known: bool,
    occupied: Vec<(u64, u64)>,
    pub metadata: VolumeGroup,
    pub diagnostics: Vec<String>,
    pub logical_volumes: Vec<LvmLogicalVolume>,
}

#[derive(Debug, Clone)]
pub struct LvmLogicalVolume {
    pub index: usize,
    pub name: String,
    /// First physical segment offset. The complete map is `layout`, not a
    /// contiguous `[absolute_offset, absolute_offset + size_bytes)` span.
    pub absolute_offset: u64,
    pub size_bytes: u64,
    pub kind: PartitionKind,
    pub identifier: String,
    pub layout: Option<LinearLayout>,
    pub read_error: Option<String>,
}

impl LvmVolume {
    pub fn open(image: &DiskImage, partition: &Partition) -> anyhow::Result<Self> {
        if partition.kind != PartitionKind::Lvm {
            anyhow::bail!("partition {} is not an LVM2 physical volume", partition.id);
        }

        let partition_offset = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| anyhow::anyhow!("LVM partition offset overflow"))?;
        let partition_size = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| anyhow::anyhow!("LVM partition size overflow"))?;
        let metadata = read_metadata(image, partition_offset, partition_size)?;
        let group = &metadata.volume_group;
        let pv = group
            .physical_volumes
            .iter()
            .find(|pv| pv.identifier.replace('-', "") == metadata.header.identifier)
            .ok_or_else(|| anyhow::anyhow!("LVM label PV identifier absent from metadata"))?;
        let pe_start_bytes = pv
            .pe_start_sectors
            .checked_mul(512)
            .and_then(|offset| partition_offset.checked_add(offset))
            .ok_or_else(|| anyhow::anyhow!("LVM PE start overflow"))?;
        let pv_end_bytes = pv
            .extent_count
            .checked_mul(group.extent_size_sectors)
            .and_then(|size| size.checked_mul(512))
            .and_then(|size| pe_start_bytes.checked_add(size))
            .ok_or_else(|| anyhow::anyhow!("LVM PE end overflow"))?;
        let partition_end = partition_offset
            .checked_add(partition_size)
            .ok_or_else(|| anyhow::anyhow!("LVM partition end overflow"))?;
        if pv_end_bytes > partition_end {
            anyhow::bail!("LVM PE space exceeds partition");
        }
        let mut logical_volumes = Vec::new();
        let mut occupied = Vec::new();
        let mut diagnostics = metadata.diagnostics;
        let mut allocation_known = true;
        for volume in &group.logical_volumes {
            let size_bytes = volume.segments.iter().try_fold(0u64, |total, segment| {
                segment
                    .extent_count
                    .checked_mul(group.extent_size_sectors)
                    .and_then(|size| size.checked_mul(512))
                    .and_then(|size| total.checked_add(size))
                    .ok_or_else(|| anyhow::anyhow!("LVM LV size overflow"))
            })?;
            let (layout, read_error, absolute_offset, kind) =
                match linear_layout(group, volume, pv, partition_offset, partition_size) {
                    Ok(layout) => {
                        for segment in &layout.segments {
                            occupied.push((segment.physical_offset, segment.size));
                        }
                        let signature = layout.read(
                            image,
                            0,
                            crate::core::evidence::partitions::BTRFS_PROBE_END_OFFSET as usize,
                        )?;
                        let kind = detect_filesystem(&signature, PartitionKind::Unknown);
                        let offset = layout
                            .segments
                            .first()
                            .map_or(0, |segment| segment.physical_offset);
                        (Some(layout), None, offset, kind)
                    }
                    Err(error) => {
                        let message = error.to_string();
                        diagnostics.push(format!("LV {}: {message}", volume.name));
                        // Unknown segment types can reserve local extents. Avoid
                        // falsely labelling allocated data as unallocated space.
                        allocation_known = false;
                        (None, Some(message), 0, PartitionKind::Unknown)
                    }
                };
            logical_volumes.push(LvmLogicalVolume {
                index: logical_volumes.len(),
                name: volume.name.clone(),
                identifier: volume.identifier.clone(),
                absolute_offset,
                size_bytes,
                kind,
                layout,
                read_error,
            });
        }
        for diagnostic in &diagnostics {
            tracing::warn!("LVM: {diagnostic}");
        }
        Ok(Self {
            partition_id: partition.id,
            pe_start_bytes,
            pv_end_bytes,
            allocation_known,
            occupied,
            logical_volumes,
            metadata: metadata.volume_group,
            diagnostics,
        })
    }

    pub fn logical_volume_nodes(&self) -> anyhow::Result<Vec<FsNode>> {
        Ok(self
            .logical_volumes
            .iter()
            .map(|lv| FsNode {
                id: format!("lvm:{}:{}", self.partition_id, lv.index),
                // Keep the LV label concise and usable as a path component.
                name: crate::core::common::navigation_name::volume_name(&lv.name),
                parent_id: Some(format!("part:{}", self.partition_id)),
                kind: if is_navigable_lv_kind(lv.kind) {
                    NodeKind::Dir
                } else {
                    NodeKind::File
                },
                size: lv.size_bytes,
                deleted: false,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            })
            .collect())
    }

    /// Free physical extents, excluding every segment and non-PE tail bytes.
    /// If any allocation cannot be resolved safely, no free-space claim is made.
    pub fn vg_unalloc_nodes(&self) -> Vec<FsNode> {
        if !self.allocation_known || self.pv_end_bytes <= self.pe_start_bytes {
            return Vec::new();
        }
        let occupied = &self.occupied;
        let total = self.pv_end_bytes - self.pe_start_bytes;
        unalloc::free_intervals(
            self.pe_start_bytes,
            total,
            occupied,
            self.metadata.extent_size_sectors * 512,
        )
        .into_iter()
        .enumerate()
        .map(|(idx, (offset, len))| FsNode {
            id: format!("unalloc:lvm:{}:0x{offset:x}:0x{len:x}", self.partition_id),
            name: format!("未使用空间{}", idx + 1),
            parent_id: Some(format!("part:{}", self.partition_id)),
            kind: NodeKind::File,
            size: len,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        })
        .collect()
    }
}

/// Filesystems we can navigate into when they appear as the contents of an
/// LVM2 logical volume. Mirrors the per-FS `Lvm` source variants in
/// `core::filesystem::volumes::{ext4, xfs, btrfs, ntfs}`. Add a new entry here whenever
/// another FS adapter grows an `Lvm` source.
fn is_navigable_lv_kind(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Ext4 | PartitionKind::Xfs | PartitionKind::Btrfs | PartitionKind::Ntfs
    )
}

pub fn parse_lvm_id(id: &str) -> Option<(&str, usize)> {
    let mut parts = id.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("lvm"), Some(part_id), Some(lv_index), None) => {
            lv_index.parse().ok().map(|idx| (part_id, idx))
        }
        _ => None,
    }
}

impl LvmLogicalVolume {
    pub fn readable_layout(&self) -> anyhow::Result<&LinearLayout> {
        self.layout.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "LVM {}: {}",
                self.name,
                self.read_error
                    .as_deref()
                    .unwrap_or("volume is unavailable")
            )
        })
    }

    /// A contiguous volume uses its original image offset; fragmented maps
    /// use LV-relative offsets inside the filesystem reader.
    pub fn reader_geometry(&self) -> anyhow::Result<(u64, Option<LinearLayout>)> {
        let layout = self.readable_layout()?;
        match layout.contiguous_offset() {
            Some(offset) => Ok((offset, None)),
            None => Ok((0, Some(layout.clone()))),
        }
    }
}

pub(crate) fn read_backing_range(
    image: &DiskImage,
    layout: Option<&LinearLayout>,
    offset: u64,
    length: usize,
) -> anyhow::Result<Vec<u8>> {
    match layout {
        Some(layout) => layout.read(image, offset, length),
        None => image.read_range(offset, length),
    }
}

pub(crate) fn read_backing_into(
    image: &DiskImage,
    layout: Option<&LinearLayout>,
    offset: u64,
    buffer: &mut [u8],
) -> anyhow::Result<usize> {
    match layout {
        Some(layout) => {
            let bytes = layout.read(image, offset, buffer.len())?;
            buffer[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
        None => image.read_into(offset, buffer),
    }
}

/// Project filesystem runs to original image offsets for hex/provenance views.
pub(crate) fn physical_runs(
    layout: Option<&LinearLayout>,
    runs: Vec<FsDataRun>,
) -> anyhow::Result<Vec<FsDataRun>> {
    let Some(layout) = layout else {
        return Ok(runs);
    };
    let mut output = Vec::new();
    for run in runs {
        run.logical_offset
            .checked_add(run.length)
            .ok_or_else(|| anyhow::anyhow!("LVM file logical run end overflow"))?;
        for range in layout.ranges(run.physical_offset, run.length)? {
            let logical_offset = run
                .logical_offset
                .checked_add(range.logical_offset - run.physical_offset)
                .ok_or_else(|| anyhow::anyhow!("LVM file logical run offset overflow"))?;
            output.push(FsDataRun {
                logical_offset,
                physical_offset: range.physical_offset,
                length: range.size,
            });
        }
    }
    Ok(output)
}

pub(crate) fn logical_volume_for_id(
    image: &DiskImage,
    id: &str,
) -> anyhow::Result<LvmLogicalVolume> {
    let (partition_id, index) =
        parse_lvm_id(id).ok_or_else(|| anyhow::anyhow!("invalid LVM node {id}"))?;
    let partition = find_partition(image, partition_id)?;
    LvmVolume::open(image, partition)?
        .logical_volumes
        .into_iter()
        .nth(index)
        .ok_or_else(|| anyhow::anyhow!("logical volume missing: {id}"))
}

pub(crate) fn logical_volume_stat(
    image: &DiskImage,
    id: &str,
) -> anyhow::Result<crate::core::filesystem::tree::fs_tree::FsStat> {
    let volume = logical_volume_for_id(image, id)?;
    let layout = volume.readable_layout()?;
    let sample = layout.read(image, 0, 4096)?;
    Ok(crate::core::filesystem::tree::fs_tree::FsStat {
        node_id: id.to_owned(),
        size: layout.size,
        inode: None,
        mode: None,
        mime: crate::core::filesystem::tree::fs_tree::detect_mime_bytes(&sample).to_owned(),
        magic_hex: (!sample.is_empty()).then(|| hex::encode(&sample[..sample.len().min(32)])),
        entropy: (!sample.is_empty())
            .then(|| crate::core::filesystem::tree::fs_tree::shannon_entropy(&sample)),
        data_runs: layout
            .segments
            .iter()
            .map(|segment| FsDataRun {
                logical_offset: segment.logical_offset,
                physical_offset: segment.physical_offset,
                length: segment.size,
            })
            .collect(),
    })
}
