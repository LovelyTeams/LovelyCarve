//! Metadata-driven RAID detection and assembly.
use std::collections::{HashMap, HashSet};

use super::firmware::assemble_firmware_sets;
use super::lvm_stack::{assemble_lvm_stack, label_whole_volume};
use super::mdadm_super::ProbeHit;
use super::source::BuiltVolume;
use super::*;

/// [`detect_with_options`] with the default options: arrays whose metadata
/// still expects absent members wait for them.
pub fn detect_with_sources(images: &[Arc<DiskImage>]) -> (RaidStack, Vec<AssembledArray>) {
    detect_with_options(images, &DetectOptions::default())
}

/// Detect every LVM / md stack across `images` and return the descriptive
/// stack plus the assembled devices, which the caller registers as synthetic
/// `DiskImage`s. Reported arrays and LVs have `assembled_image_id == None`
/// here; the caller fills it in after registration.
pub fn detect_with_options(
    images: &[Arc<DiskImage>],
    options: &DetectOptions,
) -> (RaidStack, Vec<AssembledArray>) {
    let mut diagnostics = Vec::new();

    // Pass 1 — LVM PVs on the loaded images, joined into volume groups. Their
    // complete LVs may be md members. Grouping is redone in pass 4 once PVs on
    // assembled arrays are known; only that grouping's diagnostics are kept.
    let mut image_summaries = Vec::with_capacity(images.len());
    let mut image_pvs: Vec<LvmPvMetadata> = Vec::new();
    for image in images {
        let mut pv_count = 0usize;
        if is_block_evidence(image) {
            for partition in &image.partitions {
                match lvm_meta::read_pv_metadata(image, partition) {
                    Ok(Some(meta)) => {
                        pv_count += 1;
                        image_pvs.push(meta);
                    }
                    Ok(None) => {}
                    Err(err) => diagnostics.push(format!(
                        "image {} partition {} PV read failed: {err}",
                        image.id, partition.id
                    )),
                }
            }
        }
        image_summaries.push(RaidImage {
            image_id: image.id.clone(),
            path: image.path.display().to_string(),
            size_bytes: image.len(),
            partition_count: image.partitions.len(),
            lvm_pv_count: pv_count,
        });
    }
    let image_vgs = group_pvs_into_vgs(&image_pvs, &mut Vec::new());
    let lv_devices = materialise_logical_volumes(&PvSources::from_images(images), &image_vgs);

    // Pass 2 — md superblocks on LVs, partitions and whole images.
    let mut found = Vec::new();
    for candidate in enumerate_candidates(images, &lv_devices) {
        match mdadm_super::probe(candidate.reader.as_ref()) {
            Ok(Some(hit)) => found.push((hit, candidate)),
            Ok(None) => {}
            Err(err) => diagnostics.push(format!(
                "md superblock probe failed on {}: {err}",
                describe_origin(&candidate.origin)
            )),
        }
    }
    let mut members_by_set: BTreeMap<String, Vec<AssembledMember>> = BTreeMap::new();
    for (hit, candidate) in innermost_superblocks(found) {
        if !hit.superblock.checksum_valid {
            diagnostics.push(format!(
                "md {} superblock on {} fails its checksum; ignored",
                hit.superblock.version,
                describe_origin(&candidate.origin)
            ));
            continue;
        }
        let set_uuid = mdadm_super::format_uuid(&hit.superblock.set_uuid);
        members_by_set
            .entry(set_uuid)
            .or_default()
            .push(AssembledMember {
                sb: hit.superblock,
                origin: candidate.origin,
                reader: candidate.reader,
            });
    }

    // Pass 3 — assemble each set and describe what it holds.
    let mut arrays = Vec::new();
    let mut filesystems = Vec::new();
    let mut assembled = Vec::new();
    for (set_uuid, members) in members_by_set {
        let allow_degraded = options.degraded_sets.contains(&set_uuid);
        let Some(outcome) = md_array::assemble_md_set(&set_uuid, members, allow_degraded) else {
            continue;
        };
        diagnostics.extend(outcome.diagnostics);
        let array = outcome.array;
        if let Some(volume) = outcome.volume {
            let tag = if array.set_name.is_empty() {
                format!("md/{set_uuid}")
            } else {
                format!("md/{}", array.set_name)
            };
            let identity = VolumeIdentity {
                set_uuid: &set_uuid,
                set_name: &array.set_name,
                tag: &tag,
                label: array_label(&array.set_name, &set_uuid),
            };
            assembled.push(register_volume(
                identity,
                volume,
                &outcome.absent_slots,
                &mut filesystems,
                &mut diagnostics,
            ));
        }
        arrays.push(array);
    }

    // Pass 3b — firmware RAID (Intel IMSM, SNIA DDF) on whole images that are
    // not whole-disk md members.
    let image_sizes: HashMap<&str, u64> = images
        .iter()
        .map(|image| (image.id.as_str(), image.len()))
        .collect();
    let md_whole_disks: HashSet<String> = arrays
        .iter()
        .flat_map(|array| &array.members)
        .filter_map(|member| match &member.origin {
            RaidMemberOrigin::Whole {
                image_id,
                byte_offset: 0,
                byte_length,
                ..
            } if image_sizes.get(image_id.as_str()) == Some(byte_length) => Some(image_id.clone()),
            RaidMemberOrigin::Whole { .. } | RaidMemberOrigin::Lv { .. } => None,
        })
        .collect();
    let firmware = assemble_firmware_sets(images, &md_whole_disks, options);
    diagnostics.extend(firmware.diagnostics);
    for outcome in firmware.volumes {
        if let Some(volume) = outcome.volume {
            let identity = VolumeIdentity {
                set_uuid: &outcome.array.set_uuid,
                set_name: &outcome.array.set_name,
                tag: &outcome.tag,
                label: outcome.label,
            };
            assembled.push(register_volume(
                identity,
                volume,
                &outcome.absent_slots,
                &mut filesystems,
                &mut diagnostics,
            ));
        }
        arrays.push(outcome.array);
    }
    // A firmware member disk shows its volume's partitions (a RAID1 mirror,
    // or the first strip of a striped volume); PVs read through them repeat
    // the PVs on the assembled volume.
    let image_pvs: Vec<LvmPvMetadata> = image_pvs
        .into_iter()
        .filter(|pv| !firmware.claimed_images.contains(&pv.image_id))
        .collect();

    // Pass 4 — PVs on assembled arrays and LVs spanning several PVs.
    let md_member_lvs: HashSet<(String, String)> = arrays
        .iter()
        .flat_map(|array| &array.members)
        .filter_map(|member| match &member.origin {
            RaidMemberOrigin::Lv {
                vg_uuid, lv_name, ..
            } => Some((vg_uuid.clone(), lv_name.clone())),
            RaidMemberOrigin::Whole { .. } => None,
        })
        .collect();
    let lvm = assemble_lvm_stack(
        &image_pvs,
        images,
        &assembled,
        &md_member_lvs,
        &mut diagnostics,
    );
    let volume_groups = lvm.volume_groups;
    assembled.extend(lvm.assembled);

    // Pass 5 — metadata-less reconstruction. Images already claimed by an md
    // set, a firmware container or a volume group are excluded so a disk is
    // never assigned twice.
    let mut consumed: HashSet<String> = firmware.claimed_images;
    for array in &arrays {
        for member in &array.members {
            if let RaidMemberOrigin::Whole { image_id, .. } = &member.origin {
                consumed.insert(image_id.clone());
            }
        }
    }
    for vg in &volume_groups {
        for member in &vg.members {
            consumed.insert(member.image_id.clone());
        }
    }
    let heur = heuristic::detect(images, &consumed);
    arrays.extend(heur.arrays);
    filesystems.extend(heur.filesystems);
    assembled.extend(heur.assembled);
    diagnostics.extend(heur.diagnostics);

    let stack = RaidStack {
        images: image_summaries,
        volume_groups,
        arrays,
        filesystems,
        diagnostics,
    };
    (stack, assembled)
}

