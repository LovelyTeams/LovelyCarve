//! md linear arrays and RAID0 over members of different sizes.
//!
//! md RAID0 stripes over every member up to the smallest one ("zone 0"), then
//! keeps striping over the members that still have room, one zone per
//! distinct member size (the kernel's `create_strip_zones`). How chunks map to
//! members past zone 0 changed between kernels, so md records it in the
//! `layout` field when the array has such zones.

use anyhow::bail;

use super::traced::TracedRead;
use super::{present_member, StripedMember};
use crate::core::evidence::image::BadRegion;
use crate::core::evidence::partitions::SectorSource;

/// Concatenation of whole member data areas in slot order (md "linear").
pub struct ConcatVolume {
    members: Vec<StripedMember>,
    /// Logical end of each member's contribution.
    ends: Vec<u64>,
}

impl ConcatVolume {
    /// `chunk_bytes`, when non-zero, rounds each member down to whole chunks
    /// the way md sizes linear members.
    pub fn new(members: Vec<Option<StripedMember>>, chunk_bytes: u64) -> anyhow::Result<Self> {
        let mut present = Vec::with_capacity(members.len());
        let mut ends = Vec::with_capacity(members.len());
        let mut end = 0u64;
        for slot in 0..members.len() {
            let mut member = present_member(&members, slot)?.clone();
            if chunk_bytes > 0 {
                member.data_size_bytes -= member.data_size_bytes % chunk_bytes;
            }
            end = end
                .checked_add(member.data_size_bytes)
                .ok_or_else(|| anyhow::anyhow!("linear array size overflow"))?;
            ends.push(end);
            present.push(member);
        }
        if present.is_empty() {
            bail!("linear array has no members");
        }
        Ok(Self {
            members: present,
            ends,
        })
    }

    pub fn size_bytes(&self) -> u64 {
        self.ends.last().copied().unwrap_or(0)
    }

    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the members' synthesized spans, as array offsets.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        let size = self.size_bytes();
        if len == 0 || offset >= size {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut remaining = (size - offset).min(len as u64) as usize;
        let mut cursor = offset;
        let mut out = TracedRead::new(offset, remaining);
        while remaining > 0 {
            let index = self.ends.partition_point(|&end| end <= cursor);
            let (Some(member), Some(&end)) = (self.members.get(index), self.ends.get(index)) else {
                bail!("linear array offset {cursor} has no member");
            };
            let start = end - member.data_size_bytes;
            let take = (end - cursor).min(remaining as u64) as usize;
            out.push(member.read_exact_traced(cursor - start, take)?);
            cursor += take as u64;
            remaining -= take;
        }
        Ok(out.finish())
    }
}

impl SectorSource for ConcatVolume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes()
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_traced_at(offset, len)
    }
}

/// Chunk mapping past zone 0 of a multi-zone md RAID0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raid0ZoneLayout {
    /// md layout 1: the member index comes from the array-wide chunk number
    /// (kernels before 3.14).
    Original,
    /// md layout 2: the member index comes from the zone-relative chunk
    /// number (kernels 3.14–5.3 used this without recording it).
    Alternate,
}

