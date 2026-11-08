//! Assembly of Intel IMSM containers: the authoritative copy of the
//! metadata, and what each of its volumes reads.
//!
//! Loaded images are matched to disk records by [`imsm_identity`]. Images no
//! record could be matched to can stand in for one another only where that
//! cannot change what is read: the mirrors of a RAID1 volume whose unmatched
//! disk records are all in-sync members of it. Any other volume that would
//! need an unmatched image is listed, with the geometry the manual builder
//! needs, but not assembled.
//!
//! [`imsm_identity`]: super::imsm_identity
use std::collections::{BTreeMap, BTreeSet};

use super::firmware::*;
use super::imsm::{
    ImsmMap, ImsmSuper, ImsmVolume, MAP_UNINITIALIZED, MIGR_GEN_MIGR, MIGR_INIT, MIGR_REBUILD,
    MIGR_REPAIR, MIGR_STATE_CHANGE, MIGR_VERIFY, VOLUME_DIRTY,
};
use super::imsm_identity::{identify, Identity};
use super::mdadm_super::format_uuid;
use super::source::{ArrayLevel, Raid10Layout, Raid5Layout};
use super::*;

pub(super) struct ImsmMember {
    pub(super) mpb: ImsmSuper,
    pub(super) disk: FirmwareDisk,
}

/// Where a member appears in one volume's member list.
#[derive(Debug, Clone, Copy)]
struct Placement {
    slot: Option<u32>,
    state: RaidMemberState,
}

pub(super) fn assemble_imsm(
    members: Vec<ImsmMember>,
    options: &DetectOptions,
    out: &mut FirmwareOutput,
) {
    let mut families: BTreeMap<u32, Vec<ImsmMember>> = BTreeMap::new();
    for member in members {
        families
            .entry(member.mpb.family_num)
            .or_default()
            .push(member);
    }
    for (family, members) in families {
        assemble_container(family, &members, options, out);
    }
}

fn assemble_container(
    family: u32,
    members: &[ImsmMember],
    options: &DetectOptions,
    out: &mut FirmwareOutput,
) {
    let container = format!("IMSM container {family:08x}");
    // The copy with the highest generation is authoritative; the first wins
    // ties.
    let Some(champion) = members
        .iter()
        .enumerate()
        .fold(None::<(usize, u32)>, |best, (index, member)| match best {
            Some((_, generation)) if generation >= member.mpb.generation_num => best,
            _ => Some((index, member.mpb.generation_num)),
        })
        .map(|(index, _)| index)
    else {
        return;
    };
    let mpb = &members[champion].mpb;
    for member in members {
        if member.mpb.generation_num == mpb.generation_num && member.mpb.checksum != mpb.checksum {
            out.diagnostics.push(format!(
                "{container}: {} carries a different copy of generation {}; the copy on {} is used",
                member.disk.describe(),
                mpb.generation_num,
                members[champion].disk.describe()
            ));
        }
    }
    let unsupported = mpb.unsupported_attributes();
    if unsupported != 0 {
        out.diagnostics.push(format!(
            "{container}: metadata attributes {unsupported:#010x} (such as SSD caching or Clone-and-Go) are not supported; its volumes are listed but not assembled"
        ));
    }
    let identities = identify(mpb, members, &container, &mut out.diagnostics);
    if mpb.volumes.is_empty() {
        out.diagnostics.push(format!(
            "{container} on {} disk(s) holds no volumes",
            members.len()
        ));
    }
    for volume in &mpb.volumes {
        let context = VolumeContext {
            mpb,
            members,
            identities: &identities,
            attributes_supported: unsupported == 0,
        };
        let outcome = context.assemble(volume, options, &mut out.diagnostics);
        out.volumes.push(outcome);
    }
}

struct VolumeContext<'a> {
    mpb: &'a ImsmSuper,
    members: &'a [ImsmMember],
    identities: &'a [Identity],
    attributes_supported: bool,
}

