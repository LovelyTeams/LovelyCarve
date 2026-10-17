//! Bounded geometry and member-order search.
use super::*;

/// Reconstruct at most one array from a fixed candidate member set. Returns the
/// winning candidate (and its deep score / probe count) without emitting, so
/// the subset-selection driver can decide whether to accept it. `None` when no
/// `(level, chunk, layout, order, offset, missing)` combination yields a
/// deep-validated filesystem. `samples` are the members' pre-read samples, in
/// `group` order.
pub(super) fn reconstruct_one(
    group: &[Arc<DiskImage>],
    samples: &[&MemberSamples],
    size: u64,
) -> Option<(Candidate, u32, usize)> {
    let n = group.len();
    let orderings = orderings(n);
    let data_offsets = derive_data_offsets(samples);
    // A strong low-occupancy outlier is the likely dropped/stale member; the
    // RAID5 search additionally tries reconstructing it from parity.
    let seed = stale_seed(samples);

    let mut survivors: Vec<Candidate> = Vec::new();
    let parity = n >= 3 && (parity_vote(samples) >= PARITY_VOTE_MIN || seed.is_some());
    tracing::debug!(
        members = n,
        ?data_offsets,
        parity,
        ?seed,
        "RAID geometry search"
    );
    if parity {
        survivors = search(group, size, &orderings, &data_offsets, seed, Raid5Variants);
    }
    if survivors.is_empty() {
        survivors = search(group, size, &orderings, &data_offsets, None, Raid0Variant);
    }
    tracing::debug!(
        survivors = survivors.len(),
        "RAID structural validation complete"
    );
    pick_winner(survivors, seed)
}

pub(super) fn search(
    group: &[Arc<DiskImage>],
    size: u64,
    orderings: &[Vec<usize>],
    data_offsets: &[u64],
    seed: Option<usize>,
    level: impl LevelVariants,
) -> Vec<Candidate> {
    let n = group.len();
    if n < level.min_members() {
        return Vec::new();
    }
    let readers: Vec<Arc<dyn SectorSource + Send + Sync>> = group
        .iter()
        .map(|image| {
            Arc::new(WholeDiskMember {
                image: image.clone(),
                byte_offset: 0,
                byte_length: size,
            }) as Arc<dyn SectorSource + Send + Sync>
        })
        .collect();

    // Degraded reconstruction is only meaningful for parity levels; try both
    // "all present" and "drop the seeded stale disk" when a seed exists.
    let missing_options: Vec<Option<usize>> = match (level.supports_degraded(), seed) {
        (true, Some(s)) if s < n => vec![None, Some(s)],
        _ => vec![None],
    };

    let mut survivors = Vec::new();
    for &chunk_kib in &CHUNK_CANDIDATES_KIB {
        let chunk_bytes = chunk_kib * 1024;
        for &data_offset_bytes in data_offsets {
            if data_offset_bytes >= size {
                continue;
            }
            let per_member = size - data_offset_bytes;
            let capacity = level.capacity(n as u64, per_member);
            if capacity == 0 {
                continue;
            }
            for (level_tag, layout_raw, raid_level) in level.variants() {
                for order in orderings {
                    for &missing_disk in &missing_options {
                        // `order` is a permutation of 0..n, so the stale disk
                        // always has a column position here.
                        let missing_slot =
                            missing_disk.and_then(|d| order.iter().position(|&x| x == d));
                        let members: Vec<StripedMember> = order
                            .iter()
                            .map(|&i| StripedMember {
                                reader: readers[i].clone(),
                                data_offset_bytes,
                                data_size_bytes: per_member,
                            })
                            .collect();
                        let source: Arc<dyn SectorSource + Send + Sync> =
                            Arc::new(RaidVolume::set_degraded(
                                raid_level,
                                512,
                                chunk_bytes,
                                Some(capacity),
                                members,
                                missing_slot,
                            ));
                        if let Some((partitions, _scheme, cheap_score)) =
                            cheap_validate(source.as_ref(), capacity)
                        {
                            survivors.push(Candidate {
                                source,
                                level_tag,
                                layout_raw,
                                chunk_bytes,
                                data_offset_bytes,
                                order: order.clone(),
                                missing_disk,
                                partitions,
                                cheap_score,
                                capacity,
                            });
                        }
                    }
                }
            }
        }
    }
    survivors
}

/// Per-member data-area offsets to try, derived from each member's probes at a
/// small set of offsets plus a bounded aligned scan for an MBR / NTFS / ext4
/// boot signature (read into [`MemberSamples::signature_offsets`]). The
/// array's logical block 0 (an MBR or bare-FS boot sector) sits at the data
/// offset on whichever member holds column 0, so a hit directly reveals the
/// reserve size. `0` is always included as a fallback.
pub(super) fn derive_data_offsets(samples: &[&MemberSamples]) -> Vec<u64> {
    let mut found = std::collections::BTreeSet::new();
    found.insert(0u64);
    for member in samples {
        for &offset in &member.signature_offsets {
            found.insert(offset);
            // Bound the expensive geometry search even for a source full of
            // coincidental or deliberately repeated signatures.
            if found.len() >= MAX_DATA_OFFSETS {
                return found.into_iter().collect();
            }
        }
    }
    found.into_iter().collect()
}

// ---------------------------------------------------------------------------
// disk-order enumeration
// ---------------------------------------------------------------------------

/// All `n!` orderings for small `n`; a bounded set (identity + every single
/// rotation + reverse) past [`FULL_PERMUTATION_MAX_MEMBERS`].
pub(super) fn orderings(n: usize) -> Vec<Vec<usize>> {
    if n <= FULL_PERMUTATION_MAX_MEMBERS {
        let mut acc = Vec::new();
        let mut current: Vec<usize> = (0..n).collect();
        permute(&mut current, 0, &mut acc);
        return acc;
    }
    let base: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    for shift in 0..n {
        out.push((0..n).map(|i| (i + shift) % n).collect());
    }
    out.push(base.iter().rev().copied().collect());
    out.sort();
    out.dedup();
    out
}

pub(super) fn permute(slice: &mut [usize], start: usize, acc: &mut Vec<Vec<usize>>) {
    if start == slice.len() {
        acc.push(slice.to_vec());
        return;
    }
    for i in start..slice.len() {
        slice.swap(start, i);
        permute(slice, start + 1, acc);
        slice.swap(start, i);
    }
}
