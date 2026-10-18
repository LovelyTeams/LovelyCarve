//! LVM volume-group joining and RAID member enumeration.
use std::collections::HashMap;

use super::*;
#[cfg(test)]
#[path = "lvm_tests.rs"]
mod lvm_tests;

/// Block sources PV metadata can point at: loaded images by id, plus
/// assembled arrays under [`array_source_id`].
#[derive(Default)]
pub(super) struct PvSources {
    sources: HashMap<String, MemberReader>,
}

impl PvSources {
    pub(super) fn from_images(images: &[Arc<DiskImage>]) -> Self {
        let mut sources = Self::default();
        for image in images {
            sources.insert(image.id.clone(), image.clone());
        }
        sources
    }

    pub(super) fn insert(&mut self, id: String, source: MemberReader) {
        self.sources.insert(id, source);
    }

    fn get(&self, id: &str) -> Option<&MemberReader> {
        self.sources.get(id)
    }
}

/// Source id of an assembled array in PV metadata and LV segments, until
/// [`resolve_array_sources`] replaces it with the array's synthetic image id.
pub(super) fn array_source_id(set_uuid: &str) -> String {
    format!("md:{set_uuid}")
}

/// Once assembled arrays are registered, point the PVs and LV segments that
/// live on them at the arrays' synthetic image ids.
pub fn resolve_array_sources(stack: &mut RaidStack) {
    let images: HashMap<String, String> = stack
        .arrays
        .iter()
        .filter_map(|array| {
            let image_id = array.assembled_image_id.clone()?;
            Some((array_source_id(&array.set_uuid), image_id))
        })
        .collect();
    for vg in &mut stack.volume_groups {
        for member in &mut vg.members {
            if let Some(image_id) = images.get(&member.image_id) {
                member.image_id = image_id.clone();
            }
        }
        for segment in vg
            .logical_volumes
            .iter_mut()
            .flat_map(|lv| lv.segments.iter_mut())
        {
            if let Some(image_id) = images.get(&segment.image_id) {
                segment.image_id = image_id.clone();
            }
        }
    }
}

