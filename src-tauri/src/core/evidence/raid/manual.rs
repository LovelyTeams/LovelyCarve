//! RAID assembly from explicit geometry and ordered members.
use std::collections::BTreeSet;

use super::source::{build_volume, ArrayLevel, ArraySpec, Raid0ZoneLayout};
use super::*;

/// Assemble a RAID device from an explicit geometry and member order, detecting
/// the partition table / filesystem on the result. Used by the manual builder:
/// the parameters come from the examiner, so nothing is searched — the device
/// is materialised exactly as specified and probed so the UI can preview what
/// the geometry yields (and the caller can register it as a synthetic image).
pub fn manual_assemble(
    images: &[Arc<DiskImage>],
    params: &ManualRaidParams,
) -> anyhow::Result<AssembledArray> {
    if params.member_image_ids.is_empty() {
        anyhow::bail!("no members selected");
    }
    let level = ArrayLevel::from_md(params.level, params.layout).with_context(|| {
        format!(
            "unsupported RAID level/layout {}/{}",
            params.level, params.layout
        )
    })?;
    let mut missing: BTreeSet<usize> = params.missing_slots.iter().copied().collect();
    missing.extend(params.missing_slot);
    let mut columns = Vec::with_capacity(params.member_image_ids.len());
    for (slot, id) in params.member_image_ids.iter().enumerate() {
        if id.is_empty() {
            missing.insert(slot);
            columns.push(None);
            continue;
        }
        let image = images
            .iter()
            .find(|im| &im.id == id)
            .with_context(|| format!("member image {id} is not loaded"))?;
        columns.push(Some(image.clone()));
    }
    if let Some(&slot) = missing.iter().find(|&&slot| slot >= columns.len()) {
        anyhow::bail!(
            "missing column {slot} is outside the {} selected columns",
            columns.len()
        );
    }
    let min_size = columns
        .iter()
        .flatten()
        .map(|image| image.len())
        .min()
        .context("every column is marked missing")?;
    let data_offset = params.data_offset_sectors.saturating_mul(512);
    if data_offset >= min_size {
        anyhow::bail!("data offset {data_offset} exceeds member size {min_size}");
    }
    let per_member = min_size - data_offset;
    let members = columns
        .iter()
        .enumerate()
        .map(|(slot, image)| {
            image
                .as_ref()
                .filter(|_| !missing.contains(&slot))
                .map(|image| StripedMember {
                    reader: Arc::new(WholeDiskMember {
                        image: image.clone(),
                        byte_offset: 0,
                        byte_length: image.len(),
                    }),
                    data_offset_bytes: data_offset,
                    data_size_bytes: per_member,
                })
        })
        .collect();
    let spec = ArraySpec {
        level,
        chunk_bytes: u64::from(params.chunk_sectors) * 512,
        device_bytes: None,
        zone_layout: Raid0ZoneLayout::Original,
    };
    let built = build_volume(&spec, members)?;

    let detected = detect_partitions_with_meta(built.source.as_ref(), 512)?;
    let mut partitions = detected.partitions;
    // Best effort: an unreadable boot region keeps the table-supplied kinds.
    if let Err(err) = refine_partition_kinds(built.source.as_ref(), &mut partitions, 512) {
        tracing::debug!("assembled RAID partition kind refinement failed: {err:#}");
    }
    let fs_kind = partitions
        .iter()
        .filter(|p| is_browsable_fs(p.kind))
        .max_by_key(|p| p.length_lba)
        .map(|p| p.kind)
        .unwrap_or(PartitionKind::Unknown);

    let set_uuid = manual_set_uuid(params, &missing);
    let set_name = format!("manual-raid{}", params.level);
    Ok(AssembledArray {
        label: array_label(&set_name, &set_uuid),
        set_uuid,
        set_name,
        target: AssembledTarget::Array,
        source: built.source,
        size_bytes: built.size_bytes,
        fs_kind,
        partitions: if partitions.is_empty() {
            None
        } else {
            Some(partitions)
        },
    })
}

/// Whether a kind names a browsable filesystem (used to pick the assembled
/// device's primary `fs_kind` for the synthetic image label).
pub(super) fn is_browsable_fs(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Ext4
            | PartitionKind::Btrfs
            | PartitionKind::Xfs
            | PartitionKind::Ntfs
            | PartitionKind::Refs
            | PartitionKind::Fat12
            | PartitionKind::Fat16
            | PartitionKind::Fat32
            | PartitionKind::ExFat
    )
}

/// Deterministic `set_uuid` for a manual reconstruction so re-running the same
/// geometry reuses the same synthetic image instead of stacking duplicates.
pub(super) fn manual_set_uuid(params: &ManualRaidParams, missing: &BTreeSet<usize>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for id in &params.member_image_ids {
        id.hash(&mut hasher);
    }
    params.level.hash(&mut hasher);
    params.layout.hash(&mut hasher);
    params.chunk_sectors.hash(&mut hasher);
    params.data_offset_sectors.hash(&mut hasher);
    missing.hash(&mut hasher);
    let h = hasher.finish();
    format!("manual{h:016x}{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::raid::source::test_support::pattern;

    struct Mem(Vec<u8>);
    impl SectorSource for Mem {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = (start + len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }

    fn image(bytes: Vec<u8>) -> Arc<DiskImage> {
        Arc::new(
            DiskImage::from_assembled("manual-test".into(), Arc::new(Mem(bytes)), Vec::new(), 512)
                .expect("image"),
        )
    }

    fn params(ids: Vec<String>, level: i32, layout: u32) -> ManualRaidParams {
        ManualRaidParams {
            member_image_ids: ids,
            level,
            layout,
            chunk_sectors: 8,
            data_offset_sectors: 0,
            missing_slot: None,
            missing_slots: Vec::new(),
        }
    }

    /// A mirror pair: either disk alone, or an absent column, still yields
    /// the whole volume.
    #[test]
    fn raid1_reads_from_any_present_mirror() {
        let disk = pattern(64 * 1024, 4);
        let images = vec![image(disk.clone()), image(disk.clone())];
        let ids: Vec<String> = images.iter().map(|image| image.id.clone()).collect();
        let full = manual_assemble(&images, &params(ids.clone(), 1, 0)).expect("raid1");
        assert_eq!(full.size_bytes, disk.len() as u64);
        let degraded = manual_assemble(&images, &params(vec![ids[0].clone(), String::new()], 1, 0))
            .expect("raid1 with an absent mirror");
        assert_eq!(degraded.source.read(0, disk.len()).expect("read"), disk);
        assert_ne!(full.set_uuid, degraded.set_uuid);
    }

    #[test]
    fn rejects_more_missing_columns_than_the_level_tolerates() {
        let images: Vec<_> = (0..4).map(|seed| image(pattern(64 * 1024, seed))).collect();
        let ids: Vec<String> = images.iter().map(|image| image.id.clone()).collect();
        let mut request = params(ids, 5, 2);
        request.missing_slots = vec![0, 1];
        assert!(manual_assemble(&images, &request).is_err());
        request.level = 6;
        assert!(manual_assemble(&images, &request).is_ok());
        request.missing_slots = vec![9];
        assert!(manual_assemble(&images, &request).is_err());
    }
}
