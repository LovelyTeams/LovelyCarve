//! Assembly of SNIA DDF containers.
//!
//! Every DDF disk records its own reference number, and each virtual disk's
//! configuration record lists the reference number of the disk in every
//! position, so members are matched to slots by the metadata alone. The
//! global records (disk states, virtual disk names and states) come from the
//! disk whose header has the highest sequence number; a virtual disk's
//! geometry comes from its configuration record with the highest sequence
//! number on any loaded disk. A virtual disk with a secondary level (RAID10
//! as mirrored basic virtual disks striped together) has one record per basic
//! virtual disk (BVD); position `i` of BVD `s` is slot `s × n + i` of the md
//! RAID10 the volume is read as.
use std::collections::BTreeMap;

use super::ddf::{
    DdfConfig, DdfMetadata, DdfVirtualDisk, NO_REFNUM, PD_FAILED, PD_MISSING, PD_ONLINE,
    PD_REBUILDING, VD_INCONSISTENT, VD_INIT_FULL, VD_INIT_MASK, VD_MORPHING, VD_STATE_DELETED,
    VD_STATE_MASK,
};
use super::ddf_geometry::{bvd_mismatch, ddf_level, display_level, recorded_bytes};
use super::firmware::*;
use super::source::ArrayLevel;
use super::*;

pub(super) struct DdfMember {
    pub(super) metadata: DdfMetadata,
    pub(super) disk: FirmwareDisk,
}

/// How a loaded disk relates to the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// The disk with this reference number.
    Disk(u32),
    /// The same disk as the member at this position, loaded twice.
    Duplicate(usize),
    /// Different disks claim this reference number.
    Conflict(u32),
}

#[derive(Debug, Clone, Copy)]
struct Placement {
    slot: Option<u32>,
    state: RaidMemberState,
}

pub(super) fn assemble_ddf(
    members: Vec<DdfMember>,
    options: &DetectOptions,
    out: &mut FirmwareOutput,
) {
    let mut containers: BTreeMap<[u8; 24], Vec<DdfMember>> = BTreeMap::new();
    for member in members {
        containers
            .entry(member.metadata.header.guid)
            .or_default()
            .push(member);
    }
    for (guid, members) in containers {
        assemble_container(&guid, &members, options, out);
    }
}

fn assemble_container(
    guid: &[u8; 24],
    members: &[DdfMember],
    options: &DetectOptions,
    out: &mut FirmwareOutput,
) {
    let container = format!("DDF container {}", guid_text(guid));
    // The disk whose header was updated last holds the global records.
    let Some(best) = members
        .iter()
        .enumerate()
        .fold(None::<(usize, u32)>, |best, (index, member)| {
            let seq = member.metadata.header.effective_seq();
            match best {
                Some((_, best_seq)) if best_seq >= seq => best,
                _ => Some((index, seq)),
            }
        })
        .map(|(index, _)| index)
    else {
        return;
    };
    let global = &members[best].metadata;
    let identities = identify(members, &container, &mut out.diagnostics);
    for member in members {
        let refnum = member.metadata.disk_refnum;
        if !global.phys_disks.iter().any(|disk| disk.refnum == refnum) {
            out.diagnostics.push(format!(
                "{container}: {} (reference {refnum:#010x}) is not in the physical disk records",
                member.disk.describe()
            ));
        }
        for config in &member.metadata.configs {
            if !global
                .virtual_disks
                .iter()
                .any(|virtual_disk| virtual_disk.guid == config.guid)
            {
                out.diagnostics.push(format!(
                    "{container}: {} holds a configuration record for virtual disk {}, which the virtual disk records do not list; ignored",
                    member.disk.describe(),
                    guid_text(&config.guid)
                ));
            }
        }
    }
    if global.virtual_disks.is_empty() {
        out.diagnostics.push(format!(
            "{container} on {} disk(s) holds no virtual disks",
            members.len()
        ));
    }
    let context = ContainerContext {
        global,
        members,
        identities: &identities,
        best_seq: global.header.effective_seq(),
        revision: &global.header.revision,
    };
    for virtual_disk in &global.virtual_disks {
        if virtual_disk.state & VD_STATE_MASK == VD_STATE_DELETED {
            continue;
        }
        if let Some(outcome) = context.assemble(virtual_disk, options, &mut out.diagnostics) {
            out.volumes.push(outcome);
        }
    }
}

