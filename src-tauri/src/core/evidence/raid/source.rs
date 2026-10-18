//! Virtual block-device readers used during RAID assembly.
//!
//! Every reader implements `SectorSource`, so member devices, assembled arrays
//! and logical volumes stack in any order (md over LVM, LVM over md):
//!
//! - [`LinearLvSource`] concatenates byte ranges of one or more block sources
//!   into one address space — an LVM linear LV spanning images or arrays.
//! - [`WholeDiskMember`] exposes a byte range of a `DiskImage` as a member.
//! - [`RaidVolume`] stripes RAID0 / RAID4 / RAID5 / RAID6 chunks and rebuilds
//!   absent columns from the P and Q parity ([`striped`]).
//! - [`MirrorVolume`] serves RAID1 reads from the first present mirror.
//! - [`Raid10Volume`] maps md RAID10 near / far / offset copies.
//! - [`ConcatVolume`] and [`ZonedStripeVolume`] replay md linear arrays and
//!   RAID0 over members of different sizes.
//! - [`build_volume`] picks the reader for an [`ArrayLevel`] and sizes it.
//!
//! Members are slot-indexed `Option<StripedMember>`: `None` marks a member that
//! is absent or excluded (stale, faulty, still rebuilding). Inside the array a
//! read never comes back short: a member that cannot supply the bytes the layout
//! needs fails the read, so a hash or export cannot complete over missing data.
//!
//! Every reader also implements [`SectorSource::read_traced`]: bytes a member
//! backend zero-filled for unreadable sectors are reported at the device's
//! own offsets ([`traced`]). Redundant levels serve such bytes from another
//! copy or from parity when one holds them authentically, the way md answers a
//! member read error; only what no authentic source supplies stays zero and is
//! reported. Plain reads return the same bytes, and member faults reach the
//! read-fault collection scope only through the assembled image.

mod build;
mod gf256;
mod linear;
mod mirror;
mod parity_layout;
mod raid10;
mod rebuild;
mod striped;
mod traced;

pub use build::{build_volume, ArrayLevel, ArraySpec, BuiltVolume};
pub use linear::{ConcatVolume, Raid0ZoneLayout, ZonedStripeVolume};
pub use mirror::MirrorVolume;
pub use parity_layout::{Raid5Layout, Raid6Layout};
pub use raid10::{Raid10FarSets, Raid10Layout, Raid10Volume};
pub use striped::{RaidLevel, RaidVolume};

use std::sync::Arc;

use anyhow::{bail, Context};

use crate::core::evidence::image::{rebase_bad_regions, BadRegion, DiskImage};
use crate::core::evidence::partitions::SectorSource;
use traced::{TracedBytes, TracedRead};

/// Erased reader. Every RAID member reads through one of these so the
/// striping math stays the same regardless of whether the member is an
/// LV, a partition slice, an assembled array, or some future construct.
pub type MemberReader = Arc<dyn SectorSource + Send + Sync>;

/// Virtual block device for one LV that may span multiple PVs on multiple
/// images or assembled arrays. Implements `SectorSource` so the rest of the
/// stack (mdadm probe, RAID striper, synthetic `DiskImage`) talks to it through
/// the same trait used by raw mmap / EWF / VMDK readers.
#[derive(Clone)]
pub struct LinearLvSource {
    pub vg_uuid: String,
    pub vg_name: String,
    pub lv_name: String,
    pub size_bytes: u64,
    /// Ordered list of byte ranges that, concatenated, form the LV's
    /// logical address space. Each segment carries the source it reads from
    /// so dispatch is a single Arc clone away.
    pub segments: Vec<LvByteSegment>,
}

#[derive(Clone)]
pub struct LvByteSegment {
    pub source: MemberReader,
    pub byte_offset: u64,
    pub byte_length: u64,
}