/// Names of an assembled array: its set identity, the origin shown in volume
/// labels (`md/home`, `imsm/Volume0`) and its `assembled://` label.
struct VolumeIdentity<'a> {
    set_uuid: &'a str,
    set_name: &'a str,
    tag: &'a str,
    label: String,
}

/// Describe what an assembled array holds and build the record that
/// registers it as a synthetic image.
fn register_volume(
    identity: VolumeIdentity<'_>,
    volume: BuiltVolume,
    absent_slots: &[usize],
    filesystems: &mut Vec<RaidFilesystem>,
    diagnostics: &mut Vec<String>,
) -> AssembledArray {
    let mut description = describe_device(volume.source.as_ref(), identity.tag, diagnostics);
    label_whole_volume(&mut description.partitions, volume.size_bytes, identity.tag);
    if let Some(filesystem) = &description.filesystem {
        filesystems.push(filesystem.record(identity.set_uuid));
    }
    // A degraded assembly is a different reconstruction from the complete
    // one: it gets its own image, so loading the missing member later yields
    // a fresh, fully read image.
    let mut label = identity.label;
    if !absent_slots.is_empty() {
        let slots: Vec<String> = absent_slots.iter().map(|slot| slot.to_string()).collect();
        label.push_str(&format!("~without-{}", slots.join(",")));
    }
    AssembledArray {
        label,
        set_uuid: identity.set_uuid.to_string(),
        set_name: identity.set_name.to_string(),
        target: AssembledTarget::Array,
        source: volume.source,
        size_bytes: volume.size_bytes,
        fs_kind: description.fs_kind,
        partitions: Some(description.partitions),
    }
}

/// A superblock at the end of a partition that ends where its disk ends is
/// visible through both; it belongs to the innermost device holding it.
fn innermost_superblocks(
    found: Vec<(ProbeHit, CandidateMember)>,
) -> Vec<(ProbeHit, CandidateMember)> {
    let mut innermost: HashMap<(String, u64), (usize, u64)> = HashMap::new();
    for (index, (hit, candidate)) in found.iter().enumerate() {
        let RaidMemberOrigin::Whole {
            image_id,
            byte_offset,
            byte_length,
            ..
        } = &candidate.origin
        else {
            continue;
        };
        let key = (image_id.clone(), byte_offset + hit.super_offset_bytes);
        match innermost.get(&key) {
            Some(&(_, length)) if length <= *byte_length => {}
            _ => {
                innermost.insert(key, (index, *byte_length));
            }
        }
    }
    found
        .into_iter()
        .enumerate()
        .filter(|(index, (hit, candidate))| match &candidate.origin {
            RaidMemberOrigin::Whole {
                image_id,
                byte_offset,
                ..
            } => innermost
                .get(&(image_id.clone(), byte_offset + hit.super_offset_bytes))
                .is_some_and(|&(kept, _)| kept == *index),
            RaidMemberOrigin::Lv { .. } => true,
        })
        .map(|(_, pair)| pair)
        .collect()
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
