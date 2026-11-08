//! Parity voting, occupancy outliers and subset ranking on member samples.
use super::*;

// ---------------------------------------------------------------------------
// RAID5 parity vote, occupancy, data-offset derivation
// ---------------------------------------------------------------------------

/// Return the fraction of dense sampled stripe rows whose all-member XOR is
/// zero — the RAID5 parity vote. Sampling concentrates on the densely written
/// front of the device plus a spread across the whole disk; rows where fewer
/// than two members carry data are skipped (parity == data there is
/// uninformative), as are rows where any member's sample is missing or short.
/// Returns `1.0` when no dense row is found at all (a wholly empty group is
/// inconclusive, so the RAID5 search still gets to try, gated by filesystem
/// validation). A single stale/dropped member depresses but does not zero the
/// ratio — its post-dropout rows break parity — so callers threshold at
/// [`PARITY_VOTE_MIN`] rather than requiring a perfect `1.0`.
pub(super) fn parity_vote(members: &[&MemberSamples]) -> f64 {
    let rows = members.first().map_or(0, |member| member.blocks.len());
    let mut dense = 0u64;
    let mut zero = 0u64;
    let mut xor = vec![0u8; SAMPLE_BLOCK];
    'rows: for row in 0..rows {
        let mut data: Vec<&[u8]> = Vec::with_capacity(members.len());
        for member in members {
            match member.blocks.get(row) {
                Some(Sample::Zero(len)) if *len == SAMPLE_BLOCK => {}
                Some(Sample::Data(bytes)) if bytes.len() == SAMPLE_BLOCK => data.push(bytes),
                Some(Sample::Zero(_) | Sample::Data(_) | Sample::Failed) | None => continue 'rows,
            }
        }
        if data.len() < 2 {
            continue;
        }
        dense += 1;
        xor.fill(0);
        for bytes in &data {
            for (x, &y) in xor.iter_mut().zip(bytes.iter()) {
                *x ^= y;
            }
        }
        if xor.iter().all(|&x| x == 0) {
            zero += 1;
        }
    }
    if dense == 0 {
        return 1.0;
    }
    zero as f64 / dense as f64
}

/// 4 KiB-aligned offsets sampled for parity/occupancy scans: a dense sweep
/// over the written front (every 1 MiB across the first 512 MiB, where
/// filesystem metadata and small files live on a fresh install) plus 64 evenly
/// spaced rows across the remainder.
pub(super) fn parity_sample_offsets(size: u64) -> Vec<u64> {
    const BLOCK: u64 = SAMPLE_BLOCK as u64;
    let mut offsets = Vec::new();
    let front = size.min(512 * 1024 * 1024);
    let mut off = 0u64;
    while off + BLOCK <= front {
        offsets.push(off);
        off += 1024 * 1024;
    }
    if size > BLOCK {
        for i in 0..64u64 {
            let o = (size / 64) * i;
            offsets.push(o.min(size - BLOCK) & !0xFFF);
        }
    }
    offsets
}

/// Identify a single strong low-occupancy outlier in the group — the likely
/// dropped/stale RAID5 member — to seed degraded reconstruction. Returns its
/// index within `members` when one disk is far emptier than the rest (< 0.3×
/// the median of the others, and the others are themselves non-trivially
/// filled); `None` otherwise (no clear outlier, or the group is uniformly
/// sparse).
pub(super) fn stale_seed(members: &[&MemberSamples]) -> Option<usize> {
    let occupancy: Vec<u64> = members.iter().map(|member| member.occupancy()).collect();
    low_occupancy_outlier(&occupancy)
}

/// Position of the strong low-occupancy outlier among `occupancy` (≥ 3
/// members): a partially-stale dropped disk (< 0.3× the median of the others,
/// which hold at least 8 sampled data blocks), or a fully-empty member among
/// ≥ 2 filled peers (a blank/wiped column).
pub(super) fn low_occupancy_outlier(occupancy: &[u64]) -> Option<usize> {
    if occupancy.len() < 3 {
        return None;
    }
    let (min_idx, &min_occ) = occupancy.iter().enumerate().min_by_key(|(_, &v)| v)?;
    let mut others: Vec<u64> = occupancy
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != min_idx)
        .map(|(_, &v)| v)
        .collect();
    others.sort_unstable();
    let median = others[others.len() / 2];
    let nonzero_others = others.iter().filter(|&&v| v > 0).count();
    let strong_outlier = median >= 8 && (min_occ as f64) < 0.3 * (median as f64);
    let empty_member = min_occ == 0 && nonzero_others >= 2;
    if strong_outlier || empty_member {
        Some(min_idx)
    } else {
        None
    }
}

/// Per-disk XOR-folded sample blocks, so subset ranking scores candidates in
/// memory. The 64-bit fold is linear under XOR, so it preserves the RAID5
/// parity-zero invariant exactly (no false negatives); rare fold collisions
/// only add a little noise to non-RAID5 subsets, which the exact
/// reconstruction then rejects anyway.
pub(super) struct SampleCache {
    /// `fold[disk][i]` = XOR-fold of the 4 KiB block at sample offset `i`
    /// (`0` for an all-zero, failed or short block).
    pub(super) fold: Vec<Vec<u64>>,
    pub(super) occ: Vec<u64>,
}

impl SampleCache {
    pub(super) fn from_samples(members: &[MemberSamples]) -> Self {
        let fold: Vec<Vec<u64>> = members
            .iter()
            .map(|member| {
                member
                    .blocks
                    .iter()
                    .map(|sample| match sample {
                        Sample::Data(bytes) if bytes.len() == SAMPLE_BLOCK => fold64(bytes),
                        Sample::Data(_) | Sample::Zero(_) | Sample::Failed => 0,
                    })
                    .collect()
            })
            .collect();
        let occ = fold
            .iter()
            .map(|disk| disk.iter().filter(|&&f| f != 0).count() as u64)
            .collect();
        Self { fold, occ }
    }

    /// Parity vote over a subset of disk indices, on cached folds. Mirrors the
    /// row-by-row logic of [`parity_vote`].
    pub(super) fn parity_vote(&self, subset: &[usize]) -> f64 {
        let n_off = self.fold.first().map(Vec::len).unwrap_or(0);
        let mut dense = 0u64;
        let mut zero = 0u64;
        for i in 0..n_off {
            let nonzero_members = subset.iter().filter(|&&d| self.fold[d][i] != 0).count();
            if nonzero_members < 2 {
                continue;
            }
            dense += 1;
            let x = subset.iter().fold(0u64, |acc, &d| acc ^ self.fold[d][i]);
            if x == 0 {
                zero += 1;
            }
        }
        if dense == 0 {
            return 1.0;
        }
        zero as f64 / dense as f64
    }

    /// Stale-member detection within a subset (mirrors [`stale_seed`] on cached
    /// occupancy). Returns the position WITHIN `subset`, not the disk index.
    pub(super) fn subset_seed(&self, subset: &[usize]) -> Option<usize> {
        let occupancy: Vec<u64> = subset.iter().map(|&d| self.occ[d]).collect();
        low_occupancy_outlier(&occupancy)
    }
}

/// XOR-fold a block to a single 64-bit word (linear under XOR).
pub(super) fn fold64(b: &[u8]) -> u64 {
    let mut acc = 0u64;
    for chunk in b.as_chunks::<8>().0 {
        acc ^= u64::from_le_bytes(*chunk);
    }
    acc
}
