//! Assembly policy for one Linux md set: which members are current, which slot
//! each holds, and whether the set can be read.
//!
//! The rules follow md's own assembly. The superblock with the highest event
//! count is authoritative for geometry and roles. A member whose count is more
//! than one behind it missed updates and is stale; one behind is accepted with
//! its role taken from the freshest table, as md does after a crash between
//! superblock writes. Members still being rebuilt or replacing another device
//! are not read. A missing slot is rebuilt from redundancy only when the level
//! allows it, and automatically only when md itself recorded the slot as empty;
//! when the metadata still lists the absent member, the examiner decides
//! whether to assemble without it (`allow_degraded`).
use std::collections::BTreeSet;

use super::*;
use crate::core::evidence::raid::mdadm_super::{
    FEATURE_RAID0_LAYOUT, FEATURE_RECOVERY_OFFSET, FEATURE_REPLACEMENT, FEATURE_RESHAPE_ACTIVE,
    ROLE_FAULTY, ROLE_JOURNAL, ROLE_MAX, ROLE_SPARE,
};
use crate::core::evidence::raid::source::{
    build_volume, ArrayLevel, ArraySpec, BuiltVolume, Raid0ZoneLayout,
};

/// md arrays never exceed this many members; larger counts are corruption.
const MAX_RAID_DISKS: u32 = 1024;

/// Result of assembling one md set.
pub(super) struct MdOutcome {
    pub(super) array: RaidArray,
    pub(super) volume: Option<BuiltVolume>,
    /// Slots the assembled volume does not read (rebuilt or served by other
    /// mirrors); empty for a complete assembly.
    pub(super) absent_slots: Vec<usize>,
    pub(super) diagnostics: Vec<String>,
}

struct Classified {
    state: RaidMemberState,
    /// Slot named by the authoritative role table, even for members that
    /// will not be read.
    slot: Option<u32>,
}