impl LinearLvSource {
    /// Inherent reader; named `read_at` to leave the unqualified `read`
    /// identifier free for the `SectorSource` trait impl below.
    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the synthesized spans, as LV offsets.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if len == 0 || offset >= self.size_bytes {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut remaining = (self.size_bytes - offset).min(len as u64) as usize;
        let mut cursor = offset;
        let mut out = TracedRead::new(offset, remaining);
        let mut seg_start = 0u64;
        for segment in &self.segments {
            let seg_end = seg_start
                .checked_add(segment.byte_length)
                .ok_or_else(|| anyhow::anyhow!("LVM segment boundary overflow"))?;
            if cursor < seg_end {
                let local_off = cursor - seg_start;
                let take = (segment.byte_length - local_off).min(remaining as u64) as usize;
                if take == 0 {
                    break;
                }
                let physical = segment
                    .byte_offset
                    .checked_add(local_off)
                    .ok_or_else(|| anyhow::anyhow!("LVM source offset overflow"))?;
                let (bytes, faults) = segment.source.read_traced(physical, take)?;
                if bytes.len() != take {
                    anyhow::bail!("short LVM allocated segment read");
                }
                out.push(TracedBytes::from_read(bytes, &faults, physical));
                cursor += take as u64;
                remaining -= take;
                if remaining == 0 {
                    break;
                }
            }
            seg_start = seg_end;
            if seg_start >= self.size_bytes {
                break;
            }
        }
        if remaining != 0 {
            anyhow::bail!("incomplete LVM segment map");
        }
        Ok(out.finish())
    }
}

impl SectorSource for LinearLvSource {
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

/// Whole-disk (or whole-partition) byte-range adapter that lets a raw
/// `DiskImage` slice participate in RAID assembly through the same
/// `SectorSource` trait used for LV-backed members.
///
/// Reads go through [`DiskImage::read_range_traced`] on both paths: the
/// member image records its own bad regions, and its faults come back in
/// member offsets instead of entering the collection scope of an operation
/// that reads the assembled device at different offsets.
pub struct WholeDiskMember {
    pub image: Arc<DiskImage>,
    pub byte_offset: u64,
    pub byte_length: u64,
}

impl SectorSource for WholeDiskMember {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced(offset, len).map(|(bytes, _)| bytes)
    }
    fn size(&self) -> u64 {
        self.byte_length
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if offset >= self.byte_length || len == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let take = (self.byte_length - offset).min(len as u64) as usize;
        let physical = self
            .byte_offset
            .checked_add(offset)
            .context("RAID member offset overflow")?;
        let (bytes, faults) = self.image.read_range_traced(physical, take)?;
        let faults = rebase_bad_regions(&faults, physical, bytes.len() as u64, offset);
        Ok((bytes, faults))
    }
}

/// One member of an assembled array. The slot index defines its position in
/// the layout (slot 0 .. slot raid_disks-1). `data_offset_bytes` is where the
/// data area starts on this member (md v1.1/v1.2 reserve a header region before
/// it); `data_size_bytes` is how much of that area the member provides.
#[derive(Clone)]
pub struct StripedMember {
    pub reader: MemberReader,
    pub data_offset_bytes: u64,
    pub data_size_bytes: u64,
}

impl StripedMember {
    /// Read exactly `len` bytes at `member_offset` inside the member's data
    /// area, with the spans the member synthesized. A range past the data area
    /// or a short read from the backing source is an error rather than a
    /// truncated result.
    pub(crate) fn read_exact_traced(
        &self,
        member_offset: u64,
        len: usize,
    ) -> anyhow::Result<TracedBytes> {
        let end = member_offset
            .checked_add(len as u64)
            .context("RAID member offset overflow")?;
        if end > self.data_size_bytes {
            bail!(
                "RAID read at member offset {member_offset} (+{len}) exceeds the member data area of {} bytes",
                self.data_size_bytes
            );
        }
        let physical = self
            .data_offset_bytes
            .checked_add(member_offset)
            .context("RAID member offset overflow")?;
        let (bytes, faults) = self.reader.read_traced(physical, len)?;
        if bytes.len() != len {
            bail!(
                "short read from RAID member at offset {physical}: {} of {len} bytes",
                bytes.len()
            );
        }
        Ok(TracedBytes::from_read(bytes, &faults, physical))
    }
}

/// The present member in `slot`, or an error naming the absent slot.
pub(crate) fn present_member(
    members: &[Option<StripedMember>],
    slot: usize,
) -> anyhow::Result<&StripedMember> {
    match members.get(slot) {
        Some(Some(member)) => Ok(member),
        Some(None) => bail!("RAID member slot {slot} is absent"),
        None => bail!("RAID member slot {slot} is outside the array"),
    }
}

