//! Metadata-less RAID auto-reconstruction.
//!
//! The mdadm/LVM passes in [`super::detect_with_sources`] only recognise a
//! RAID member when it carries Linux md metadata (a v1.x superblock) or an
//! LVM PV label. Hardware-RAID controllers and many NAS/DVR boxes write
//! **no such metadata** — the member images are just the raw striped/parity
//! data. For those, the array parameters (level, chunk size, stripe rotation,
//! disk order, data offset) have to be *recovered* rather than read.
//!
//! This module recovers them by brute force, validated against the actual
//! filesystem on the reconstructed device:
//!
//! 1. **Candidate grouping.** Take every loaded image that carries no RAID
//!    metadata and was not already consumed by an mdadm/LVM array, and group
//!    by exact size — RAID members are equal-sized. Each member's parity rows,
//!    occupancy samples and data-offset probes are read once per group,
//!    members in parallel ([`samples`]); every later check scores them in
//!    memory.
//! 2. **RAID5 gate.** For a group of ≥3, test the RAID5 parity invariant
//!    (XOR of all members at the same physical offset is zero across every
//!    stripe row). It holds for any clean RAID5 regardless of chunk/layout/
//!    order, so it cheaply confirms the level before the expensive search.
//! 3. **Parameter search.** Assemble a [`RaidVolume`](super::source::RaidVolume)
//!    for each `(level, chunk, layout, order, data_offset)` candidate and
//!    keep the ones whose logical device 0 carries a sane partition table /
//!    bare filesystem ([`detect_partitions_with_meta`] + [`refine_partition_kinds`]).
//! 4. **Disambiguation.** A single ext4 superblock validates under many
//!    candidates because a sparse array has parity == data in empty stripe
//!    rows. The tie is broken by descending a bounded ext4 directory tree,
//!    checking distributed backup superblocks and comparing file signatures
//!    beyond the initial metadata stripe on the largest ext4 partition,
//!    using a minimal bounds-checked walker ([`Ext4Probe`]) — the vendored
//!    `ext4-view` parser is for browsing *valid* volumes and panics on the
//!    garbage a wrong candidate produces, so it cannot be used for scoring.
//!
//! The winner is emitted as a normal [`AssembledArray`] carrying the detected
//! partition table, so the IPC layer registers it as a synthetic `DiskImage`
//! and the rest of the app browses it like any other disk.

mod ext4_probe;
use ext4_probe::*;
mod ext4_anchor;
use ext4_anchor::*;
mod ntfs_probe;
use ntfs_probe::*;
mod search;
use search::*;
mod parity;
use parity::*;
mod samples;
use samples::*;
mod validation;
use validation::*;
mod output;
use output::*;

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::core::evidence::image::DiskImage;
use crate::core::evidence::partitions::{
    detect_partitions_with_meta, refine_partition_kinds, Partition, PartitionKind, SectorSource,
    TableScheme,
};

use super::source::{Raid5Layout, RaidLevel, RaidVolume, StripedMember, WholeDiskMember};
use super::xfs_probe::xfs_descend_score;
use super::{AssembledArray, RaidArray, RaidArrayMember, RaidFilesystem, RaidMemberOrigin};
use crate::core::evidence::partitions::lvm2::metadata::lv_regions_from_source;

/// Stripe sizes (KiB) stock `mdadm --create` and common hardware controllers
/// write. Ordered small→large; ties in the search prefer the smaller chunk.
const CHUNK_CANDIDATES_KIB: [u64; 7] = [16, 32, 64, 128, 256, 512, 1024];
/// Per-member reserved-header offsets (512-byte sectors) probed when deriving
/// data offsets. `0` covers hardware RAID that strips its metadata; `2048`
/// (1 MiB) is the near-universal controller/partition alignment reserve (the
/// NTFS RAID5 case material uses it); the rest cover smaller md/controller
/// reserves. [`derive_data_offsets`] reads each member at these offsets and
/// keeps the ones that land on a recognisable MBR / NTFS / ext4 signature,
/// plus `0` as an unconditional fallback. Larger reserves are searched on a
/// bounded 512 KiB grid; only signature hits enter the geometry search.
const DATA_OFFSET_PROBE_SECTORS: [u64; 8] = [0, 1, 8, 16, 64, 128, 256, 2048];
const DATA_OFFSET_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const DATA_OFFSET_SCAN_STEP: u64 = 512 * 1024;
const MAX_DATA_OFFSETS: usize = 32;
/// Full N! disk-order search is only affordable up to this many members; past
/// it the search falls back to a bounded order set (identity + rotations +
/// reverse) and records that it did so. 5 keeps a 5-disk RAID5 (the common
/// NTFS/NAS shape) on the exact-permutation path (5! = 120).
const FULL_PERMUTATION_MAX_MEMBERS: usize = 5;
/// Cap on how many structural survivors get the (more expensive) deep
/// validation, so a pathological group can't blow up the search.
const MAX_DEEP_VALIDATIONS: usize = 4096;
/// Maximum clean ext4 directories descended when scoring a candidate.
const MAX_SUBDIRS_PROBED: usize = 24;
/// Minimum successful directory descents for an ext4-verified accept.
/// Empirically the correct layout descends ~all root subdirs while the next
/// best wrong layout breaks well below this on dense data.
const MIN_DESCEND_SCORE: u32 = 3;
/// Number of leading $MFT records sampled by the NTFS continuity scorer.
const NTFS_MFT_PROBE_RECORDS: u64 = 64;
/// Minimum sequentially-self-consistent $MFT records for an NTFS-verified
/// accept (the system metadata files #0..#15 plus the first real entries).
const MIN_MFT_SCORE: u32 = 8;
/// Fraction of dense stripe rows whose all-member XOR must be zero for a group
/// to be treated as RAID5. Well below 1.0 so a single stale/dropped member
/// (whose post-dropout rows break parity) does not veto the level — the real
/// NTFS RAID5 case scores only ~0.44 over its true member set. A clear
/// occupancy outlier additionally forces the RAID5 search regardless of vote.
const PARITY_VOTE_MIN: f64 = 0.4;

