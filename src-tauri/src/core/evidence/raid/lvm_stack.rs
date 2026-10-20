//! LVM volume groups over assembled arrays and across devices.
//!
//! A PV can sit on an assembled md array (the usual NAS stack: md → LVM →
//! ext4 / Btrfs), and an LV can span PVs on several arrays, images or
//! partitions (Synology SHR combines md arrays of different member sizes
//! this way). Every complete LV not consumed as an md member becomes an
//! assembled device of its own, including single-PV volumes.
use std::collections::HashSet;

use super::*;

pub(super) struct LvmStack {
    pub(super) volume_groups: Vec<RaidVolumeGroup>,
    pub(super) assembled: Vec<AssembledArray>,
}

/// Join the PVs on loaded images (`image_pvs`) with those on assembled arrays
/// and materialise every complete LV. LVs already
/// consumed as md members (`md_member_lvs`, as `(vg uuid, lv name)`) stay
/// members.
pub(super) fn assemble_lvm_stack(
    image_pvs: &[LvmPvMetadata],
    images: &[Arc<DiskImage>],
    arrays: &[AssembledArray],
    md_member_lvs: &HashSet<(String, String)>,
    diagnostics: &mut Vec<String>,
) -> LvmStack {
    let mut sources = PvSources::from_images(images);
    let mut pvs = image_pvs.to_vec();
    for array in arrays {
        if array.target != AssembledTarget::Array {
            continue;
        }
        let source_id = array_source_id(&array.set_uuid);
        sources.insert(source_id.clone(), array.source.clone());
        for partition in array.partitions.iter().flatten() {
            match lvm_meta::read_pv_metadata_from(array.source.as_ref(), &source_id, partition, 512)
            {
                Ok(Some(pv)) => pvs.push(pv),
                Ok(None) => {}
                Err(error) => diagnostics.push(format!(
                    "md/{} partition {} PV read failed: {error:#}",
                    array.set_name, partition.id
                )),
            }
        }
    }

    let volume_groups = group_pvs_into_vgs(&pvs, diagnostics);
    let mut assembled = Vec::new();
    for lv in materialise_logical_volumes(&sources, &volume_groups) {
        if md_member_lvs.contains(&(lv.vg_uuid.clone(), lv.lv_name.clone())) {
            continue;
        }
        let name = format!("{}/{}", lv.vg_name, lv.lv_name);
        let size_bytes = lv.size_bytes;
        let source: Arc<dyn SectorSource + Send + Sync> = Arc::new(lv.clone());
        let mut description = describe_device(source.as_ref(), &format!("LV {name}"), diagnostics);
        label_whole_volume(
            &mut description.partitions,
            size_bytes,
            &format!("lvm/{name}"),
        );
        diagnostics.push(format!("LV {name} assembled as its own device"));
        assembled.push(AssembledArray {
            set_uuid: format!("lv:{}:{}", lv.vg_uuid, lv.lv_name),
            set_name: name.clone(),
            // The VG UUID (no `:`) comes first so the label stays injective
            // and the name after it can be shown as is.
            label: format!("lvm:{}:{name}", lv.vg_uuid),
            target: AssembledTarget::LogicalVolume {
                vg_uuid: lv.vg_uuid.clone(),
                lv_name: lv.lv_name.clone(),
            },
            source,
            size_bytes,
            fs_kind: description.fs_kind,
            partitions: Some(description.partitions),
        });
    }
    LvmStack {
        volume_groups,
        assembled,
    }
}

/// Name the single whole-device volume of an assembled device after where it
/// came from, e.g. `ext4 (md · home)`.
pub(super) fn label_whole_volume(partitions: &mut [Partition], size_bytes: u64, origin: &str) {
    if let [partition] = partitions {
        if partition.start_lba == 0
            && partition.length_lba == size_bytes / 512
            && partition.label.is_none()
        {
            // This label is also a navigation component, so origin separators
            // must not become directory separators in path-based clients.
            let origin = origin.replace('/', " · ");
            partition.label = Some(format!("{} ({origin})", kind_label(partition.kind)));
        }
    }
}
