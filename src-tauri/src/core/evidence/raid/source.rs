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

mod build;
mod gf256;
mod linear;
mod mirror;
mod parity_layout;
mod raid10;
mod striped;

pub use build::{build_volume, ArrayLevel, ArraySpec, BuiltVolume};
pub use linear::{ConcatVolume, Raid0ZoneLayout, ZonedStripeVolume};
pub use mirror::MirrorVolume;
pub use parity_layout::{Raid5Layout, Raid6Layout};
pub use raid10::{Raid10FarSets, Raid10Layout, Raid10Volume};
pub use striped::{RaidLevel, RaidVolume};

use std::sync::Arc;

use anyhow::{bail, Context};

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::SectorSource;

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
        if len == 0 || offset >= self.size_bytes {
            return Ok(Vec::new());
        }
        let mut remaining = (self.size_bytes - offset).min(len as u64) as usize;
        let mut cursor = offset;
        let mut out = Vec::with_capacity(remaining);
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
                let bytes = segment.source.read(
                    segment
                        .byte_offset
                        .checked_add(local_off)
                        .ok_or_else(|| anyhow::anyhow!("LVM source offset overflow"))?,
                    take,
                )?;
                if bytes.len() != take {
                    anyhow::bail!("short LVM allocated segment read");
                }
                out.extend_from_slice(&bytes);
                cursor += bytes.len() as u64;
                remaining -= bytes.len();
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
        Ok(out)
    }
}

impl SectorSource for LinearLvSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes
    }
}

/// Whole-disk (or whole-partition) byte-range adapter that lets a raw
/// `DiskImage` slice participate in RAID assembly through the same
/// `SectorSource` trait used for LV-backed members.
pub struct WholeDiskMember {
    pub image: Arc<DiskImage>,
    pub byte_offset: u64,
    pub byte_length: u64,
}

impl SectorSource for WholeDiskMember {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        if offset >= self.byte_length || len == 0 {
            return Ok(Vec::new());
        }
        let take = (self.byte_length - offset).min(len as u64) as usize;
        self.image.read_range(self.byte_offset + offset, take)
    }
    fn size(&self) -> u64 {
        self.byte_length
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
    /// area. A range past the data area or a short read from the backing
    /// source is an error rather than a truncated result.
    pub(crate) fn read_exact(&self, member_offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
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
        let bytes = self.reader.read(physical, len)?;
        if bytes.len() != len {
            bail!(
                "short read from RAID member at offset {physical}: {} of {len} bytes",
                bytes.len()
            );
        }
        Ok(bytes)
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
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn read_exact_rejects_ranges_past_the_data_area_and_short_reads() {
        let member = vec_member(vec![7; 64]);
        assert_eq!(member.read_exact(60, 4).expect("in range"), vec![7; 4]);
        assert!(member.read_exact(62, 4).is_err());

        let truncated = StripedMember {
            data_size_bytes: 128,
            ..vec_member(vec![7; 64])
        };
        assert!(truncated.read_exact(32, 64).is_err());
    }

    #[test]
    fn data_offset_shifts_member_reads() {
        let member = StripedMember {
            data_offset_bytes: 4,
            data_size_bytes: 4,
            ..vec_member((0u8..8).collect())
        };
        assert_eq!(member.read_exact(0, 4).expect("read"), vec![4, 5, 6, 7]);
    }
}
