//! md RAID10: `near` copies of each chunk on adjacent devices, repeated `far`
//! times further down every device (`offset` places the far copies in the next
//! chunk row instead of the next device section).
//!
//! The copy placement follows the kernel's `__raid10_find_phys`, including the
//! "far set" variants selected by layout bits 17–18, and the device / array
//! sizes follow its `calc_sectors` and `raid10_size`.

use anyhow::{bail, Context};

use super::traced::{RepairLog, TracedRead};
use super::StripedMember;
use crate::core::evidence::image::BadRegion;
use crate::core::evidence::partitions::SectorSource;

/// How far copies are grouped across devices (layout bits 17–18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raid10FarSets {
    /// Original layout: far copies rotate across every device.
    Whole,
    /// Devices split into `raid_disks / far` sets (the early "improved" layout).
    Split,
    /// Sets of `near × far` devices.
    Paired,
}

/// md RAID10 `layout` field: `near | far << 8 | offset << 16 | sets << 17`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raid10Layout {
    pub near: u32,
    pub far: u32,
    pub offset: bool,
    pub far_sets: Raid10FarSets,
}

impl Raid10Layout {
    pub fn from_raw(raw: u32) -> Option<Self> {
        if raw >> 19 != 0 {
            return None;
        }
        let near = raw & 0xff;
        let far = (raw >> 8) & 0xff;
        if near == 0 || far == 0 {
            return None;
        }
        let far_sets = match raw >> 17 {
            0 => Raid10FarSets::Whole,
            1 => Raid10FarSets::Split,
            2 => Raid10FarSets::Paired,
            _ => return None,
        };
        Some(Self {
            near,
            far,
            offset: raw & (1 << 16) != 0,
            far_sets,
        })
    }

    pub fn raw(&self) -> u32 {
        let sets = match self.far_sets {
            Raid10FarSets::Whole => 0,
            Raid10FarSets::Split => 1,
            Raid10FarSets::Paired => 2,
        };
        self.near | self.far << 8 | u32::from(self.offset) << 16 | sets << 17
    }

    pub fn copies(&self) -> u32 {
        self.near * self.far
    }

    /// Whether every chunk keeps a copy on a device outside `absent`. Which
    /// devices hold a chunk's copies does not depend on chunk or device size.
    pub fn survives(&self, raid_disks: usize, absent: &[usize]) -> bool {
        let disks = raid_disks as u64;
        let Ok((geometry, _)) = Geometry::new(*self, disks, 512, 512 * 4 * disks.max(1)) else {
            return false;
        };
        geometry.covers(|device| !absent.contains(&device))
    }

    /// md's short name: `n2`, `f2`, `o2`, or a combination such as `n2f2`.
    pub fn label(&self) -> String {
        let mut label = String::new();
        if self.near > 1 {
            label.push_str(&format!("n{}", self.near));
        }
        if self.far > 1 {
            label.push(if self.offset { 'o' } else { 'f' });
            label.push_str(&self.far.to_string());
        }
        if label.is_empty() {
            label.push_str("n1");
        }
        label
    }
}

#[derive(Debug, Clone, Copy)]
struct Geometry {
    raid_disks: u64,
    near: u64,
    far: u64,
    offset: bool,
    far_set_size: u64,
    chunk_bytes: u64,
    /// Distance between far copies on one device.
    stride_bytes: u64,
}

impl Geometry {
    fn new(
        layout: Raid10Layout,
        raid_disks: u64,
        chunk_bytes: u64,
        device_bytes: u64,
    ) -> anyhow::Result<(Self, u64)> {
        let near = u64::from(layout.near);
        let far = u64::from(layout.far);
        let copies = near * far;
        if copies < 2 || copies > raid_disks {
            bail!(
                "RAID10 {} keeps {copies} copies of each chunk, which {raid_disks} devices cannot hold",
                layout.label()
            );
        }
        if chunk_bytes == 0 || !chunk_bytes.is_multiple_of(512) {
            bail!("RAID10 chunk size {chunk_bytes} is not a whole number of sectors");
        }
        let far_set_size = match layout.far_sets {
            Raid10FarSets::Whole => raid_disks,
            Raid10FarSets::Split => raid_disks / far,
            Raid10FarSets::Paired => far * near,
        };
        if far_set_size == 0 || far_set_size > raid_disks {
            bail!("RAID10 far-set size {far_set_size} does not fit {raid_disks} devices");
        }
        // calc_sectors: chunks the array uses on each device.
        let chunks_per_device = device_bytes / chunk_bytes;
        let array_chunks = (chunks_per_device / far)
            .checked_mul(raid_disks)
            .context("RAID10 size overflow")?
            / near;
        let used_chunks = array_chunks
            .checked_mul(copies)
            .context("RAID10 size overflow")?
            .div_ceil(raid_disks);
        let stride_chunks = if layout.offset { 1 } else { used_chunks / far };
        // raid10_size over the used device size.
        let size_chunks = (used_chunks / far)
            .checked_mul(raid_disks)
            .context("RAID10 size overflow")?
            / near;
        let geometry = Self {
            raid_disks,
            near,
            far,
            offset: layout.offset,
            far_set_size,
            chunk_bytes,
            stride_bytes: stride_chunks * chunk_bytes,
        };
        Ok((geometry, size_chunks.saturating_mul(chunk_bytes)))
    }