pub(super) fn group_pvs_into_vgs(
    pvs: &[LvmPvMetadata],
    diagnostics: &mut Vec<String>,
) -> Vec<RaidVolumeGroup> {
    let mut grouped: BTreeMap<String, Vec<&LvmPvMetadata>> = BTreeMap::new();
    for pv in pvs {
        grouped.entry(pv.vg_uuid.clone()).or_default().push(pv);
    }
    grouped
        .into_iter()
        .map(|(vg_uuid, members)| {
            let head = members
                .iter()
                .max_by_key(|pv| pv.volume_group.sequence_number)
                .copied()
                .unwrap_or(members[0]);
            let group = &head.volume_group;
            let conflicting = members.iter().any(|pv| {
                pv.volume_group.sequence_number == group.sequence_number
                    && pv.volume_group != *group
            });
            if conflicting {
                diagnostics.push(format!(
                    "VG {} has conflicting copies at seqno {}",
                    group.name, group.sequence_number
                ));
            }
            let lvs = group
                .logical_volumes
                .iter()
                .filter_map(|lv| {
                    let size_bytes = lv.segments.iter().try_fold(0u64, |total, segment| {
                        segment
                            .extent_count
                            .checked_mul(group.extent_size_sectors)?
                            .checked_mul(512)?
                            .checked_add(total)
                    });
                    let Some(size_bytes) = size_bytes else {
                        diagnostics.push(format!("LVM {} size overflow", lv.name));
                        return None;
                    };
                    let mut segments = Vec::new();
                    for segment in &lv.segments {
                        if conflicting {
                            break;
                        }
                        if segment.kind != "striped"
                            || segment.stripe_count != 1
                            || segment.stripes.len() != 1
                        {
                            diagnostics.push(format!(
                                "LVM {}: unsupported segment {} with {} stripes",
                                lv.name, segment.kind, segment.stripe_count
                            ));
                            segments.clear();
                            break;
                        }
                        let stripe = &segment.stripes[0];
                        let Some(pv_definition) = group
                            .physical_volumes
                            .iter()
                            .find(|pv| pv.name == stripe.physical_volume)
                        else {
                            segments.clear();
                            break;
                        };
                        let candidates: Vec<_> = members
                            .iter()
                            .filter(|pv| pv.pv_uuid == pv_definition.identifier)
                            .collect();
                        if candidates.len() != 1 {
                            diagnostics.push(format!(
                                "LVM {}: missing or ambiguous PV {}",
                                lv.name, pv_definition.identifier
                            ));
                            segments.clear();
                            break;
                        }
                        let pv = candidates[0];
                        if pv.pe_start_sectors != pv_definition.pe_start_sectors {
                            diagnostics.push(format!(
                                "LVM {}: PV {} PE start conflicts with its label",
                                lv.name, pv.pv_uuid
                            ));
                            segments.clear();
                            break;
                        }
                        let mut single = segment.clone();
                        single.start_extent = 0;
                        let volume = crate::core::evidence::partitions::lvm2::text::LogicalVolume {
                            name: lv.name.clone(),
                            identifier: lv.identifier.clone(),
                            segments: vec![single],
                        };
                        let layout = crate::core::evidence::partitions::lvm2::layout::linear_layout(
                            group,
                            &volume,
                            pv_definition,
                            pv.partition_byte_offset,
                            pv.partition_byte_length.min(pv.pv_size_bytes),
                        );
                        let layout = match layout {
                            Ok(layout) => layout,
                            Err(error) => {
                                diagnostics.push(format!("LVM {}: {error:#}", lv.name));
                                segments.clear();
                                break;
                            }
                        };
                        let Some(range) = layout.segments.first() else {
                            segments.clear();
                            break;
                        };
                        if segments.iter().any(|previous: &RaidLvSegment| {
                            previous.image_id == pv.image_id
                                && previous.partition_id == pv.partition_id
                                && previous.byte_offset < range.physical_offset + range.size
                                && range.physical_offset
                                    < previous.byte_offset + previous.byte_length
                        }) {
                            diagnostics
                                .push(format!("LVM {}: overlapping physical extents", lv.name));
                            segments.clear();
                            break;
                        }
                        segments.push(RaidLvSegment {
                            pv_local_id: pv_definition.name.clone(),
                            image_id: pv.image_id.clone(),
                            partition_id: pv.partition_id,
                            byte_offset: range.physical_offset,
                            byte_length: range.size,
                        });
                    }
                    Some(RaidLogicalVolume {
                        name: lv.name.clone(),
                        size_bytes,
                        segments,
                        assembled_image_id: None,
                    })
                })
                .collect();
            RaidVolumeGroup {
                vg_uuid,
                vg_name: head.vg_name.clone(),
                extent_size_sectors: head.extent_size_sectors,
                members: members
                    .iter()
                    .filter_map(|pv| {
                        let definition = group
                            .physical_volumes
                            .iter()
                            .find(|definition| definition.identifier == pv.pv_uuid)?;
                        Some(RaidVgMember {
                            image_id: pv.image_id.clone(),
                            partition_id: pv.partition_id,
                            pv_local_id: definition.name.clone(),
                            pv_uuid: pv.pv_uuid.clone(),
                            pe_start_sectors: definition.pe_start_sectors,
                            partition_byte_offset: pv.partition_byte_offset,
                            partition_byte_length: pv.partition_byte_length,
                        })
                    })
                    .collect(),
                logical_volumes: lvs,
            }
        })
        .collect()
}

/// Every complete LV of `vgs`, as a device reading through `sources`. An LV
/// whose segments are incomplete, reach past their source, or do not add up
/// to its size is skipped rather than shortened.
pub(super) fn materialise_logical_volumes(
    sources: &PvSources,
    vgs: &[RaidVolumeGroup],
) -> Vec<LinearLvSource> {
    let mut out = Vec::new();
    for vg in vgs {
        for lv in &vg.logical_volumes {
            let mut segments = Vec::with_capacity(lv.segments.len());
            let mut total: u64 = 0;
            for seg in &lv.segments {
                let Some(source) = sources.get(&seg.image_id) else {
                    continue;
                };
                let Some(next_total) = total.checked_add(seg.byte_length) else {
                    segments.clear();
                    break;
                };
                if seg
                    .byte_offset
                    .checked_add(seg.byte_length)
                    .is_none_or(|end| end > source.size())
                {
                    segments.clear();
                    break;
                }
                total = next_total;
                segments.push(LvByteSegment {
                    source: source.clone(),
                    byte_offset: seg.byte_offset,
                    byte_length: seg.byte_length,
                });
            }
            if segments.is_empty() || total != lv.size_bytes {
                continue;
            }
            out.push(LinearLvSource {
                vg_uuid: vg.vg_uuid.clone(),
                vg_name: vg.vg_name.clone(),
                lv_name: lv.name.clone(),
                size_bytes: total,
                segments,
            });
        }
    }
    out
}