pub(super) fn assemble_md_set(
    set_uuid: &str,
    members: Vec<AssembledMember>,
    allow_degraded: bool,
) -> Option<MdOutcome> {
    let mut diagnostics = Vec::new();
    // The first member with the highest event count is authoritative.
    let freshest = members
        .iter()
        .enumerate()
        .fold(None::<(usize, u64)>, |best, (index, member)| match best {
            Some((_, events)) if events >= member.sb.events => best,
            _ => Some((index, member.sb.events)),
        })
        .map(|(index, _)| index)?;
    let fresh = members[freshest].sb.clone();
    let name = if fresh.set_name.is_empty() {
        format!("md/{set_uuid}")
    } else {
        format!("md/{}", fresh.set_name)
    };
    let raid_disks = fresh.raid_disks;
    if raid_disks == 0 || raid_disks > MAX_RAID_DISKS {
        diagnostics.push(format!("{name}: implausible member count {raid_disks}"));
    }

    let classified: Vec<Classified> = members
        .iter()
        .map(|member| classify(member, &fresh, &name, &mut diagnostics))
        .collect();

    // Fill slots with active members; a second active member in a slot is
    // either the same device loaded twice or a conflict.
    let slot_count = raid_disks.min(MAX_RAID_DISKS) as usize;
    let mut slots: Vec<Option<usize>> = vec![None; slot_count];
    let mut duplicates = Vec::new();
    let mut conflict = false;
    for (index, class) in classified.iter().enumerate() {
        if class.state != RaidMemberState::Active {
            continue;
        }
        let Some(slot) = class
            .slot
            .map(|slot| slot as usize)
            .filter(|&slot| slot < slot_count)
        else {
            continue;
        };
        match slots[slot] {
            None => slots[slot] = Some(index),
            Some(existing) => {
                let first = &members[existing].sb;
                let second = &members[index].sb;
                if first.dev_number == second.dev_number
                    && first.events == second.events
                    && first.device_uuid == second.device_uuid
                {
                    diagnostics.push(format!(
                        "{name}: {} duplicates the slot {slot} member {}, which is the one read",
                        describe_origin(&members[index].origin),
                        describe_origin(&members[existing].origin)
                    ));
                    duplicates.push(index);
                } else {
                    diagnostics.push(format!(
                        "{name}: {} and {} both claim slot {slot}",
                        describe_origin(&members[existing].origin),
                        describe_origin(&members[index].origin)
                    ));
                    conflict = true;
                }
            }
        }
    }
    let absent: Vec<usize> = (0..slot_count)
        .filter(|&slot| slots[slot].is_none())
        .collect();
    // A slot md still assigns to a device is expected back, unless that
    // device is here and still being rebuilt (md starts such arrays on its
    // own); every other gap needs the examiner's consent.
    let recorded: BTreeSet<usize> = fresh
        .dev_roles
        .iter()
        .filter(|&&role| role < ROLE_MAX && u32::from(role) < raid_disks)
        .map(|&role| role as usize)
        .collect();
    let rebuilding: BTreeSet<usize> = classified
        .iter()
        .filter(|class| class.state == RaidMemberState::Rebuilding)
        .filter_map(|class| class.slot.map(|slot| slot as usize))
        .collect();
    let expected_back = |slot: &usize| recorded.contains(slot) && !rebuilding.contains(slot);

    let level = ArrayLevel::from_md(fresh.level, fresh.layout);
    let mut degradable = false;
    let mut volume = None;
    let readable = match level {
        None => {
            diagnostics.push(format!(
                "{name}: md level {} layout {} is not supported",
                fresh.level, fresh.layout
            ));
            false
        }
        Some(_) if fresh.feature_map & FEATURE_RESHAPE_ACTIVE != 0 => {
            diagnostics.push(format!(
                "{name}: a reshape is in progress (array sector {}); its layout is mixed and is not assembled",
                fresh.reshape_position_sectors
            ));
            false
        }
        Some(_) if conflict || slot_count == 0 || raid_disks > MAX_RAID_DISKS => false,
        Some(_) if absent.is_empty() => true,
        Some(level) if !level.tolerates(slot_count, &absent) => false,
        Some(_) => {
            if allow_degraded || !absent.iter().any(expected_back) {
                true
            } else {
                degradable = true;
                false
            }
        }
    };
    if let (true, Some(level)) = (readable, level) {
        match build(level, &fresh, &members, &slots) {
            Ok(built) => {
                if let Some(from) = built.zone_layout_from {
                    if recorded_zone_layout(&fresh).is_none() {
                        diagnostics.push(format!(
                            "{name}: RAID0 members differ in size and the metadata does not record the zone layout; data past byte {from} assumes the original layout (arrays created on Linux 3.14–5.3 use the alternate one)"
                        ));
                    }
                }
                if !absent.is_empty() {
                    diagnostics.push(format!(
                        "{name}: assembled without slot(s) {absent:?}; their data comes from the remaining {}",
                        if matches!(level, ArrayLevel::Raid1 | ArrayLevel::Raid10(_)) {
                            "mirrors"
                        } else {
                            "members and parity"
                        }
                    ));
                    if fresh.resync_offset_sectors != u64::MAX {
                        diagnostics.push(format!(
                            "{name}: the array was not stopped cleanly (in sync up to sector {}); rebuilt data from around that time may be inconsistent",
                            fresh.resync_offset_sectors
                        ));
                    }
                }
                volume = Some(built);
            }
            Err(error) => diagnostics.push(format!("{name}: {error:#}")),
        }
    }

    let reconstructed_parity = volume.is_some()
        && matches!(
            level,
            Some(ArrayLevel::Raid4 | ArrayLevel::Raid5(_) | ArrayLevel::Raid6(_))
        );
    let array_members = members
        .iter()
        .zip(&classified)
        .enumerate()
        .map(|(index, (member, class))| {
            let state = if duplicates.contains(&index) {
                RaidMemberState::Duplicate
            } else {
                class.state
            };
            RaidArrayMember {
                dev_number: member.sb.dev_number,
                slot: class.slot,
                device_uuid: mdadm_super::format_uuid(&member.sb.device_uuid),
                data_size_sectors: member.sb.data_size_sectors,
                reconstructed: reconstructed_parity
                    && matches!(state, RaidMemberState::Stale | RaidMemberState::Rebuilding)
                    && class
                        .slot
                        .is_some_and(|slot| absent.contains(&(slot as usize))),
                state,
                events: Some(member.sb.events),
                origin: member.origin.clone(),
            }
        })
        .collect();
    let assembled_bytes = volume
        .as_ref()
        .map_or_else(|| estimate_capacity(&fresh), |built| built.size_bytes);
    let array = RaidArray {
        set_uuid: set_uuid.to_string(),
        set_name: fresh.set_name.clone(),
        level: fresh.level,
        layout: fresh.layout,
        chunksize_sectors: fresh.chunksize_sectors,
        raid_disks,
        data_offset_sectors: fresh.data_offset_sectors,
        assembled_bytes,
        members: array_members,
        assembled_image_id: None,
        heuristic: false,
        metadata_version: fresh.version.to_string(),
        degraded: volume.is_some() && !absent.is_empty(),
        degradable,
    };
    Some(MdOutcome {
        array,
        volume,
        absent_slots: absent,
        diagnostics,
    })
}