    /// Whether every chunk has a copy on a device `present` accepts. The copy
    /// pattern repeats every `raid_disks` chunks, so those decide it.
    fn covers(&self, present: impl Fn(usize) -> bool) -> bool {
        (0..self.raid_disks).all(|chunk| {
            self.copies(chunk)
                .iter()
                .any(|&(device, _)| present(device))
        })
    }

    /// Every copy of logical chunk `chunk` as `(device, member offset of the
    /// chunk start)`, in the kernel's preference order.
    fn copies(&self, chunk: u64) -> Vec<(usize, u64)> {
        let n = self.raid_disks;
        let last_far_set_start = (n / self.far_set_size - 1) * self.far_set_size;
        let last_far_set_size = self.far_set_size + n % self.far_set_size;

        let scaled = chunk * self.near;
        let mut stripe = scaled / n;
        let mut device = scaled % n;
        if self.offset {
            stripe *= self.far;
        }
        let mut base = stripe * self.chunk_bytes;
        let mut out = Vec::with_capacity((self.near * self.far) as usize);
        for _ in 0..self.near {
            let mut copy_device = device;
            let mut copy_offset = base;
            out.push((copy_device as usize, copy_offset));
            for _ in 1..self.far {
                let set = copy_device / self.far_set_size;
                copy_device += self.near;
                if !n.is_multiple_of(self.far_set_size) && copy_device > last_far_set_start {
                    copy_device -= last_far_set_start;
                    copy_device %= last_far_set_size;
                    copy_device += last_far_set_start;
                } else {
                    copy_device %= self.far_set_size;
                    copy_device += self.far_set_size * set;
                }
                copy_offset += self.stride_bytes;
                out.push((copy_device as usize, copy_offset));
            }
            device += 1;
            if device >= n {
                device = 0;
                base += self.chunk_bytes;
            }
        }
        out
    }
}

/// md RAID10 reader. Each chunk is served by its first readable copy; bytes
/// that copy synthesized are taken from the other copies where they hold them
/// authentically.
pub struct Raid10Volume {
    geometry: Geometry,
    members: Vec<Option<StripedMember>>,
    size_bytes: u64,
    repair_log: RepairLog,
}

impl Raid10Volume {
    /// `device_bytes` is md's per-device `size`, from which the kernel derives
    /// the used device span, the far-copy stride and the array size. A member
    /// shorter than its span only serves the copies it holds; reads fall back
    /// to the other copies.
    pub fn new(
        layout: Raid10Layout,
        chunk_bytes: u64,
        device_bytes: u64,
        members: Vec<Option<StripedMember>>,
    ) -> anyhow::Result<Self> {
        let (geometry, size_bytes) =
            Geometry::new(layout, members.len() as u64, chunk_bytes, device_bytes)?;
        if !geometry.covers(|device| matches!(members.get(device), Some(Some(_)))) {
            bail!("RAID10 {} has chunks with no present copy", layout.label());
        }
        Ok(Self {
            geometry,
            members,
            size_bytes,
            repair_log: RepairLog::default(),
        })
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the spans no copy holds authentically.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if len == 0 || offset >= self.size_bytes {
            return Ok((Vec::new(), Vec::new()));
        }
        let chunk_bytes = self.geometry.chunk_bytes;
        let mut remaining = (self.size_bytes - offset).min(len as u64) as usize;
        let mut cursor = offset;
        let mut out = TracedRead::new(offset, remaining);
        while remaining > 0 {
            let chunk = cursor / chunk_bytes;
            let in_chunk = cursor % chunk_bytes;
            let take = (chunk_bytes - in_chunk).min(remaining as u64) as usize;
            // Present copies in the kernel's preference order, as (device,
            // member, member offset of this piece).
            let copies: Vec<(usize, &StripedMember, u64)> = self
                .geometry
                .copies(chunk)
                .into_iter()
                .filter_map(|(device, chunk_offset)| match self.members.get(device) {
                    Some(Some(member)) => Some((device, member, chunk_offset + in_chunk)),
                    Some(None) | None => None,
                })
                .collect();
            let mut failure = None;
            let mut served = None;
            for (index, &(device, member, member_offset)) in copies.iter().enumerate() {
                match member.read_exact_traced(member_offset, take) {
                    Ok(piece) => {
                        served = Some((index, device, piece));
                        break;
                    }
                    Err(error) => {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                    }
                }
            }
            let (served_index, device, mut piece) = match (served, failure) {
                (Some(served), _) => served,
                (None, Some(error)) => {
                    return Err(error.context(format!("every RAID10 copy of chunk {chunk} failed")))
                }
                (None, None) => bail!("RAID10 chunk {chunk} has no present copy"),
            };
            if !piece.is_authentic() {
                let mut repaired = 0;
                for (index, &(_, member, member_offset)) in copies.iter().enumerate() {
                    if piece.is_authentic() {
                        break;
                    }
                    if index == served_index {
                        continue;
                    }
                    repaired += piece.repair_from(|range| {
                        member.read_exact_traced(member_offset + range.start as u64, range.len())
                    });
                }
                if repaired > 0 {
                    self.repair_log.note("RAID10", device, cursor, repaired);
                }
            }
            out.push(piece);
            cursor += take as u64;
            remaining -= take;
        }
        Ok(out.finish())
    }
}

