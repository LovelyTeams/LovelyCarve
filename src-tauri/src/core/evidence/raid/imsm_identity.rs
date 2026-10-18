//! Which disk record of an IMSM container each loaded image is.
//!
//! IMSM names its member disks only by their drive serial numbers, and a disk
//! does not store its own serial: the drive reports it. A loaded image is
//! therefore matched to its disk record by the serial number its acquisition
//! recorded (the EWF header), or by its size when exactly one disk record has
//! that size. Images matched neither way stay unknown; [`imsm_array`] decides
//! where an unknown image can still be read.
//!
//! [`imsm_array`]: super::imsm_array
use std::collections::{BTreeMap, BTreeSet};

use super::imsm::{normalize_serial, serial_eq, ImsmSuper};
use super::imsm_array::ImsmMember;

/// How a loaded image relates to the authoritative disk table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Identity {
    /// The disk record at this index.
    Disk(usize),
    /// The same disk as the member at this position, loaded twice.
    Duplicate(usize),
    /// Different images claim the disk record at this index.
    Conflict(usize),
    /// No disk record can be told to be this image.
    Unknown,
}

/// Match every member to a disk record of the authoritative table `mpb`.
pub(super) fn identify(
    mpb: &ImsmSuper,
    members: &[ImsmMember],
    container: &str,
    diagnostics: &mut Vec<String>,
) -> Vec<Identity> {
    let mut claims: Vec<Option<usize>> = Vec::with_capacity(members.len());
    let mut by_serial = vec![false; members.len()];
    for (position, member) in members.iter().enumerate() {
        let Some(raw) = member.disk.drive_serial() else {
            claims.push(None);
            continue;
        };
        let found: Vec<usize> = normalize_serial(raw)
            .map(|serial| {
                mpb.disks
                    .iter()
                    .enumerate()
                    .filter(|(_, disk)| serial_eq(&disk.serial, &serial))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
        if let [index] = found[..] {
            claims.push(Some(index));
            by_serial[position] = true;
        } else {
            diagnostics.push(format!(
                "{container}: drive serial {raw:?} recorded for {} matches no disk record",
                member.disk.describe()
            ));
            claims.push(None);
        }
    }
    // A size identifies a record only when no other record has that size and
    // no serial claimed it.
    let serial_claimed: BTreeSet<usize> = claims.iter().flatten().copied().collect();
    for (member, claim) in members.iter().zip(claims.iter_mut()) {
        if claim.is_some() {
            continue;
        }
        let sectors = member.disk.size() / 512;
        let sized: Vec<usize> = mpb
            .disks
            .iter()
            .enumerate()
            .filter(|(_, disk)| disk.total_sectors == sectors)
            .map(|(index, _)| index)
            .collect();
        if let [index] = sized[..] {
            if !serial_claimed.contains(&index) {
                *claim = Some(index);
            }
        }
    }

    // One image per record: a second image with an identical metadata copy
    // is the same disk loaded twice.
    let mut identities = vec![Identity::Unknown; members.len()];
    let mut by_disk: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (position, claim) in claims.iter().enumerate() {
        if let Some(index) = claim {
            by_disk.entry(*index).or_default().push(position);
        }
    }
    for (index, claimants) in by_disk {
        let Some((&first, rest)) = claimants.split_first() else {
            continue;
        };
        let copies = rest.iter().all(|&other| {
            let (a, b) = (&members[first], &members[other]);
            a.mpb.checksum == b.mpb.checksum
                && a.mpb.generation_num == b.mpb.generation_num
                && a.disk.size() == b.disk.size()
        });
        if copies {
            identities[first] = Identity::Disk(index);
            for &other in rest {
                identities[other] = Identity::Duplicate(first);
            }
        } else if by_serial[first] {
            diagnostics.push(format!(
                "{container}: different images claim disk record {index} by drive serial"
            ));
            for &claimant in &claimants {
                identities[claimant] = Identity::Conflict(index);
            }
        }
        // Different images of a size only one record has stay unknown.
    }
    identities
}