/// One identity per member; a reference number claimed twice is the same
/// disk loaded twice when the copies are identical, otherwise a conflict.
fn identify(
    members: &[DdfMember],
    container: &str,
    diagnostics: &mut Vec<String>,
) -> Vec<Identity> {
    let mut identities = Vec::with_capacity(members.len());
    for (position, member) in members.iter().enumerate() {
        let refnum = member.metadata.disk_refnum;
        let earlier = members[..position]
            .iter()
            .position(|other| other.metadata.disk_refnum == refnum);
        let identity = match earlier {
            None => Identity::Disk(refnum),
            Some(first) => {
                let original = &members[first];
                if original.metadata == member.metadata
                    && original.disk.size() == member.disk.size()
                {
                    Identity::Duplicate(first)
                } else {
                    diagnostics.push(format!(
                        "{container}: {} and {} both claim disk reference {refnum:#010x}",
                        original.disk.describe(),
                        member.disk.describe()
                    ));
                    if let Some(slot) = identities.get_mut(first) {
                        *slot = Identity::Conflict(refnum);
                    }
                    Identity::Conflict(refnum)
                }
            }
        };
        identities.push(identity);
    }
    identities
}

struct ContainerContext<'a> {
    global: &'a DdfMetadata,
    members: &'a [DdfMember],
    identities: &'a [Identity],
    best_seq: u32,
    revision: &'a str,
}

