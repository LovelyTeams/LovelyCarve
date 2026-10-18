//! Choosing and sizing the reader for an array level.

use std::sync::Arc;

use anyhow::bail;

use super::linear::{ConcatVolume, Raid0ZoneLayout, ZonedStripeVolume};
use super::mirror::MirrorVolume;
use super::parity_layout::{Raid5Layout, Raid6Layout};
use super::raid10::{Raid10Layout, Raid10Volume};
use super::striped::{RaidLevel, RaidVolume};
use super::{smallest_present, StripedMember};
use crate::core::evidence::partitions::SectorSource;

/// Array level and layout, as md records them or as an examiner chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayLevel {
    /// Members concatenated in slot order (md "linear", JBOD).
    Linear,
    Raid0,
    Raid1,
    Raid4,
    Raid5(Raid5Layout),
    Raid6(Raid6Layout),
    Raid10(Raid10Layout),
}

impl ArrayLevel {
    /// Decode md's `level` and `layout` fields. RAID0's `layout` field selects
    /// the zone mapping instead ([`Raid0ZoneLayout`]) and is read separately.
    pub fn from_md(level: i32, layout: u32) -> Option<Self> {
        match level {
            -1 => Some(Self::Linear),
            0 => Some(Self::Raid0),
            1 => Some(Self::Raid1),
            4 => Some(Self::Raid4),
            5 => Raid5Layout::from_raw(layout).map(Self::Raid5),
            6 => Raid6Layout::from_raw(layout).map(Self::Raid6),
            10 => Raid10Layout::from_raw(layout).map(Self::Raid10),
            _ => None,
        }
    }

    /// md's `level` number.
    pub fn md_level(&self) -> i32 {
        match self {
            Self::Linear => -1,
            Self::Raid0 => 0,
            Self::Raid1 => 1,
            Self::Raid4 => 4,
            Self::Raid5(_) => 5,
            Self::Raid6(_) => 6,
            Self::Raid10(_) => 10,
        }
    }

    /// md's `layout` number; 0 for levels without one.
    pub fn md_layout(&self) -> u32 {
        match self {
            Self::Linear | Self::Raid0 | Self::Raid1 | Self::Raid4 => 0,
            Self::Raid5(layout) => layout.raw(),
            Self::Raid6(layout) => layout.raw(),
            Self::Raid10(layout) => layout.raw(),
        }
    }

    /// Readable name for diagnostics, e.g. `RAID6 left-symmetric`.
    pub fn label(&self) -> String {
        match self {
            Self::Linear => "linear".into(),
            Self::Raid0 => "RAID0".into(),
            Self::Raid1 => "RAID1".into(),
            Self::Raid4 => "RAID4".into(),
            Self::Raid5(layout) => format!("RAID5 {}", layout.as_str()),
            Self::Raid6(layout) => format!("RAID6 {}", layout.as_str()),
            Self::Raid10(layout) => format!("RAID10 {}", layout.label()),
        }
    }

    /// Smallest member count the level is defined for.
    pub fn min_members(&self) -> usize {
        match self {
            Self::Linear | Self::Raid0 | Self::Raid1 => 1,
            Self::Raid4 | Self::Raid5(_) => 2,
            Self::Raid6(_) => 4,
            Self::Raid10(layout) => (layout.copies() as usize).max(2),
        }
    }

    /// Whether reads depend on the chunk size (linear only uses it to round
    /// member sizes, RAID1 not at all).
    pub fn uses_chunks(&self) -> bool {
        !matches!(self, Self::Linear | Self::Raid1)
    }

    /// Whether an array of `raid_disks` members stays readable without the
    /// `absent` slots.
    pub fn tolerates(&self, raid_disks: usize, absent: &[usize]) -> bool {
        if absent.is_empty() {
            return true;
        }
        match self {
            Self::Linear | Self::Raid0 => false,
            Self::Raid1 => absent.len() < raid_disks,
            Self::Raid4 | Self::Raid5(_) => absent.len() == 1,
            Self::Raid6(_) => absent.len() <= 2,
            Self::Raid10(layout) => layout.survives(raid_disks, absent),
        }
    }

    fn striped(&self) -> Option<RaidLevel> {
        match self {
            Self::Raid0 => Some(RaidLevel::Raid0),
            Self::Raid4 => Some(RaidLevel::Raid4),
            Self::Raid5(layout) => Some(RaidLevel::Raid5(*layout)),
            Self::Raid6(layout) => Some(RaidLevel::Raid6(*layout)),
            Self::Linear | Self::Raid1 | Self::Raid10(_) => None,
        }
    }
}

