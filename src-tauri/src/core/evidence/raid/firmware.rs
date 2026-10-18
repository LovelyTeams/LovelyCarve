//! Firmware RAID containers: Intel IMSM and SNIA DDF.
//!
//! Unlike md, whose superblock describes one array and the member's own slot,
//! firmware RAID keeps container-wide metadata on every member disk: the
//! container's disks, every volume over them, and which disk fills each slot.
//! This pass reads that metadata from the end of every whole loaded image,
//! groups the images by container, takes the freshest copy of the metadata as
//! authoritative, matches each image to its disk record
//! ([`imsm_identity`](super::imsm_identity), [`ddf_array`](super::ddf_array))
//! and assembles every volume with the same readers as md ([`build_volume`]).
//!
//! The member and degraded rules are md's ([`md_array`](super::md_array)): a
//! member whose metadata copy is more than one update behind the freshest is
//! stale and not read; a slot whose disk the metadata records as failed,
//! missing or still being rebuilt is rebuilt from redundancy automatically; a
//! slot whose disk the metadata still expects but that is not loaded (or is
//! stale) waits for the examiner's consent, as does any rebuild from parity
//! the metadata says may not match the data.
use std::collections::HashSet;

use super::ddf::DdfProbe;
use super::ddf_array::{assemble_ddf, DdfMember};
use super::imsm::ImsmProbe;
use super::imsm_array::{assemble_imsm, ImsmMember};
use super::source::{
    build_volume, ArrayLevel, ArraySpec, BuiltVolume, ConcatVolume, Raid0ZoneLayout,
};
use super::*;

/// A whole loaded image probed for firmware RAID metadata.
#[derive(Clone)]
pub(super) struct FirmwareDisk {
    pub(super) image: Arc<DiskImage>,
    pub(super) reader: MemberReader,
    pub(super) origin: RaidMemberOrigin,
}

impl FirmwareDisk {
    fn whole(image: &Arc<DiskImage>) -> Self {
        let byte_length = image.len();
        Self {
            image: image.clone(),
            reader: Arc::new(WholeDiskMember {
                image: image.clone(),
                byte_offset: 0,
                byte_length,
            }),
            origin: RaidMemberOrigin::Whole {
                image_id: image.id.clone(),
                partition_id: 0,
                byte_offset: 0,
                byte_length,
            },
        }
    }

    pub(super) fn size(&self) -> u64 {
        self.reader.size()
    }

    /// Serial reported by the physical source, or recorded in its EWF header.
    pub(super) fn drive_serial(&self) -> Option<&str> {
        self.image
            .physical_drive_serial
            .as_ref()
            .map(crate::core::evidence::physical_disk::DriveSerial::as_str)
            .or_else(|| {
                self.image
                    .ewf_acquiry
                    .as_ref()?
                    .serial_number
                    .as_deref()
                    .filter(|serial| !serial.trim().is_empty())
            })
    }

    pub(super) fn describe(&self) -> String {
        describe_origin(&self.origin)
    }

    /// The member data area from `offset_sectors`, `size_sectors` long.
    pub(super) fn member(&self, offset_sectors: u64, size_sectors: u64) -> StripedMember {
        StripedMember {
            reader: self.reader.clone(),
            data_offset_bytes: offset_sectors.saturating_mul(512),
            data_size_bytes: size_sectors.saturating_mul(512),
        }
    }
}

/// Where the data of one slot comes from.
pub(super) enum SlotSource {
    /// Read from this member.
    Read(StripedMember),
    /// The metadata records the slot's disk as failed, missing or being
    /// rebuilt: rebuilt from redundancy without asking.
    Recorded,
    /// The metadata expects a disk here that is not available: rebuilt only
    /// with the examiner's consent.
    Expected,
}

/// Geometry of a firmware volume in md terms.
pub(super) struct VolumeGeometry {
    pub(super) level: ArrayLevel,
    pub(super) chunk_bytes: u64,
    /// Bytes of each member the volume uses.
    pub(super) component_bytes: u64,
    /// Volume size the metadata records; a larger assembled device is cut to
    /// it.
    pub(super) recorded_bytes: Option<u64>,
    /// Why the parity may not match the data (unclean shutdown, unfinished
    /// initialisation), if it may not.
    pub(super) parity_doubt: Option<String>,
}