/// Whether the image exposes block bytes that can hold RAID members. Logical
/// evidence (folders, archives, AD1, Docker) and metadata-only sessions do
/// not.
pub(super) fn is_block_evidence(image: &DiskImage) -> bool {
    !image.is_index_only()
        && image.folder.is_none()
        && image.tar.is_none()
        && image.zip.is_none()
        && image.sevenz.is_none()
        && image.ad1.is_none()
        && image.docker.is_none()
}

/// Partition kinds whose bytes cannot be an md member themselves: containers
/// whose payload is something else (LVs, extended chains, ciphertext).
fn never_member(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Extended
            | PartitionKind::Lvm
            | PartitionKind::Luks
            | PartitionKind::Bitlocker
            | PartitionKind::Filevault
    )
}

/// Every device that could carry an md superblock: complete LVs, partitions
/// and whole images. A superblock can be visible through more than one of
/// them (a partition ending at the disk end shares its tail superblock with
/// the disk); the caller keeps the innermost.
pub(super) fn enumerate_candidates(
    images: &[Arc<DiskImage>],
    lv_devices: &[LinearLvSource],
) -> Vec<CandidateMember> {
    let mut out = Vec::with_capacity(lv_devices.len());
    for lv in lv_devices {
        let reader: MemberReader = Arc::new(lv.clone());
        out.push(CandidateMember {
            reader,
            origin: RaidMemberOrigin::Lv {
                vg_uuid: lv.vg_uuid.clone(),
                vg_name: lv.vg_name.clone(),
                lv_name: lv.lv_name.clone(),
                size_bytes: lv.size_bytes,
            },
        });
    }
    for image in images.iter().filter(|image| is_block_evidence(image)) {
        let sector_size = image.sector_size as u64;
        let ranges = image
            .partitions
            .iter()
            .filter(|partition| !never_member(partition.kind))
            .map(|partition| {
                (
                    partition.id,
                    partition.start_lba.saturating_mul(sector_size),
                    partition.length_lba.saturating_mul(sector_size),
                )
            })
            .chain(std::iter::once((0, 0, image.len())));
        for (partition_id, byte_offset, byte_length) in ranges {
            if byte_length == 0 || byte_offset.saturating_add(byte_length) > image.len() {
                continue;
            }
            out.push(CandidateMember {
                reader: Arc::new(WholeDiskMember {
                    image: image.clone(),
                    byte_offset,
                    byte_length,
                }),
                origin: RaidMemberOrigin::Whole {
                    image_id: image.id.clone(),
                    partition_id,
                    byte_offset,
                    byte_length,
                },
            });
        }
    }
    out
}

pub(super) fn describe_origin(origin: &RaidMemberOrigin) -> String {
    match origin {
        RaidMemberOrigin::Lv {
            vg_name, lv_name, ..
        } => format!("LV {vg_name}/{lv_name}"),
        RaidMemberOrigin::Whole {
            image_id,
            partition_id,
            byte_offset,
            ..
        } => format!("image {image_id} partition {partition_id} (offset {byte_offset})"),
    }
}

#[derive(Clone)]
pub(super) struct AssembledMember {
    pub(super) sb: MdadmSuperblock,
    pub(super) origin: RaidMemberOrigin,
    pub(super) reader: MemberReader,
}

/// One enumerated RAID-member candidate. LVs, partitions and whole images
/// converge on this struct so the probe loop in `detect_with_options` runs
/// once over a uniform input list.
pub(super) struct CandidateMember {
    pub(super) reader: MemberReader,
    pub(super) origin: RaidMemberOrigin,
}