impl VolumeContext<'_> {
    fn assemble(
        &self,
        volume: &ImsmVolume,
        options: &DetectOptions,
        diagnostics: &mut Vec<String>,
    ) -> VolumeOutcome {
        let mpb = self.mpb;
        let uuid = format_uuid(&mpb.volume_uuid(volume));
        let set_name = volume.name_text();
        let name = format!(
            "imsm/{}",
            if set_name.is_empty() {
                &uuid
            } else {
                &set_name
            }
        );
        let map = &volume.map;
        let current = volume.current_map();
        let raid_disks = usize::from(map.num_members);
        let level = imsm_level(map);
        let component = map.component_sectors(volume.size_sectors).unwrap_or(0);

        let mut notes = Vec::new();
        let mut blocked = Vec::new();
        let mut parity_doubt = None;
        if !self.attributes_supported {
            blocked.push("the container uses metadata features that are not supported".to_string());
        }
        if volume.migrating {
            match volume.migr_type {
                MIGR_GEN_MIGR => blocked.push(format!(
                    "a migration (level, member count, size or strip change) is in progress at unit {}; old and new layouts are mixed",
                    volume.curr_migr_unit
                )),
                MIGR_STATE_CHANGE => blocked.push("a state-change migration is in progress".into()),
                MIGR_INIT => {
                    parity_doubt = Some(format!(
                        "parity initialisation had reached unit {}",
                        volume.curr_migr_unit
                    ));
                }
                MIGR_REBUILD | MIGR_VERIFY | MIGR_REPAIR => {}
                other => blocked.push(format!("migration type {other} is not recognised")),
            }
            if volume.migr_type != MIGR_GEN_MIGR && !same_layout(map, current) {
                blocked.push("its two migration maps describe different layouts".into());
            }
        }
        if parity_doubt.is_none() && current.map_state == MAP_UNINITIALIZED {
            parity_doubt = Some("the volume was never initialised".into());
        }
        if parity_doubt.is_none() && volume.dirty & VOLUME_DIRTY != 0 {
            parity_doubt = Some("the volume was not shut down cleanly".into());
        }
        if level.is_none() {
            blocked.push(format!(
                "IMSM level {} with {raid_disks} member(s) is not supported",
                map.raw_level
            ));
        } else if component == 0 {
            blocked.push("its member size cannot be determined".into());
        }

        let mut placed: Vec<Option<Placement>> = vec![None; self.members.len()];
        let mut slots = Vec::with_capacity(raid_disks);
        let mut open = Vec::new();
        for slot in 0..raid_disks {
            let slot_number = Some(slot as u32);
            let Some(index) = map.disk_index(slot) else {
                slots.push(SlotSource::Recorded);
                continue;
            };
            let claimant = self
                .identities
                .iter()
                .position(|identity| *identity == Identity::Disk(index));
            let conflicted = self.identities.contains(&Identity::Conflict(index));
            let mut place = |position: usize, state| {
                placed[position] = Some(Placement {
                    slot: slot_number,
                    state,
                });
            };
            match mpb.disks.get(index) {
                None => {
                    notes.push(format!(
                        "slot {slot} names disk record {index}, which the disk table does not hold"
                    ));
                    slots.push(SlotSource::Recorded);
                }
                Some(disk) if disk.is_failed() => {
                    if let Some(position) = claimant {
                        place(position, RaidMemberState::Faulty);
                    }
                    slots.push(SlotSource::Recorded);
                }
                Some(_) if current.rebuilding(slot) => {
                    if let Some(position) = claimant {
                        notes.push(format!(
                            "{} was still being rebuilt into slot {slot}; it is not read",
                            self.members[position].disk.describe()
                        ));
                        place(position, RaidMemberState::Rebuilding);
                    }
                    slots.push(SlotSource::Recorded);
                }
                Some(_) if conflicted => {
                    blocked.push(format!("different images claim the disk of slot {slot}"));
                    for (position, identity) in self.identities.iter().enumerate() {
                        if *identity == Identity::Conflict(index) {
                            place(position, RaidMemberState::Active);
                        }
                    }
                    slots.push(SlotSource::Expected);
                }
                Some(_) => match claimant {
                    None => {
                        open.push(slot);
                        slots.push(SlotSource::Expected);
                    }
                    Some(position) => {
                        let member = &self.members[position];
                        if is_stale(
                            u64::from(member.mpb.generation_num),
                            u64::from(mpb.generation_num),
                        ) {
                            notes.push(format!(
                                "{} is stale (generation {} < {}); it is not read",
                                member.disk.describe(),
                                member.mpb.generation_num,
                                mpb.generation_num
                            ));
                            place(position, RaidMemberState::Stale);
                            slots.push(SlotSource::Expected);
                        } else {
                            place(position, RaidMemberState::Active);
                            slots.push(SlotSource::Read(
                                member.disk.member(map.pba_of_lba0, component),
                            ));
                        }
                    }
                },
            }
        }

        // Images no disk record could be matched to.
        let (current_unknown, stale_unknown): (Vec<usize>, Vec<usize>) = (0..self.members.len())
            .filter(|&position| self.identities[position] == Identity::Unknown)
            .partition(|&position| {
                !is_stale(
                    u64::from(self.members[position].mpb.generation_num),
                    u64::from(mpb.generation_num),
                )
            });
        for position in stale_unknown {
            placed[position] = Some(Placement {
                slot: None,
                state: RaidMemberState::Stale,
            });
        }
        if !current_unknown.is_empty() {
            let interchangeable = level == Some(ArrayLevel::Raid1)
                && current_unknown.len() <= open.len()
                && self.unmatched_records_are(&open, map);
            if interchangeable {
                for (&slot, &position) in open.iter().zip(&current_unknown) {
                    let member = &self.members[position];
                    notes.push(format!(
                        "{} has no drive serial or unique size to match a disk record; as an in-sync RAID1 mirror it is read for slot {slot}",
                        member.disk.describe()
                    ));
                    placed[position] = Some(Placement {
                        slot: Some(slot as u32),
                        state: RaidMemberState::Active,
                    });
                    slots[slot] = SlotSource::Read(member.disk.member(map.pba_of_lba0, component));
                }
            } else {
                for &position in &current_unknown {
                    placed[position] = Some(Placement {
                        slot: None,
                        state: RaidMemberState::Unidentified,
                    });
                }
                if !open.is_empty() {
                    blocked.push(order_unknown(current_unknown.len(), level, map, raid_disks));
                }
            }
        }
        // Spares, journal disks and copies are listed too; disks of other
        // volumes only are not.
        for (position, identity) in self.identities.iter().enumerate() {
            match *identity {
                Identity::Disk(index) if placed[position].is_none() => {
                    let state = mpb.disks.get(index).and_then(|disk| {
                        if disk.is_journal() {
                            Some(RaidMemberState::Journal)
                        } else if disk.is_spare() {
                            Some(RaidMemberState::Spare)
                        } else {
                            None
                        }
                    });
                    placed[position] = state.map(|state| Placement { slot: None, state });
                }
                Identity::Duplicate(original) => {
                    placed[position] = placed[original].map(|placement| Placement {
                        slot: placement.slot,
                        state: RaidMemberState::Duplicate,
                    });
                }
                Identity::Disk(_) | Identity::Conflict(_) | Identity::Unknown => {}
            }
        }

        diagnostics.extend(notes.into_iter().map(|note| format!("{name}: {note}")));
        let assembly = match level {
            Some(level) if blocked.is_empty() => {
                let geometry = VolumeGeometry {
                    level,
                    chunk_bytes: map.blocks_per_strip.saturating_mul(512),
                    component_bytes: component.saturating_mul(512),
                    recorded_bytes: Some(volume.size_sectors.saturating_mul(512)),
                    parity_doubt,
                };
                let consent = options.degraded_sets.contains(&uuid);
                assemble_slots(&name, &geometry, slots, consent, diagnostics)
            }
            _ => {
                for reason in &blocked {
                    diagnostics.push(format!("{name}: {reason}; not assembled"));
                }
                SlotAssembly {
                    volume: None,
                    absent: Vec::new(),
                    degradable: false,
                }
            }
        };

        let parity = matches!(
            level,
            Some(ArrayLevel::Raid4 | ArrayLevel::Raid5(_) | ArrayLevel::Raid6(_))
        );
        let assembled = assembly.volume.is_some();
        let array_members = placed
            .iter()
            .enumerate()
            .filter_map(|(position, placement)| {
                let placement = (*placement)?;
                let member = &self.members[position];
                let record = match self.identities[position] {
                    Identity::Disk(index) | Identity::Conflict(index) => Some(index),
                    Identity::Duplicate(original) => match self.identities[original] {
                        Identity::Disk(index) => Some(index),
                        _ => None,
                    },
                    Identity::Unknown => None,
                };
                let device_uuid = record
                    .and_then(|index| mpb.disks.get(index))
                    .map(|disk| disk.serial_text())
                    .or_else(|| member.disk.drive_serial().map(str::to_string))
                    .unwrap_or_default();
                Some(RaidArrayMember {
                    dev_number: record.map_or(u32::MAX, |index| index as u32),
                    slot: placement.slot,
                    device_uuid,
                    data_size_sectors: component,
                    reconstructed: assembled
                        && parity
                        && matches!(
                            placement.state,
                            RaidMemberState::Stale | RaidMemberState::Rebuilding
                        )
                        && placement
                            .slot
                            .is_some_and(|slot| assembly.absent.contains(&(slot as usize))),
                    state: placement.state,
                    events: Some(u64::from(member.mpb.generation_num)),
                    origin: member.disk.origin.clone(),
                })
            })
            .collect();

        let (md_level, md_layout) = level.map_or((i32::from(map.level()), 0), |level| {
            (level.md_level(), level.md_layout())
        });
        let array = RaidArray {
            set_uuid: uuid.clone(),
            set_name: set_name.clone(),
            level: md_level,
            layout: md_layout,
            chunksize_sectors: u32::try_from(map.blocks_per_strip).unwrap_or(u32::MAX),
            raid_disks: u32::from(map.num_members),
            data_offset_sectors: map.pba_of_lba0,
            assembled_bytes: assembly.volume.as_ref().map_or_else(
                || volume.size_sectors.saturating_mul(512),
                |built| built.size_bytes,
            ),
            members: array_members,
            assembled_image_id: None,
            heuristic: false,
            metadata_version: format!("imsm {}", mpb.version),
            degraded: assembled && !assembly.absent.is_empty(),
            degradable: assembly.degradable,
        };
        VolumeOutcome {
            array,
            volume: assembly.volume,
            absent_slots: assembly.absent,
            tag: name,
            label: volume_label("imsm", &set_name, &uuid),
        }
    }

    /// Whether every disk record no image was matched to fills one of the
    /// volume's `open` slots — so an unmatched image can only be one of them.
    fn unmatched_records_are(&self, open: &[usize], map: &ImsmMap) -> bool {
        let matched: BTreeSet<usize> = self
            .identities
            .iter()
            .filter_map(|identity| match identity {
                Identity::Disk(index) | Identity::Conflict(index) => Some(*index),
                Identity::Duplicate(_) | Identity::Unknown => None,
            })
            .collect();
        let open_records: BTreeSet<usize> = open
            .iter()
            .filter_map(|&slot| map.disk_index(slot))
            .collect();
        (0..self.mpb.disks.len())
            .filter(|index| !matched.contains(index))
            .all(|index| open_records.contains(&index))
    }
}