/// Everything the heuristic pass contributes back to the [`super::RaidStack`]
/// and the assembled-source list.
#[derive(Default)]
pub struct HeuristicOutput {
    pub arrays: Vec<RaidArray>,
    pub filesystems: Vec<RaidFilesystem>,
    pub assembled: Vec<AssembledArray>,
    pub diagnostics: Vec<String>,
}

/// Reconstruct metadata-less RAID arrays from `images`. `consumed` is the set
/// of image ids already claimed by an mdadm/LVM array — those are skipped so a
/// disk is never assigned to two arrays.
pub fn detect(images: &[Arc<DiskImage>], consumed: &HashSet<String>) -> HeuristicOutput {
    let mut out = HeuristicOutput::default();

    // Group RAID-metadata-free, unconsumed images by exact byte size.
    let mut by_size: BTreeMap<u64, Vec<Arc<DiskImage>>> = BTreeMap::new();
    for image in images {
        if consumed.contains(&image.id) || carries_raid_metadata(image) {
            continue;
        }
        by_size.entry(image.len()).or_default().push(image.clone());
    }

    for (size, group) in by_size {
        if group.len() < 2 || size == 0 {
            continue;
        }
        attempt_size_group(&group, size, &mut out);
    }
    out
}

/// An image already recognised as an mdadm member or LVM PV is not a
/// metadata-less candidate. (PV labels surface as a `Lvm` partition kind via
/// the normal partition scan; the mdadm/LVM passes also feed their member
/// image ids into `consumed`.)
fn carries_raid_metadata(image: &DiskImage) -> bool {
    image
        .partitions
        .iter()
        .any(|p| matches!(p.kind, PartitionKind::Mdadm | PartitionKind::Lvm))
}

/// One validated reconstruction candidate: the assembled source plus the
/// parameters that produced it and the partition table it yielded.
struct Candidate {
    source: Arc<dyn SectorSource + Send + Sync>,
    level_tag: i32,
    layout_raw: u32,
    chunk_bytes: u64,
    data_offset_bytes: u64,
    order: Vec<usize>,
    /// Group index of the member reconstructed from parity (a dropped/stale
    /// RAID5 disk), or `None` for a fully-present array.
    missing_disk: Option<usize>,
    partitions: Vec<Partition>,
    cheap_score: u32,
    capacity: u64,
}

/// Largest group size for which subset selection enumerates all subsets; past
/// it the search only tries the whole group (a diagnostic records the skip).
const SUBSET_ENUM_MAX_DISKS: usize = 12;
/// How many top-ranked subsets get the (expensive) full geometry search per
/// greedy round.
const MAX_SUBSET_SEARCHES: usize = 24;

/// Reconstruct every array hiding in one equal-size group. When more disks are
/// present than a single array uses (spares, multiple arrays, or unrelated
/// same-size disks), candidate member-subsets are ranked by the RAID5 parity
/// vote — boosted when the subset has a clear stale-member outlier, since a
/// degraded array's vote is low but its dropped disk is obvious — and greedily
/// reconstructed: the best subset that deep-validates is emitted, its members
/// are consumed, and the search repeats on the remainder. Mirrors the manual
/// "pick the member subset, then solve geometry" workflow but driven by
/// filesystem validation rather than a human.
fn attempt_size_group(group: &[Arc<DiskImage>], size: u64, out: &mut HeuristicOutput) {
    let n = group.len();
    // Ranking and every reconstruction attempt below score these samples in
    // memory; each member is read once per group.
    let samples = read_group_samples(group, size);
    if n > SUBSET_ENUM_MAX_DISKS {
        out.diagnostics.push(format!(
            "metadata-less RAID: {n} equal-sized images ({size} bytes) exceed the subset-search \
             limit ({SUBSET_ENUM_MAX_DISKS}); trying the whole group only"
        ));
        let members: Vec<&MemberSamples> = samples.iter().collect();
        if let Some((winner, deep, probed)) = reconstruct_one(group, &members, size) {
            emit(group, winner, deep, probed, out);
        }
        return;
    }

    let cache = SampleCache::from_samples(&samples);
    let mut used = vec![false; n];
    let mut found_any = false;
    loop {
        let avail: Vec<usize> = (0..n).filter(|&i| !used[i]).collect();
        if avail.len() < 2 {
            break;
        }
        let mut accepted: Option<Vec<usize>> = None;
        for subset in ranked_subsets(&cache, &avail)
            .into_iter()
            .take(MAX_SUBSET_SEARCHES)
        {
            let subgroup: Vec<Arc<DiskImage>> = subset.iter().map(|&i| group[i].clone()).collect();
            let subgroup_samples: Vec<&MemberSamples> =
                subset.iter().map(|&i| &samples[i]).collect();
            if let Some((winner, deep, probed)) =
                reconstruct_one(&subgroup, &subgroup_samples, size)
            {
                emit(&subgroup, winner, deep, probed, out);
                accepted = Some(subset);
                found_any = true;
                break;
            }
        }
        match accepted {
            Some(s) => {
                for i in s {
                    used[i] = true;
                }
            }
            None => break,
        }
    }

    if !found_any {
        out.diagnostics.push(format!(
            "metadata-less RAID: {n} equal-sized images ({size} bytes), no RAID metadata; \
             could not reconstruct a valid RAID0/RAID5 layout from any subset"
        ));
    }
}