/// Geometry needed to read an array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArraySpec {
    pub level: ArrayLevel,
    /// Stripe unit in bytes. RAID1 ignores it; linear arrays round member
    /// sizes to it when non-zero.
    pub chunk_bytes: u64,
    /// Bytes of each member the array uses (md's per-device `size`); `None`
    /// takes the smallest present member's data area. RAID0 and linear arrays
    /// always use each member's own data area.
    pub device_bytes: Option<u64>,
    /// RAID0 mapping past zone 0 when members differ in size.
    pub zone_layout: Raid0ZoneLayout,
}

/// A ready reader plus its logical size.
pub struct BuiltVolume {
    pub source: Arc<dyn SectorSource + Send + Sync>,
    pub size_bytes: u64,
    /// For RAID0 over members of different sizes: the logical offset from
    /// which [`ArraySpec::zone_layout`] decides where chunks live.
    pub zone_layout_from: Option<u64>,
}

/// Build the reader for `spec` over slot-ordered `members` (`None` = absent).
pub fn build_volume(
    spec: &ArraySpec,
    members: Vec<Option<StripedMember>>,
) -> anyhow::Result<BuiltVolume> {
    let level = spec.level;
    let raid_disks = members.len();
    if raid_disks < level.min_members() {
        bail!(
            "{} needs at least {} members, got {raid_disks}",
            level.label(),
            level.min_members()
        );
    }
    let absent: Vec<usize> = (0..raid_disks)
        .filter(|&slot| members[slot].is_none())
        .collect();
    if !level.tolerates(raid_disks, &absent) {
        bail!(
            "{} cannot be read without slot(s) {absent:?}",
            level.label()
        );
    }
    let chunk = spec.chunk_bytes;
    if level.uses_chunks() && (chunk == 0 || !chunk.is_multiple_of(512)) {
        bail!(
            "{} chunk size {chunk} is not a positive whole number of sectors",
            level.label()
        );
    }
    let device_bytes = spec
        .device_bytes
        .or_else(|| smallest_present(&members))
        .unwrap_or(0);

    let built = match level {
        ArrayLevel::Linear => {
            let volume = ConcatVolume::new(members, chunk)?;
            let size_bytes = volume.size_bytes();
            BuiltVolume {
                source: Arc::new(volume),
                size_bytes,
                zone_layout_from: None,
            }
        }
        ArrayLevel::Raid0 => {
            let volume = ZonedStripeVolume::new(members, chunk, spec.zone_layout)?;
            let size_bytes = volume.size_bytes();
            let zone_layout_from = volume.layout_matters().then(|| volume.first_zone_end());
            BuiltVolume {
                source: Arc::new(volume),
                size_bytes,
                zone_layout_from,
            }
        }
        ArrayLevel::Raid1 => {
            let volume = MirrorVolume::new(members, device_bytes)?;
            BuiltVolume {
                source: Arc::new(volume),
                size_bytes: device_bytes,
                zone_layout_from: None,
            }
        }
        ArrayLevel::Raid10(layout) => {
            let volume = Raid10Volume::new(layout, chunk, device_bytes, members)?;
            let size_bytes = volume.size_bytes();
            BuiltVolume {
                source: Arc::new(volume),
                size_bytes,
                zone_layout_from: None,
            }
        }
        ArrayLevel::Raid4 | ArrayLevel::Raid5(_) | ArrayLevel::Raid6(_) => {
            let Some(striped) = level.striped() else {
                bail!("{} is not a striped level", level.label());
            };
            let per_member = device_bytes - device_bytes % chunk;
            if per_member == 0 {
                bail!("{} members hold less than one chunk", level.label());
            }
            if let Some(slot) = (0..raid_disks).find(|&slot| {
                members[slot]
                    .as_ref()
                    .is_some_and(|member| member.data_size_bytes < per_member)
            }) {
                bail!(
                    "{} member in slot {slot} holds less than the {per_member} bytes the array uses",
                    level.label()
                );
            }
            let data_disks = (raid_disks - striped.parity_disks()) as u64;
            let size_bytes = per_member
                .checked_mul(data_disks)
                .ok_or_else(|| anyhow::anyhow!("{} size overflow", level.label()))?;
            let volume = RaidVolume::from_slots(striped, 512, chunk, Some(size_bytes), members);
            BuiltVolume {
                source: Arc::new(volume),
                size_bytes,
                zone_layout_from: None,
            }
        }
    };
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::raid::source::test_support::{pattern, vec_member};

    fn spec(level: ArrayLevel, chunk_bytes: u64) -> ArraySpec {
        ArraySpec {
            level,
            chunk_bytes,
            device_bytes: None,
            zone_layout: Raid0ZoneLayout::Original,
        }
    }

    #[test]
    fn md_level_codes_round_trip() {
        for (level, layout) in [
            (-1, 0),
            (0, 0),
            (1, 0),
            (4, 0),
            (5, 2),
            (6, 18),
            (10, 0x102),
        ] {
            let decoded = ArrayLevel::from_md(level, layout).expect("supported");
            assert_eq!((decoded.md_level(), decoded.md_layout()), (level, layout));
        }
        assert_eq!(ArrayLevel::from_md(7, 0), None);
        assert_eq!(ArrayLevel::from_md(-4, 0), None);
        assert_eq!(ArrayLevel::from_md(5, 8), None);
    }

    #[test]
    fn tolerance_follows_redundancy() {
        let raid5 = ArrayLevel::Raid5(Raid5Layout::LeftSymmetric);
        let raid6 = ArrayLevel::Raid6(Raid6Layout::LeftSymmetric);
        let raid10 = ArrayLevel::from_md(10, 0x102).expect("n2");
        assert!(raid5.tolerates(4, &[2]));
        assert!(!raid5.tolerates(4, &[1, 2]));
        assert!(raid6.tolerates(5, &[0, 4]));
        assert!(!raid6.tolerates(5, &[0, 1, 4]));
        assert!(ArrayLevel::Raid1.tolerates(3, &[0, 2]));
        assert!(!ArrayLevel::Raid1.tolerates(2, &[0, 1]));
        assert!(!ArrayLevel::Raid0.tolerates(2, &[1]));
        assert!(raid10.tolerates(4, &[0, 2]));
        assert!(!raid10.tolerates(4, &[2, 3]));
    }

    #[test]
    fn striped_parity_size_uses_whole_chunks_of_the_device_size() {
        let members = (0..3)
            .map(|seed| Some(vec_member(pattern(100, seed))))
            .collect();
        let built = build_volume(
            &ArraySpec {
                device_bytes: Some(96),
                ..spec(ArrayLevel::Raid5(Raid5Layout::LeftSymmetric), 512)
            },
            members,
        );
        // 96 bytes is less than one 512-byte chunk.
        assert!(built.is_err());

        let members = (0..3)
            .map(|seed| Some(vec_member(pattern(2048, seed))))
            .collect();
        let built = build_volume(
            &ArraySpec {
                device_bytes: Some(1536),
                ..spec(ArrayLevel::Raid5(Raid5Layout::LeftSymmetric), 1024)
            },
            members,
        )
        .expect("raid5");
        assert_eq!(built.size_bytes, 2 * 1024);
    }

    #[test]
    fn mirror_and_concat_sizes() {
        let members = vec![Some(vec_member(pattern(4096, 1))), None];
        let built = build_volume(&spec(ArrayLevel::Raid1, 0), members).expect("raid1");
        assert_eq!(built.size_bytes, 4096);

        let members = vec![
            Some(vec_member(pattern(1000, 1))),
            Some(vec_member(pattern(3000, 2))),
        ];
        let built = build_volume(&spec(ArrayLevel::Linear, 512), members).expect("linear");
        assert_eq!(built.size_bytes, 512 + 2560);
    }

    #[test]
    fn unequal_raid0_reports_where_the_zone_layout_matters() {
        let members = vec![
            Some(vec_member(pattern(1024, 1))),
            Some(vec_member(pattern(2048, 2))),
            Some(vec_member(pattern(2048, 3))),
        ];
        let built = build_volume(&spec(ArrayLevel::Raid0, 512), members).expect("raid0");
        assert_eq!(built.size_bytes, 1024 * 3 + 1024 * 2);
        assert_eq!(built.zone_layout_from, Some(3072));
    }

    #[test]
    fn insufficient_members_are_rejected_up_front() {
        let members = vec![Some(vec_member(pattern(4096, 1))), None, None];
        assert!(build_volume(
            &spec(ArrayLevel::Raid5(Raid5Layout::LeftSymmetric), 512),
            members
        )
        .is_err());
        let members = vec![Some(vec_member(pattern(4096, 1))); 3];
        assert!(build_volume(
            &spec(ArrayLevel::Raid6(Raid6Layout::LeftSymmetric), 512),
            members
        )
        .is_err());
        let members = vec![Some(vec_member(pattern(4096, 1))); 2];
        assert!(build_volume(&spec(ArrayLevel::Raid0, 0), members).is_err());
    }
}