/// One firmware volume, ready to be described and registered.
pub(super) struct VolumeOutcome {
    pub(super) array: RaidArray,
    pub(super) volume: Option<BuiltVolume>,
    /// Slots the assembled volume does not read.
    pub(super) absent_slots: Vec<usize>,
    /// Origin shown in volume labels, e.g. `imsm/Volume0`.
    pub(super) tag: String,
    /// `assembled://` label before any degraded suffix.
    pub(super) label: String,
}

#[derive(Default)]
pub(super) struct FirmwareOutput {
    pub(super) volumes: Vec<VolumeOutcome>,
    /// Images that carry usable IMSM or DDF metadata, whether or not a volume
    /// reads them.
    pub(super) claimed_images: HashSet<String>,
    pub(super) diagnostics: Vec<String>,
}

/// Probe every whole block image outside `skip` for IMSM and DDF metadata and
/// assemble the containers found.
pub(super) fn assemble_firmware_sets(
    images: &[Arc<DiskImage>],
    skip: &HashSet<String>,
    options: &DetectOptions,
) -> FirmwareOutput {
    let mut out = FirmwareOutput::default();
    let mut imsm_members = Vec::new();
    let mut ddf_members = Vec::new();
    for image in images
        .iter()
        .filter(|image| is_block_evidence(image) && !skip.contains(&image.id))
    {
        let disk = FirmwareDisk::whole(image);
        let ddf = match super::ddf::probe(disk.reader.as_ref()) {
            Ok(DdfProbe::Valid(metadata)) => Some(metadata),
            Ok(DdfProbe::Invalid(reason)) => {
                out.diagnostics.push(format!(
                    "DDF metadata on {} is unusable: {reason}",
                    disk.describe()
                ));
                None
            }
            Ok(DdfProbe::Absent) => None,
            Err(error) => {
                out.diagnostics.push(format!(
                    "DDF probe failed on {}: {error:#}",
                    disk.describe()
                ));
                None
            }
        };
        let imsm = match super::imsm::probe(disk.reader.as_ref()) {
            Ok(ImsmProbe::Valid(mpb)) => Some(mpb),
            Ok(ImsmProbe::Invalid(reason)) => {
                out.diagnostics.push(format!(
                    "IMSM metadata on {} is unusable: {reason}",
                    disk.describe()
                ));
                None
            }
            Ok(ImsmProbe::Absent) => None,
            Err(error) => {
                out.diagnostics.push(format!(
                    "IMSM probe failed on {}: {error:#}",
                    disk.describe()
                ));
                None
            }
        };
        match (ddf, imsm) {
            (Some(metadata), imsm) => {
                if imsm.is_some() {
                    out.diagnostics.push(format!(
                        "{} carries both DDF and IMSM metadata; the DDF metadata is used",
                        disk.describe()
                    ));
                }
                out.claimed_images.insert(image.id.clone());
                ddf_members.push(DdfMember {
                    metadata: *metadata,
                    disk,
                });
            }
            (None, Some(mpb)) => {
                out.claimed_images.insert(image.id.clone());
                imsm_members.push(ImsmMember { mpb, disk });
            }
            (None, None) => {}
        }
    }
    assemble_imsm(imsm_members, options, &mut out);
    assemble_ddf(ddf_members, options, &mut out);
    out
}

/// What the slots of a volume make readable.
pub(super) struct SlotAssembly {
    pub(super) volume: Option<BuiltVolume>,
    /// Slots not read.
    pub(super) absent: Vec<usize>,
    /// Not assembled only for want of the examiner's consent.
    pub(super) degradable: bool,
}