impl ContainerContext<'_> {
    fn assemble(
        &self,
        virtual_disk: &DdfVirtualDisk,
        options: &DetectOptions,
        diagnostics: &mut Vec<String>,
    ) -> Option<VolumeOutcome> {
        let uuid = hex(&virtual_disk.guid);
        let set_name = virtual_disk.name.clone();
        let name = format!(
            "ddf/{}",
            if set_name.is_empty() {
                &uuid
            } else {
                &set_name
            }
        );
        // Per BVD, the freshest configuration record on any loaded disk.
        let mut bvds: BTreeMap<u8, &DdfConfig> = BTreeMap::new();
        for config in self
            .members
            .iter()
            .flat_map(|member| &member.metadata.configs)
            .filter(|config| config.guid == virtual_disk.guid)
        {
            match bvds.get(&config.sec_elmnt_seq) {
                Some(kept) if kept.seqnum >= config.seqnum => {}
                _ => {
                    bvds.insert(config.sec_elmnt_seq, config);
                }
            }
        }
        let Some(&primary) = bvds.values().next() else {
            diagnostics.push(format!(
                "{name}: none of the loaded disks carries its configuration record"
            ));
            return None;
        };

        let mut notes = Vec::new();
        let mut blocked = Vec::new();
        let level = ddf_level(primary);
        let level = match level {
            Ok(level) => Some(level),
            Err(reason) => {
                blocked.push(reason);
                None
            }
        };
        let sec_count = primary.sec_elmnt_count.max(1);
        let prim = usize::from(primary.prim_elmnt_count);
        if let Some(reason) = bvds
            .values()
            .find_map(|bvd| bvd_mismatch(primary, bvd, sec_count))
        {
            blocked.push(reason);
        }
        if primary.chunk_shift >= 32 {
            blocked.push(format!(
                "strip size 2^{} sectors is out of range",
                primary.chunk_shift
            ));
        }
        // A concatenation records one extent size; disks of other sizes
        // would shift every later member.
        if level == Some(ArrayLevel::Linear)
            && Some(primary.array_blocks)
                != primary
                    .blocks
                    .checked_mul(u64::from(primary.prim_elmnt_count))
        {
            blocked.push(
                "its concatenated disks contribute different sizes, which the configuration record does not describe".into(),
            );
        }
        if virtual_disk.state & VD_MORPHING != 0 {
            blocked.push(
                "a reconfiguration (morphing) is in progress; old and new layouts are mixed".into(),
            );
        }
        let parity_doubt = if virtual_disk.state & VD_INCONSISTENT != 0 {
            Some("the virtual disk is marked inconsistent".to_string())
        } else if virtual_disk.init_state & VD_INIT_MASK != VD_INIT_FULL {
            Some("the virtual disk was not fully initialised".to_string())
        } else {
            None
        };
        match virtual_disk.state & VD_STATE_MASK {
            0 => {}
            1 => notes.push("the controller recorded it as degraded".to_string()),
            3 => notes.push("the controller recorded it as missing".to_string()),
            4 => notes.push("the controller recorded it as failed".to_string()),
            5 => notes.push("the controller recorded it as partially optimal".to_string()),
            other => notes.push(format!("the controller recorded state {other}")),
        }

        let raid_disks = prim * usize::from(sec_count);
        let mut placed: Vec<Option<Placement>> = vec![None; self.members.len()];
        let mut slots = Vec::with_capacity(raid_disks);
        for seq in 0..sec_count {
            let bvd = bvds.get(&seq);
            if let Some(bvd) = bvd {
                if bvd
                    .phys_refnum
                    .iter()
                    .skip(prim)
                    .any(|&refnum| refnum != NO_REFNUM)
                {
                    blocked.push(format!(
                        "configuration record {seq} lists more disks than its {prim} positions"
                    ));
                }
            } else {
                notes.push(format!(
                    "no loaded disk carries the configuration record of BVD {seq}"
                ));
            }
            for position in 0..prim {
                let slot = usize::from(seq) * prim + position;
                let source = match bvd {
                    Some(bvd) => {
                        self.slot_source(bvd, position, slot, &mut placed, &mut notes, &mut blocked)
                    }
                    None => SlotSource::Expected,
                };
                slots.push(source);
            }
        }
        for (position, identity) in self.identities.iter().enumerate() {
            match *identity {
                Identity::Disk(refnum) if placed[position].is_none() => {
                    let spare = self
                        .global
                        .phys_disks
                        .iter()
                        .any(|disk| disk.refnum == refnum && disk.is_spare());
                    if spare {
                        placed[position] = Some(Placement {
                            slot: None,
                            state: RaidMemberState::Spare,
                        });
                    }
                }
                Identity::Duplicate(original) => {
                    placed[position] = placed[original].map(|placement| Placement {
                        slot: placement.slot,
                        state: RaidMemberState::Duplicate,
                    });
                }
                Identity::Disk(_) | Identity::Conflict(_) => {}
            }
        }

        diagnostics.extend(notes.into_iter().map(|note| format!("{name}: {note}")));
        let chunk_bytes = 512u64 << primary.chunk_shift.min(31);
        let component_bytes = primary.blocks.saturating_mul(512);
        let assembly = match level {
            Some(level) if blocked.is_empty() => {
                let geometry = VolumeGeometry {
                    level,
                    chunk_bytes,
                    component_bytes,
                    recorded_bytes: recorded_bytes(primary, sec_count),
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

        let assembled = assembly.volume.is_some();
        let parity = matches!(
            level,
            Some(ArrayLevel::Raid4 | ArrayLevel::Raid5(_) | ArrayLevel::Raid6(_))
        );
        let array_members = placed
            .iter()
            .enumerate()
            .filter_map(|(position, placement)| {
                let placement = (*placement)?;
                let member = &self.members[position];
                Some(RaidArrayMember {
                    dev_number: member.metadata.disk_refnum,
                    slot: placement.slot,
                    device_uuid: guid_text(&member.metadata.disk_guid),
                    data_size_sectors: primary.blocks,
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
                    events: Some(u64::from(member.metadata.header.effective_seq())),
                    origin: member.disk.origin.clone(),
                })
            })
            .collect();
        let (md_level, md_layout) = level.map_or((display_level(primary), 0), |level| {
            (level.md_level(), level.md_layout())
        });
        let array = RaidArray {
            set_uuid: uuid.clone(),
            set_name: set_name.clone(),
            level: md_level,
            layout: md_layout,
            chunksize_sectors: 1u32 << primary.chunk_shift.min(31),
            raid_disks: raid_disks as u32,
            data_offset_sectors: primary
                .lba_offset
                .first()
                .copied()
                .filter(|&offset| offset != u64::MAX)
                .unwrap_or(0),
            assembled_bytes: assembly.volume.as_ref().map_or_else(
                || primary.array_blocks.saturating_mul(512),
                |built| built.size_bytes,
            ),
            members: array_members,
            assembled_image_id: None,
            heuristic: false,
            metadata_version: format!("ddf {}", self.revision),
            degraded: assembled && !assembly.absent.is_empty(),
            degradable: assembly.degradable,
        };
        Some(VolumeOutcome {
            array,
            volume: assembly.volume,
            absent_slots: assembly.absent,
            tag: name,
            label: volume_label("ddf", &set_name, &uuid),
        })
    }

    /// Where the data of position `bvd_position` of `bvd` (array slot
    /// `slot`) comes from, recording the member found there.
    fn slot_source(
        &self,
        bvd: &DdfConfig,
        bvd_position: usize,
        slot: usize,
        placed: &mut [Option<Placement>],
        notes: &mut Vec<String>,
        blocked: &mut Vec<String>,
    ) -> SlotSource {
        let refnum = bvd
            .phys_refnum
            .get(bvd_position)
            .copied()
            .unwrap_or(NO_REFNUM);
        if refnum == NO_REFNUM {
            notes.push(format!("slot {slot} has no disk assigned"));
            return SlotSource::Recorded;
        }
        let claimant = self
            .identities
            .iter()
            .position(|identity| *identity == Identity::Disk(refnum));
        let mut place = |member_index: usize, state| {
            placed[member_index] = Some(Placement {
                slot: Some(slot as u32),
                state,
            });
        };
        if self.identities.contains(&Identity::Conflict(refnum)) {
            blocked.push(format!("different disks claim the disk of slot {slot}"));
            for (member_index, identity) in self.identities.iter().enumerate() {
                if *identity == Identity::Conflict(refnum) {
                    place(member_index, RaidMemberState::Active);
                }
            }
            return SlotSource::Expected;
        }
        let Some(record) = self
            .global
            .phys_disks
            .iter()
            .find(|disk| disk.refnum == refnum)
        else {
            notes.push(format!(
                "slot {slot} names disk {refnum:#010x}, which the physical disk records do not hold"
            ));
            if let Some(member_index) = claimant {
                place(member_index, RaidMemberState::Faulty);
            }
            return SlotSource::Expected;
        };
        if record.state & PD_REBUILDING != 0 && record.state & PD_FAILED == 0 {
            if let Some(member_index) = claimant {
                notes.push(format!(
                    "{} was still being rebuilt into slot {slot}; it is not read",
                    self.members[member_index].disk.describe()
                ));
                place(member_index, RaidMemberState::Rebuilding);
            }
            return SlotSource::Recorded;
        }
        if !record.in_sync() {
            let reason = if record.state & PD_FAILED != 0 {
                "failed"
            } else if record.state & PD_MISSING != 0 {
                "missing"
            } else if record.state & PD_ONLINE == 0 {
                "offline"
            } else {
                "not in sync"
            };
            notes.push(format!(
                "the disk of slot {slot} ({refnum:#010x}) is recorded as {reason}"
            ));
            if let Some(member_index) = claimant {
                place(member_index, RaidMemberState::Faulty);
            }
            return SlotSource::Recorded;
        }
        let Some(member_index) = claimant else {
            return SlotSource::Expected;
        };
        let member = &self.members[member_index];
        let own_seq = member.metadata.header.effective_seq();
        if is_stale(u64::from(own_seq), u64::from(self.best_seq)) {
            notes.push(format!(
                "{} is stale (sequence {own_seq} < {}); it is not read",
                member.disk.describe(),
                self.best_seq
            ));
            place(member_index, RaidMemberState::Stale);
            return SlotSource::Expected;
        }
        let offset = bvd
            .lba_offset
            .get(bvd_position)
            .copied()
            .unwrap_or(u64::MAX);
        if offset == u64::MAX {
            notes.push(format!("slot {slot} has no recorded start sector"));
            place(member_index, RaidMemberState::Faulty);
            return SlotSource::Expected;
        }
        place(member_index, RaidMemberState::Active);
        SlotSource::Read(member.disk.member(offset, bvd.blocks))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A GUID as text when it is printable ASCII (controllers build disk GUIDs
/// from vendor, model and serial), otherwise as hex.
fn guid_text(guid: &[u8; 24]) -> String {
    if guid
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(guid).trim().to_string()
    } else {
        hex(guid)
    }
}