impl SectorSource for Raid10Volume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_traced_at(offset, len)
    }
}

/// Fill `disks` members of `device` bytes with a patterned logical volume,
/// writing every copy of every chunk where the geometry places it. Returns the
/// logical bytes and the member images.
#[cfg(test)]
pub(crate) fn encode_raid10(
    layout: Raid10Layout,
    disks: usize,
    chunk: usize,
    device: usize,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let Ok((geometry, size)) = Geometry::new(layout, disks as u64, chunk as u64, device as u64)
    else {
        panic!("invalid RAID10 test geometry");
    };
    let logical = super::test_support::pattern(size as usize, 7);
    let mut members = vec![vec![0u8; device]; disks];
    for (index, block) in logical.chunks(chunk).enumerate() {
        for (dev, offset) in geometry.copies(index as u64) {
            let offset = offset as usize;
            members[dev][offset..offset + block.len()].copy_from_slice(block);
        }
    }
    (logical, members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::raid::source::test_support::{
        assert_audited, faulty_member, regions_where, vec_member, Lcg,
    };

    const NEAR2: u32 = 0x102;
    const FAR2: u32 = 0x201;
    const OFFSET2: u32 = 0x10201;

    use super::encode_raid10 as encode;

    fn open(
        layout: Raid10Layout,
        chunk: usize,
        device: usize,
        members: &[Vec<u8>],
        absent: &[usize],
    ) -> anyhow::Result<Raid10Volume> {
        let slots = members
            .iter()
            .enumerate()
            .map(|(slot, bytes)| (!absent.contains(&slot)).then(|| vec_member(bytes.clone())))
            .collect();
        Raid10Volume::new(layout, chunk as u64, device as u64, slots)
    }

    #[test]
    fn layout_field_decodes_md_names() {
        let near = Raid10Layout::from_raw(NEAR2).expect("n2");
        assert_eq!((near.near, near.far, near.offset), (2, 1, false));
        assert_eq!(near.label(), "n2");
        assert_eq!(Raid10Layout::from_raw(FAR2).expect("f2").label(), "f2");
        assert_eq!(Raid10Layout::from_raw(OFFSET2).expect("o2").label(), "o2");
        assert_eq!(
            Raid10Layout::from_raw(NEAR2).map(|layout| layout.raw()),
            Some(NEAR2)
        );
        assert_eq!(Raid10Layout::from_raw(0), None);
        assert_eq!(Raid10Layout::from_raw(1 << 19 | NEAR2), None);
    }

    /// Four devices, near=2: chunk c sits on devices (2c mod 4, 2c+1 mod 4) —
    /// RAID1+0 with mirrored pairs.
    #[test]
    fn near_copies_pair_adjacent_devices() {
        let layout = Raid10Layout::from_raw(NEAR2).expect("n2");
        let (geometry, size) = Geometry::new(layout, 4, 512, 4096).expect("geometry");
        assert_eq!(size, 8192);
        assert_eq!(geometry.copies(0), vec![(0, 0), (1, 0)]);
        assert_eq!(geometry.copies(1), vec![(2, 0), (3, 0)]);
        assert_eq!(geometry.copies(2), vec![(0, 512), (1, 512)]);
    }

    /// Far copies of two devices sit half a device further down on the
    /// neighbour; offset copies sit one chunk row further down.
    #[test]
    fn far_and_offset_copies_follow_md() {
        let far = Raid10Layout::from_raw(FAR2).expect("f2");
        let (geometry, size) = Geometry::new(far, 2, 512, 4096).expect("geometry");
        assert_eq!(size, 4096);
        assert_eq!(geometry.copies(0), vec![(0, 0), (1, 2048)]);
        assert_eq!(geometry.copies(1), vec![(1, 0), (0, 2048)]);

        let offset = Raid10Layout::from_raw(OFFSET2).expect("o2");
        let (geometry, _) = Geometry::new(offset, 2, 512, 4096).expect("geometry");
        assert_eq!(geometry.copies(0), vec![(0, 0), (1, 512)]);
        assert_eq!(geometry.copies(1), vec![(1, 0), (0, 512)]);
        assert_eq!(geometry.copies(2), vec![(0, 1024), (1, 1536)]);
    }

    #[test]
    fn every_layout_reads_back_and_survives_one_absent_device() {
        for raw in [NEAR2, FAR2, OFFSET2, 0x103, 0x202] {
            let layout = Raid10Layout::from_raw(raw).expect("layout");
            for disks in [3usize, 4, 5] {
                if layout.copies() as usize > disks {
                    continue;
                }
                let (logical, members) = encode(layout, disks, 512, 8192);
                let full = open(layout, 512, 8192, &members, &[]).expect("full");
                assert_eq!(
                    full.size_bytes(),
                    logical.len() as u64,
                    "{raw:#x} disks={disks}"
                );
                assert_eq!(full.read_at(0, usize::MAX).expect("read"), logical);
                for absent in 0..disks {
                    let degraded =
                        open(layout, 512, 8192, &members, &[absent]).unwrap_or_else(|error| {
                            panic!("{raw:#x} disks={disks} absent={absent}: {error:#}")
                        });
                    assert_eq!(
                        degraded.read_at(0, usize::MAX).expect("degraded read"),
                        logical,
                        "{raw:#x} disks={disks} absent={absent}"
                    );
                }
            }
        }
    }

    #[test]
    fn losing_both_copies_is_rejected() {
        let layout = Raid10Layout::from_raw(NEAR2).expect("n2");
        let (_, members) = encode(layout, 4, 512, 4096);
        assert!(open(layout, 512, 4096, &members, &[0, 1]).is_err());
        assert!(open(layout, 512, 4096, &members, &[0, 2]).is_ok());
    }

    /// A byte is synthesized only where every copy of its chunk is: other
    /// copies serve the rest, and what is left is reported at its logical
    /// offset.
    #[test]
    fn synthesized_bytes_come_from_other_copies() {
        let mut rng = Lcg(11);
        for raw in [NEAR2, FAR2, OFFSET2, 0x103, 0x202] {
            let layout = Raid10Layout::from_raw(raw).expect("layout");
            for disks in [3usize, 4, 5] {
                if layout.copies() as usize > disks {
                    continue;
                }
                let (logical, members) = encode(layout, disks, 512, 8192);
                let (geometry, _) =
                    Geometry::new(layout, disks as u64, 512, 8192).expect("geometry");
                for case in 0..8 {
                    let bad: Vec<Vec<std::ops::Range<u64>>> =
                        (0..disks).map(|_| rng.spans(8192, 3, 700)).collect();
                    let slots = members
                        .iter()
                        .zip(&bad)
                        .map(|(bytes, spans)| Some(faulty_member(bytes.clone(), spans)))
                        .collect();
                    let volume = Raid10Volume::new(layout, 512, 8192, slots).expect("volume");
                    let (bytes, faults) = volume.read_traced_at(0, usize::MAX).expect("read");

                    let mut synthesized = vec![false; logical.len()];
                    for (chunk, flags) in synthesized.chunks_mut(512).enumerate() {
                        let copies = geometry.copies(chunk as u64);
                        for (in_chunk, flag) in flags.iter_mut().enumerate() {
                            *flag = copies.iter().all(|&(device, offset)| {
                                let position = offset + in_chunk as u64;
                                bad[device].iter().any(|span| span.contains(&position))
                            });
                        }
                    }
                    let expected = regions_where(logical.len(), |index| synthesized[index]);
                    let context = format!("{raw:#x} disks={disks} case={case}");
                    assert_eq!(faults, expected, "{context}: {bad:?}");
                    assert_audited(&bytes, &faults, &logical, &context);
                    assert_eq!(volume.read_at(0, usize::MAX).expect("plain read"), bytes);
                }
            }
        }
    }
}