/// Smallest data area among the present members.
pub(crate) fn smallest_present(members: &[Option<StripedMember>]) -> Option<u64> {
    members
        .iter()
        .flatten()
        .map(|member| member.data_size_bytes)
        .min()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) use super::raid10::encode_raid10;
    pub(crate) use super::striped::encode_striped;

    /// Byte-buffer member: content varies per offset, so XOR rebuilds and
    /// misplaced chunks are both visible in assertions.
    pub(crate) struct VecMember(pub Vec<u8>);

    impl SectorSource for VecMember {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = (start + len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }

    pub(crate) fn vec_member(bytes: Vec<u8>) -> StripedMember {
        let size = bytes.len() as u64;
        StripedMember {
            reader: Arc::new(VecMember(bytes)),
            data_offset_bytes: 0,
            data_size_bytes: size,
        }
    }

    /// Member with unreadable `bad` spans (member offsets) that it serves the
    /// way a tolerant backend does: zero-filled, and reported as synthesized
    /// by `read_traced`.
    pub(crate) struct FaultyMember {
        pub(crate) bytes: Vec<u8>,
        pub(crate) bad: Vec<std::ops::Range<u64>>,
    }

    impl FaultyMember {
        fn serve(&self, offset: u64, len: usize) -> (Vec<u8>, Vec<BadRegion>) {
            let start = (offset as usize).min(self.bytes.len());
            let end = (start + len).min(self.bytes.len());
            let mut bytes = self.bytes[start..end].to_vec();
            let mut faults = Vec::new();
            for bad in &self.bad {
                let from = bad.start.max(start as u64);
                let to = bad.end.min(end as u64);
                if to > from {
                    bytes[from as usize - start..to as usize - start].fill(0);
                    faults.push(BadRegion {
                        offset: from,
                        len: to - from,
                    });
                }
            }
            (bytes, faults)
        }
    }

    impl SectorSource for FaultyMember {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            Ok(self.serve(offset, len).0)
        }
        fn size(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_traced(
            &self,
            offset: u64,
            len: usize,
        ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
            Ok(self.serve(offset, len))
        }
    }

    pub(crate) fn faulty_member(bytes: Vec<u8>, bad: &[std::ops::Range<u64>]) -> StripedMember {
        let size = bytes.len() as u64;
        StripedMember {
            reader: Arc::new(FaultyMember {
                bytes,
                bad: bad.to_vec(),
            }),
            data_offset_bytes: 0,
            data_size_bytes: size,
        }
    }

    /// The single bytes of `0..len` that `synthetic` accepts, as coalesced
    /// regions — the expected report of an oracle that decides per byte.
    pub(crate) fn regions_where(len: usize, synthetic: impl Fn(usize) -> bool) -> Vec<BadRegion> {
        let mut regions: Vec<BadRegion> = Vec::new();
        for index in (0..len).filter(|&index| synthetic(index)) {
            match regions.last_mut() {
                Some(last) if last.offset + last.len == index as u64 => last.len += 1,
                _ => regions.push(BadRegion {
                    offset: index as u64,
                    len: 1,
                }),
            }
        }
        regions
    }

    /// Bytes inside `faults` are zero; every other byte matches `logical`.
    pub(crate) fn assert_audited(
        bytes: &[u8],
        faults: &[BadRegion],
        logical: &[u8],
        context: &str,
    ) {
        assert_eq!(bytes.len(), logical.len(), "{context}: length");
        for (index, (&byte, &expected)) in bytes.iter().zip(logical).enumerate() {
            let position = index as u64;
            let synthetic = faults
                .iter()
                .any(|fault| fault.offset <= position && position < fault.offset + fault.len);
            if synthetic {
                assert_eq!(byte, 0, "{context}: synthesized byte {index} is not zero");
            } else {
                assert_eq!(byte, expected, "{context}: byte {index}");
            }
        }
    }

    /// Small deterministic generator for fault placements.
    pub(crate) struct Lcg(pub(crate) u64);

    impl Lcg {
        pub(crate) fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound.max(1)
        }

        /// One to `count` spans of at most `longest` bytes inside `0..len`.
        pub(crate) fn spans(
            &mut self,
            len: u64,
            count: u64,
            longest: u64,
        ) -> Vec<std::ops::Range<u64>> {
            (0..=self.below(count))
                .map(|_| {
                    let start = self.below(len);
                    start..(start + 1 + self.below(longest)).min(len)
                })
                .collect()
        }
    }

    /// Deterministic, position-dependent filler.
    pub(crate) fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|index| {
                (index as u32)
                    .wrapping_mul(2_654_435_761)
                    .rotate_left(u32::from(seed % 31))
                    .to_le_bytes()[1]
                    ^ seed
            })
            .collect()
    }
}