/// All candidate member-subsets of `avail` (size ≥ 3, plus the whole group when
/// it is only 2 disks for RAID0), ranked best-first by parity vote plus a
/// degraded-array bonus, larger subsets winning ties so a full array beats one
/// of its own subsets.
fn ranked_subsets(cache: &SampleCache, avail: &[usize]) -> Vec<Vec<usize>> {
    let m = avail.len();
    let mut scored: Vec<(f64, Vec<usize>)> = Vec::new();
    for mask in 1u32..(1u32 << m) {
        let k = mask.count_ones() as usize;
        // RAID5 needs ≥3; allow the full 2-disk group through for RAID0.
        if k < 3 && !(k == 2 && k == m) {
            continue;
        }
        let subset: Vec<usize> = (0..m)
            .filter(|&b| mask & (1 << b) != 0)
            .map(|b| avail[b])
            .collect();
        let pv = cache.parity_vote(&subset);
        let bonus = if cache.subset_seed(&subset).is_some() {
            0.5
        } else {
            0.0
        };
        scored.push((pv + bonus, subset));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.len().cmp(&a.1.len()))
    });
    scored.into_iter().map(|(_, s)| s).collect()
}

/// Which levels a search pass enumerates. RAID5 walks all four md layouts;
/// RAID0 has a single (no-rotation) layout.
#[derive(Clone, Copy)]
struct Raid5Variants;
#[derive(Clone, Copy)]
struct Raid0Variant;

trait LevelVariants {
    /// `(level_tag, layout_raw, RaidLevel)` tuples to try.
    fn variants(&self) -> Vec<(i32, u32, RaidLevel)>;
    /// Data capacity for `n` members each contributing `per_member` data bytes.
    fn capacity(&self, n: u64, per_member: u64) -> u64;
    /// Smallest member count this level is defined for.
    fn min_members(&self) -> usize;
    /// Whether a single missing/stale member can be reconstructed from parity
    /// (true for RAID5, false for RAID0).
    fn supports_degraded(&self) -> bool;
}

impl LevelVariants for Raid5Variants {
    fn variants(&self) -> Vec<(i32, u32, RaidLevel)> {
        (0..4)
            .filter_map(|l| Raid5Layout::from_raw(l).map(|layout| (5, l, RaidLevel::Raid5(layout))))
            .collect()
    }
    fn capacity(&self, n: u64, per_member: u64) -> u64 {
        per_member.saturating_mul(n.saturating_sub(1))
    }
    fn min_members(&self) -> usize {
        3
    }
    fn supports_degraded(&self) -> bool {
        true
    }
}

impl LevelVariants for Raid0Variant {
    fn variants(&self) -> Vec<(i32, u32, RaidLevel)> {
        vec![(0, 0, RaidLevel::Raid0)]
    }
    fn capacity(&self, n: u64, per_member: u64) -> u64 {
        per_member.saturating_mul(n)
    }
    fn min_members(&self) -> usize {
        2
    }
    fn supports_degraded(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// ext4 deep validation (panic-free, bounds-checked walker)
// ---------------------------------------------------------------------------
//

struct Ext4SbInfo {
    uuid_hex: String,
    total_bytes: u64,
    block_size: u32,
}

/// Filesystem identity/size pulled from an assembled candidate's superblock,
/// normalised across filesystems for the [`RaidFilesystem`] record and a
/// stable `set_uuid`.
struct FsSummary {
    kind: &'static str,
    fsid: Option<String>,
    total_bytes: Option<u64>,
    sector_size: Option<u32>,
}

struct XfsSbInfo {
    uuid_hex: String,
    total_bytes: u64,
    block_size: u32,
}

struct NtfsSbInfo {
    serial_hex: String,
    total_bytes: u64,
    bytes_per_sector: u32,
}

#[cfg(test)]
mod tests;