impl Raid0ZoneLayout {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Original),
            2 => Some(Self::Alternate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Zone {
    /// Logical end (exclusive) of the zone.
    end: u64,
    /// Member offset where the zone starts on every member it uses.
    member_start: u64,
    /// Slots striped in this zone, in slot order.
    slots: Vec<usize>,
}

/// md RAID0, including zones past the smallest member.
pub struct ZonedStripeVolume {
    members: Vec<StripedMember>,
    chunk_bytes: u64,
    zones: Vec<Zone>,
    layout: Raid0ZoneLayout,
}

impl ZonedStripeVolume {
    /// Every slot must be present; RAID0 has no redundancy. Member sizes are
    /// rounded down to whole chunks.
    pub fn new(
        members: Vec<Option<StripedMember>>,
        chunk_bytes: u64,
        layout: Raid0ZoneLayout,
    ) -> anyhow::Result<Self> {
        if chunk_bytes == 0 {
            bail!("RAID0 chunk size must be non-zero");
        }
        let mut present = Vec::with_capacity(members.len());
        for slot in 0..members.len() {
            let mut member = present_member(&members, slot)?.clone();
            member.data_size_bytes -= member.data_size_bytes % chunk_bytes;
            present.push(member);
        }
        if present.is_empty() {
            bail!("RAID0 has no members");
        }
        let sizes: Vec<u64> = present
            .iter()
            .map(|member| member.data_size_bytes)
            .collect();
        let zones = build_zones(&sizes)?;
        Ok(Self {
            members: present,
            chunk_bytes,
            zones,
            layout,
        })
    }

    pub fn size_bytes(&self) -> u64 {
        self.zones.last().map_or(0, |zone| zone.end)
    }

    /// True when chunks past zone 0 depend on [`Raid0ZoneLayout`].
    pub fn layout_matters(&self) -> bool {
        self.zones.get(1).is_some_and(|zone| zone.slots.len() > 1)
    }

    /// Logical start of the second zone, where the layout choice starts to matter.
    pub fn first_zone_end(&self) -> u64 {
        self.zones.first().map_or(0, |zone| zone.end)
    }

    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the members' synthesized spans, as array offsets.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        let size = self.size_bytes();
        if len == 0 || offset >= size {
            return Ok((Vec::new(), Vec::new()));
        }
        let chunk = self.chunk_bytes;
        let mut remaining = (size - offset).min(len as u64) as usize;
        let mut cursor = offset;
        let mut out = TracedRead::new(offset, remaining);
        while remaining > 0 {
            let index = self.zones.partition_point(|zone| zone.end <= cursor);
            let Some(zone) = self.zones.get(index) else {
                bail!("RAID0 offset {cursor} is past the last zone");
            };
            let zone_start = index
                .checked_sub(1)
                .and_then(|previous| self.zones.get(previous))
                .map_or(0, |previous| previous.end);
            let in_zone = cursor - zone_start;
            let width = zone.slots.len() as u64;
            let selector = match self.layout {
                Raid0ZoneLayout::Original => cursor,
                Raid0ZoneLayout::Alternate => in_zone,
            };
            let Some(&slot) = zone.slots.get(((selector / chunk) % width) as usize) else {
                bail!("RAID0 zone {index} has no member for offset {cursor}");
            };
            let in_chunk = cursor % chunk;
            let member_offset = zone.member_start + (in_zone / (width * chunk)) * chunk + in_chunk;
            let take = (chunk - in_chunk).min(remaining as u64) as usize;
            out.push(self.members[slot].read_exact_traced(member_offset, take)?);
            cursor += take as u64;
            remaining -= take;
        }
        Ok(out.finish())
    }
}

impl SectorSource for ZonedStripeVolume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes()
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_traced_at(offset, len)
    }
}