fn classify(
    member: &AssembledMember,
    fresh: &MdadmSuperblock,
    name: &str,
    diagnostics: &mut Vec<String>,
) -> Classified {
    let sb = &member.sb;
    let origin = describe_origin(&member.origin);
    // The freshest table is authoritative for roles; a device missing from it
    // keeps its own.
    let role = fresh
        .role_of(sb.dev_number)
        .or_else(|| sb.role_of(sb.dev_number))
        .unwrap_or(ROLE_SPARE);
    let slot = (role < ROLE_MAX && u32::from(role) < fresh.raid_disks).then_some(u32::from(role));
    let state = match (role, slot) {
        (ROLE_SPARE, _) => RaidMemberState::Spare,
        (ROLE_FAULTY, _) => RaidMemberState::Faulty,
        (ROLE_JOURNAL, _) => RaidMemberState::Journal,
        (_, None) => {
            diagnostics.push(format!("{name}: {origin} has invalid role {role:#06x}"));
            RaidMemberState::Faulty
        }
        (_, Some(_)) => {
            let geometry_matches = sb.level == fresh.level
                && sb.raid_disks == fresh.raid_disks
                && sb.layout == fresh.layout
                && sb.chunksize_sectors == fresh.chunksize_sectors;
            if !geometry_matches {
                diagnostics.push(format!(
                    "{name}: {origin} records a different geometry than the freshest superblock; treated as stale"
                ));
                RaidMemberState::Stale
            } else if sb.events.saturating_add(1) < fresh.events {
                diagnostics.push(format!(
                    "{name}: {origin} is stale (events {} < {}); it is not read",
                    sb.events, fresh.events
                ));
                RaidMemberState::Stale
            } else if sb.recovering
                || sb.feature_map & (FEATURE_RECOVERY_OFFSET | FEATURE_REPLACEMENT) != 0
            {
                diagnostics.push(format!(
                    "{name}: {origin} was still being rebuilt (recovered up to sector {}); it is not read",
                    sb.recovery_offset_sectors
                ));
                RaidMemberState::Rebuilding
            } else {
                RaidMemberState::Active
            }
        }
    };
    if !sb.checksum_valid {
        diagnostics.push(format!("{name}: {origin} superblock checksum mismatch"));
    }
    Classified { state, slot }
}

/// RAID0 zone layout recorded in the metadata, if any.
fn recorded_zone_layout(sb: &MdadmSuperblock) -> Option<Raid0ZoneLayout> {
    let recorded = sb.major_version == 0 || sb.feature_map & FEATURE_RAID0_LAYOUT != 0;
    recorded
        .then(|| Raid0ZoneLayout::from_raw(sb.layout))
        .flatten()
}

fn build(
    level: ArrayLevel,
    fresh: &MdadmSuperblock,
    members: &[AssembledMember],
    slots: &[Option<usize>],
) -> anyhow::Result<BuiltVolume> {
    let readers = slots
        .iter()
        .map(|slot| {
            slot.map(|index| {
                let sb = &members[index].sb;
                StripedMember {
                    reader: members[index].reader.clone(),
                    data_offset_bytes: sb.data_offset_sectors.saturating_mul(512),
                    data_size_bytes: sb.data_size_sectors.saturating_mul(512),
                }
            })
        })
        .collect();
    let device_bytes = match level {
        ArrayLevel::Linear | ArrayLevel::Raid0 => None,
        _ => (fresh.size_sectors > 0).then(|| fresh.size_sectors.saturating_mul(512)),
    };
    let spec = ArraySpec {
        level,
        chunk_bytes: u64::from(fresh.chunksize_sectors) * 512,
        device_bytes,
        zone_layout: recorded_zone_layout(fresh).unwrap_or(Raid0ZoneLayout::Original),
    };
    build_volume(&spec, readers)
}

/// Array size for a set that is not assembled, from the authoritative
/// superblock alone.
fn estimate_capacity(sb: &MdadmSuperblock) -> u64 {
    let disks = u64::from(sb.raid_disks);
    let chunk = u64::from(sb.chunksize_sectors) * 512;
    let device = if sb.size_sectors > 0 {
        sb.size_sectors
    } else {
        sb.data_size_sectors
    }
    .saturating_mul(512);
    let whole_chunks = if chunk > 0 {
        device - device % chunk
    } else {
        device
    };
    match ArrayLevel::from_md(sb.level, sb.layout) {
        Some(ArrayLevel::Linear | ArrayLevel::Raid0) => sb
            .data_size_sectors
            .saturating_mul(512)
            .saturating_mul(disks),
        Some(ArrayLevel::Raid1) | None => device,
        Some(ArrayLevel::Raid4 | ArrayLevel::Raid5(_)) => {
            whole_chunks.saturating_mul(disks.saturating_sub(1))
        }
        Some(ArrayLevel::Raid6(_)) => whole_chunks.saturating_mul(disks.saturating_sub(2)),
        Some(ArrayLevel::Raid10(layout)) => {
            device.saturating_mul(disks) / u64::from(layout.copies().max(1))
        }
    }
}

#[cfg(test)]
#[path = "md_array_tests.rs"]
mod tests;