/// Decide whether the volume can be read from `slots` and build its reader.
/// `consent` is the examiner's permission to rebuild slots the metadata
/// still expects.
pub(super) fn assemble_slots(
    name: &str,
    geometry: &VolumeGeometry,
    slots: Vec<SlotSource>,
    consent: bool,
    diagnostics: &mut Vec<String>,
) -> SlotAssembly {
    let level = geometry.level;
    let absent: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| !matches!(slot, SlotSource::Read(_)))
        .map(|(index, _)| index)
        .collect();
    let parity_doubt = geometry
        .parity_doubt
        .as_deref()
        .filter(|_| is_parity(level) && !absent.is_empty());
    if !absent.is_empty() {
        if !level.tolerates(slots.len(), &absent) {
            diagnostics.push(format!(
                "{name}: {} cannot be read without slot(s) {absent:?}",
                level.label()
            ));
            return SlotAssembly {
                volume: None,
                absent,
                degradable: false,
            };
        }
        let waiting = slots
            .iter()
            .any(|slot| matches!(slot, SlotSource::Expected));
        if (waiting || parity_doubt.is_some()) && !consent {
            if let Some(reason) = parity_doubt {
                diagnostics.push(format!(
                    "{name}: {reason}, so parity may not match the data; rebuilding slot(s) {absent:?} needs the examiner's consent"
                ));
            }
            return SlotAssembly {
                volume: None,
                absent,
                degradable: true,
            };
        }
    }
    let readers = slots
        .into_iter()
        .map(|slot| match slot {
            SlotSource::Read(member) => Some(member),
            SlotSource::Recorded | SlotSource::Expected => None,
        })
        .collect();
    let spec = ArraySpec {
        level,
        chunk_bytes: geometry.chunk_bytes,
        device_bytes: match level {
            ArrayLevel::Linear | ArrayLevel::Raid0 => None,
            _ => Some(geometry.component_bytes),
        },
        zone_layout: Raid0ZoneLayout::Original,
    };
    let built = match build_volume(&spec, readers) {
        Ok(built) => built,
        Err(error) => {
            diagnostics.push(format!("{name}: {error:#}"));
            return SlotAssembly {
                volume: None,
                absent,
                degradable: false,
            };
        }
    };
    if !absent.is_empty() {
        diagnostics.push(format!(
            "{name}: assembled without slot(s) {absent:?}; their data comes from the remaining {}",
            if is_parity(level) {
                "members and parity"
            } else {
                "mirrors"
            }
        ));
        if let Some(reason) = parity_doubt {
            diagnostics.push(format!(
                "{name}: {reason}; data rebuilt from parity may be inconsistent"
            ));
        }
    }
    let volume = match fit_to_recorded(name, built, geometry.recorded_bytes, diagnostics) {
        Ok(volume) => Some(volume),
        Err(error) => {
            diagnostics.push(format!("{name}: {error:#}"));
            None
        }
    };
    SlotAssembly {
        volume,
        absent,
        degradable: false,
    }
}

fn is_parity(level: ArrayLevel) -> bool {
    matches!(
        level,
        ArrayLevel::Raid4 | ArrayLevel::Raid5(_) | ArrayLevel::Raid6(_)
    )
}

/// Cut the assembled device to the size the metadata records for the
/// volume; the members may hold whole strips past it.
fn fit_to_recorded(
    name: &str,
    built: BuiltVolume,
    recorded: Option<u64>,
    diagnostics: &mut Vec<String>,
) -> anyhow::Result<BuiltVolume> {
    match recorded {
        Some(recorded) if recorded < built.size_bytes => {
            let whole = StripedMember {
                reader: built.source,
                data_offset_bytes: 0,
                data_size_bytes: recorded,
            };
            let volume = ConcatVolume::new(vec![Some(whole)], 0)?;
            Ok(BuiltVolume {
                source: Arc::new(volume),
                size_bytes: recorded,
                zone_layout_from: None,
            })
        }
        Some(recorded) if recorded > built.size_bytes => {
            diagnostics.push(format!(
                "{name}: the metadata records {recorded} bytes but the members hold {}; only those are readable",
                built.size_bytes
            ));
            Ok(built)
        }
        _ => Ok(built),
    }
}

/// `assembled://` label of a firmware volume: `<scheme>:<name>-<id>`, or
/// `<scheme>:<id>` for an unnamed one. The full identity keeps it injective.
pub(super) fn volume_label(scheme: &str, set_name: &str, set_uuid: &str) -> String {
    if set_name.is_empty() {
        format!("{scheme}:{set_uuid}")
    } else {
        format!("{scheme}:{set_name}-{set_uuid}")
    }
}

/// Whether a member's metadata copy missed more than one update of the
/// freshest copy (md's staleness rule).
pub(super) fn is_stale(own: u64, freshest: u64) -> bool {
    own.saturating_add(1) < freshest
}

#[cfg(test)]
#[path = "firmware_fixture.rs"]
mod test_support;

#[cfg(test)]
#[path = "firmware_imsm_tests.rs"]
mod imsm_tests;

#[cfg(test)]
#[path = "firmware_ddf_tests.rs"]
mod ddf_tests;