/// One zone per distinct member size: zone 0 stripes every member up to the
/// smallest; each later zone stripes the members larger than the previous
/// zone's limit up to the smallest of those.
fn build_zones(sizes: &[u64]) -> anyhow::Result<Vec<Zone>> {
    let mut zones = Vec::new();
    let mut member_start = 0u64;
    let mut end = 0u64;
    loop {
        let slots: Vec<usize> = (0..sizes.len())
            .filter(|&slot| sizes[slot] > member_start)
            .collect();
        let Some(limit) = slots.iter().map(|&slot| sizes[slot]).min() else {
            break;
        };
        let span = (limit - member_start)
            .checked_mul(slots.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("RAID0 zone size overflow"))?;
        end = end
            .checked_add(span)
            .ok_or_else(|| anyhow::anyhow!("RAID0 size overflow"))?;
        zones.push(Zone {
            end,
            member_start,
            slots,
        });
        member_start = limit;
    }
    Ok(zones)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::raid::source::test_support::{
        assert_audited, faulty_member, pattern, vec_member,
    };

    fn region(offset: u64, len: u64) -> BadRegion {
        BadRegion { offset, len }
    }

    #[test]
    fn member_faults_land_at_array_offsets() {
        let first = pattern(40, 1);
        let second = pattern(24, 2);
        let linear = ConcatVolume::new(
            vec![
                Some(vec_member(first.clone())),
                Some(faulty_member(second.clone(), &[4..6])),
            ],
            0,
        )
        .expect("linear");
        let (bytes, faults) = linear.read_traced_at(0, 64).expect("read");
        assert_eq!(faults, vec![region(44, 2)]);
        assert_audited(&bytes, &faults, &[first, second].concat(), "linear");

        // Member 2 offset 2 is zone-0 chunk 2; offsets 40..44 sit in zone 1,
        // which only member 2 reaches (array 96..112).
        let members = [pattern(32, 1), pattern(32, 2), pattern(48, 3)];
        let open = |bad: &[std::ops::Range<u64>]| {
            ZonedStripeVolume::new(
                vec![
                    Some(vec_member(members[0].clone())),
                    Some(vec_member(members[1].clone())),
                    Some(faulty_member(members[2].clone(), bad)),
                ],
                8,
                Raid0ZoneLayout::Original,
            )
            .expect("raid0")
        };
        let clean = open(&[]).read_at(0, usize::MAX).expect("clean read");
        let raid0 = open(&[2..3, 40..44]);
        let (bytes, faults) = raid0.read_traced_at(0, usize::MAX).expect("read");
        assert_eq!(faults, vec![region(18, 1), region(104, 4)]);
        assert_audited(&bytes, &faults, &clean, "zoned RAID0");
        assert_eq!(raid0.read_at(0, usize::MAX).expect("plain read"), bytes);
    }

    #[test]
    fn linear_concatenates_members_in_slot_order() {
        let first = pattern(40, 1);
        let second = pattern(24, 2);
        let volume = ConcatVolume::new(
            vec![
                Some(vec_member(first.clone())),
                Some(vec_member(second.clone())),
            ],
            0,
        )
        .expect("linear");
        assert_eq!(volume.size_bytes(), 64);
        assert_eq!(
            volume.read_at(0, 100).expect("read"),
            [first.clone(), second].concat()
        );
        assert_eq!(volume.read_at(38, 4).expect("boundary")[..2], first[38..]);

        let rounded =
            ConcatVolume::new(vec![Some(vec_member(pattern(40, 1)))], 16).expect("linear");
        assert_eq!(rounded.size_bytes(), 32);
        assert!(ConcatVolume::new(vec![Some(vec_member(vec![0; 8])), None], 0).is_err());
    }

    #[test]
    fn zones_follow_distinct_member_sizes() {
        let zones = build_zones(&[64, 96, 96, 128]).expect("zones");
        assert_eq!(
            zones,
            vec![
                Zone {
                    end: 256,
                    member_start: 0,
                    slots: vec![0, 1, 2, 3]
                },
                Zone {
                    end: 352,
                    member_start: 64,
                    slots: vec![1, 2, 3]
                },
                Zone {
                    end: 384,
                    member_start: 96,
                    slots: vec![3]
                },
            ]
        );
    }

    /// Two 4-chunk members and one 6-chunk member: zone 1 holds only the
    /// larger member, so both layouts agree and the tail is a plain run.
    #[test]
    fn single_member_tail_zone_is_read_in_order() {
        let chunk = 8u64;
        let members = [pattern(32, 1), pattern(32, 2), pattern(48, 3)];
        let volume = ZonedStripeVolume::new(
            members
                .iter()
                .cloned()
                .map(|bytes| Some(vec_member(bytes)))
                .collect(),
            chunk,
            Raid0ZoneLayout::Original,
        )
        .expect("raid0");
        assert!(!volume.layout_matters());
        assert_eq!(volume.size_bytes(), 112);
        let bytes = volume.read_at(0, usize::MAX).expect("read");
        // Zone 0: chunk c on member c % 3 at row c / 3.
        assert_eq!(bytes[0..8], members[0][0..8]);
        assert_eq!(bytes[8..16], members[1][0..8]);
        assert_eq!(bytes[16..24], members[2][0..8]);
        assert_eq!(bytes[24..32], members[0][8..16]);
        // Zone 1: the rest of member 2.
        assert_eq!(bytes[96..112], members[2][32..48]);
    }

    /// Zone 1 with two members: the original layout picks the member from the
    /// array-wide chunk number, the alternate one from the zone-relative one.
    #[test]
    fn multi_member_zone_layouts_differ_past_zone_zero() {
        let chunk = 8u64;
        // Zone 0 spans 6 chunks (3 members × 2), so zone 1 starts at an even
        // array chunk and both selectors agree on its first chunk.
        let members = [pattern(16, 1), pattern(32, 2), pattern(32, 3)];
        let slots = || {
            members
                .iter()
                .cloned()
                .map(|bytes| Some(vec_member(bytes)))
                .collect()
        };
        let original =
            ZonedStripeVolume::new(slots(), chunk, Raid0ZoneLayout::Original).expect("raid0");
        let alternate =
            ZonedStripeVolume::new(slots(), chunk, Raid0ZoneLayout::Alternate).expect("raid0");
        assert!(original.layout_matters());
        assert_eq!(original.first_zone_end(), 48);
        assert_eq!(original.read_at(48, 8).expect("read"), members[1][16..24]);
        assert_eq!(alternate.read_at(48, 8).expect("read"), members[1][16..24]);

        // Zone 0 spans 3 chunks: array chunk 3 is zone chunk 0, so the
        // original layout starts zone 1 on its second member (3 % 2 = 1) and
        // the alternate layout on its first (0 % 2 = 0).
        let members = [pattern(8, 1), pattern(24, 2), pattern(24, 3)];
        let slots = || {
            members
                .iter()
                .cloned()
                .map(|bytes| Some(vec_member(bytes)))
                .collect()
        };
        let original =
            ZonedStripeVolume::new(slots(), chunk, Raid0ZoneLayout::Original).expect("raid0");
        let alternate =
            ZonedStripeVolume::new(slots(), chunk, Raid0ZoneLayout::Alternate).expect("raid0");
        assert_eq!(original.read_at(24, 8).expect("read"), members[2][8..16]);
        assert_eq!(alternate.read_at(24, 8).expect("read"), members[1][8..16]);
        assert_eq!(original.read_at(32, 8).expect("read"), members[1][8..16]);
        assert_eq!(alternate.read_at(32, 8).expect("read"), members[2][8..16]);
    }

    #[test]
    fn raid0_requires_every_member() {
        let result = ZonedStripeVolume::new(
            vec![Some(vec_member(vec![0; 16])), None],
            8,
            Raid0ZoneLayout::Original,
        );
        assert!(result.is_err());
    }
}