#[cfg(test)]
mod audit_tests;

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn read_exact_rejects_ranges_past_the_data_area_and_short_reads() {
        let member = vec_member(vec![7; 64]);
        assert_eq!(
            member.read_exact_traced(60, 4).expect("in range").bytes,
            vec![7; 4]
        );
        assert!(member.read_exact_traced(62, 4).is_err());

        let truncated = StripedMember {
            data_size_bytes: 128,
            ..vec_member(vec![7; 64])
        };
        assert!(truncated.read_exact_traced(32, 64).is_err());
    }

    #[test]
    fn data_offset_shifts_member_reads() {
        let member = StripedMember {
            data_offset_bytes: 4,
            data_size_bytes: 4,
            ..vec_member((0u8..8).collect())
        };
        assert_eq!(
            member.read_exact_traced(0, 4).expect("read").bytes,
            vec![4, 5, 6, 7]
        );
    }

    #[test]
    fn member_faults_are_relative_to_the_data_area() {
        let member = StripedMember {
            data_offset_bytes: 16,
            data_size_bytes: 32,
            ..faulty_member(pattern(48, 3), &[20..24])
        };
        let piece = member.read_exact_traced(2, 8).expect("read");
        assert_eq!(piece.synthetic, vec![2..6]);
        assert_eq!(piece.bytes[2..6], [0; 4]);
    }

    #[test]
    fn lv_segments_report_faults_at_lv_offsets() {
        let first = pattern(160, 1);
        let second = pattern(64, 2);
        let lv = LinearLvSource {
            vg_uuid: "vg".into(),
            vg_name: "vg".into(),
            lv_name: "lv".into(),
            size_bytes: 48,
            segments: vec![
                LvByteSegment {
                    source: Arc::new(FaultyMember {
                        bytes: first.clone(),
                        bad: vec![110..112],
                    }),
                    byte_offset: 100,
                    byte_length: 32,
                },
                LvByteSegment {
                    source: Arc::new(FaultyMember {
                        bytes: second.clone(),
                        bad: vec![4..6, 60..64],
                    }),
                    byte_offset: 0,
                    byte_length: 16,
                },
            ],
        };
        let logical = [&first[100..132], &second[..16]].concat();
        let (bytes, faults) = lv.read_traced(0, 64).expect("read");
        let expected = vec![
            BadRegion { offset: 10, len: 2 },
            BadRegion { offset: 36, len: 2 },
        ];
        assert_eq!(faults, expected);
        assert_audited(&bytes, &faults, &logical, "LV");
        assert_eq!(lv.read(0, 64).expect("plain read"), bytes);

        let (_, faults) = lv.read_traced(11, 2).expect("partial read");
        assert_eq!(faults, vec![BadRegion { offset: 11, len: 1 }]);
    }

    #[test]
    fn whole_disk_member_reports_faults_in_member_offsets() {
        let bytes = pattern(4096, 5);
        let image = Arc::new(
            DiskImage::from_assembled(
                "disk".into(),
                Arc::new(FaultyMember {
                    bytes: bytes.clone(),
                    bad: vec![1500..1600],
                }),
                Vec::new(),
                512,
            )
            .expect("image"),
        );
        let member = WholeDiskMember {
            image: Arc::clone(&image),
            byte_offset: 1024,
            byte_length: 2048,
        };
        let (read, faults) = member.read_traced(256, 512).expect("read");
        assert_eq!(
            faults,
            vec![BadRegion {
                offset: 476,
                len: 100,
            }]
        );
        let relative = rebase_bad_regions(&faults, 256, 512, 0);
        assert_audited(&read, &relative, &bytes[1280..1792], "member");
        // The member image keeps its own record, in image offsets.
        assert_eq!(
            image.bad_regions(),
            vec![BadRegion {
                offset: 1500,
                len: 100,
            }]
        );
    }
}