/// md level and layout of an IMSM map: RAID5 rotates parity left
/// asymmetrically, RAID10 is mirrored pairs striped (md near-2).
fn imsm_level(map: &ImsmMap) -> Option<ArrayLevel> {
    match (map.level(), map.num_members) {
        (0, members) if members >= 1 => Some(ArrayLevel::Raid0),
        (1, 2) => Some(ArrayLevel::Raid1),
        (5, members) if members >= 3 => Some(ArrayLevel::Raid5(Raid5Layout::LeftAsymmetric)),
        (10, members) if members >= 4 && members.is_multiple_of(2) => {
            Raid10Layout::from_raw(0x102).map(ArrayLevel::Raid10)
        }
        _ => None,
    }
}

/// Whether a migration's two maps describe the same layout (initialisation,
/// rebuild and verify only change slot states).
fn same_layout(a: &ImsmMap, b: &ImsmMap) -> bool {
    a.level() == b.level()
        && a.num_members == b.num_members
        && a.blocks_per_strip == b.blocks_per_strip
        && a.pba_of_lba0 == b.pba_of_lba0
}

fn order_unknown(
    images: usize,
    level: Option<ArrayLevel>,
    map: &ImsmMap,
    raid_disks: usize,
) -> String {
    let level = level.map_or_else(|| format!("level {}", map.raw_level), |level| level.label());
    format!(
        "{images} image(s) cannot be matched to disk records — IMSM identifies member disks by drive serial number, which the disks do not store and the evidence metadata does not record, and no disk size is unique — so the slot order is unknown; assemble it manually as {level}, {} KiB strips, data offset {} sectors, {raid_disks} members in slot order",
        map.blocks_per_strip / 2,
        map.pba_of_lba0
    )
}
