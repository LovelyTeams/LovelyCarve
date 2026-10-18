use std::sync::Arc;

use super::*;
use crate::core::evidence::image::rebase_bad_regions;
use crate::core::evidence::raid::source::gf256;
use crate::core::evidence::raid::source::test_support::{
    assert_audited, faulty_member, pattern, regions_where, vec_member, Lcg, VecMember,
};

use super::encode_striped as encode;

fn volume(level: RaidLevel, members: &[Vec<u8>], chunk: usize, absent: &[usize]) -> RaidVolume {
    let slots = members
        .iter()
        .enumerate()
        .map(|(slot, bytes)| (!absent.contains(&slot)).then(|| vec_member(bytes.clone())))
        .collect();
    RaidVolume::from_slots(level, 512, chunk as u64, None, slots)
}

const RAID5_LAYOUTS: [Raid5Layout; 6] = [
    Raid5Layout::LeftAsymmetric,
    Raid5Layout::RightAsymmetric,
    Raid5Layout::LeftSymmetric,
    Raid5Layout::RightSymmetric,
    Raid5Layout::Parity0,
    Raid5Layout::ParityN,
];

const RAID6_LAYOUTS: [u32; 14] = [0, 1, 2, 3, 4, 5, 8, 9, 10, 16, 17, 18, 19, 20];

#[test]
fn stripe_step_walks_canonical_raid0_layout() {
    let expected = [(0u64, 0usize, 0u64), (4, 1, 0), (8, 0, 4), (12, 1, 4)];
    for (cursor, column, member_offset) in expected {
        let step = stripe_step(cursor, 4, 2, 16);
        assert_eq!((step.column, step.member_offset), (column, member_offset));
        assert_eq!(step.take_len, 4);
    }
    // Mid-chunk reads clamp to the chunk boundary.
    let step = stripe_step(2, 4, 2, 100);
    assert_eq!((step.column, step.member_offset, step.take_len), (0, 2, 2));
}

/// The left-symmetric mapping md uses by default: data restarts right after
/// the parity column, which walks backwards one column per stripe row.
#[test]
fn parity_step_maps_left_symmetric_chunks() {
    let kind = ParityKind::Raid5(Raid5Layout::LeftSymmetric);
    let expected = [
        (0, 0, 0),
        (1, 1, 0),
        (2, 2, 8),
        (3, 0, 8),
        (4, 1, 16),
        (5, 2, 16),
    ];
    for (chunk, column, member_offset) in expected {
        let step = parity_step(chunk * 8, 8, 3, kind, 8);
        assert_eq!(
            (step.column, step.member_offset),
            (column, member_offset),
            "chunk {chunk}"
        );
    }
}

#[test]
fn raid_level_from_raw_accepts_striped_levels_only() {
    assert_eq!(RaidLevel::from_raw(0, 0), Some(RaidLevel::Raid0));
    assert_eq!(RaidLevel::from_raw(4, 0), Some(RaidLevel::Raid4));
    assert_eq!(
        RaidLevel::from_raw(5, 2),
        Some(RaidLevel::Raid5(Raid5Layout::LeftSymmetric))
    );
    assert_eq!(
        RaidLevel::from_raw(6, 18),
        Some(RaidLevel::Raid6(Raid6Layout::LeftSymmetric6))
    );
    assert_eq!(RaidLevel::from_raw(5, 99), None);
    assert_eq!(RaidLevel::from_raw(6, 7), None);
    assert_eq!(RaidLevel::from_raw(1, 0), None);
    assert_eq!(RaidLevel::from_raw(10, 258), None);
}

#[test]
fn full_arrays_read_back_their_logical_bytes() {
    let chunk = 16;
    let mut levels = vec![RaidLevel::Raid0, RaidLevel::Raid4];
    levels.extend(RAID5_LAYOUTS.iter().map(|&layout| RaidLevel::Raid5(layout)));
    levels.extend(
        RAID6_LAYOUTS
            .iter()
            .filter_map(|&raw| Raid6Layout::from_raw(raw).map(RaidLevel::Raid6)),
    );
    for level in levels {
        for n in [4usize, 5] {
            let data_disks = n - level.parity_disks();
            let logical = pattern(chunk * data_disks * 7, n as u8);
            let members = encode(level, &logical, n, chunk);
            let array = volume(level, &members, chunk, &[]);
            assert_eq!(array.total_bytes(), logical.len() as u64, "{level:?} n={n}");
            assert_eq!(
                array.read_at(0, usize::MAX).expect("read"),
                logical,
                "{level:?} n={n}"
            );
            // Unaligned reads cross chunk and stripe boundaries.
            assert_eq!(
                array.read_at(5, chunk * 3).expect("read"),
                logical[5..5 + chunk * 3],
                "{level:?} n={n}"
            );
        }
    }
}

#[test]
fn single_parity_levels_rebuild_any_one_absent_member() {
    let chunk = 8;
    let mut levels = vec![RaidLevel::Raid4];
    levels.extend(RAID5_LAYOUTS.iter().map(|&layout| RaidLevel::Raid5(layout)));
    for level in levels {
        for n in [3usize, 4] {
            let logical = pattern(chunk * (n - 1) * 9, 3);
            let members = encode(level, &logical, n, chunk);
            for absent in 0..n {
                let array = volume(level, &members, chunk, &[absent]);
                assert_eq!(
                    array.read_at(0, usize::MAX).expect("degraded read"),
                    logical,
                    "{level:?} n={n} absent={absent}"
                );
            }
            let array = volume(level, &members, chunk, &[0, 1]);
            assert!(
                array.read_at(0, usize::MAX).is_err(),
                "{level:?} two absent"
            );
        }
    }
}

#[test]
fn raid6_rebuilds_every_pair_of_absent_members() {
    let chunk = 8;
    for raw in RAID6_LAYOUTS {
        let level = RaidLevel::Raid6(Raid6Layout::from_raw(raw).expect("layout"));
        for n in [4usize, 5, 6] {
            let logical = pattern(chunk * (n - 2) * (n + 3), raw as u8);
            let members = encode(level, &logical, n, chunk);
            for first in 0..n {
                let array = volume(level, &members, chunk, &[first]);
                assert_eq!(
                    array.read_at(0, usize::MAX).expect("single erasure"),
                    logical,
                    "layout {raw} n={n} absent={first}"
                );
                for second in first + 1..n {
                    let array = volume(level, &members, chunk, &[first, second]);
                    assert_eq!(
                        array.read_at(0, usize::MAX).expect("double erasure"),
                        logical,
                        "layout {raw} n={n} absent={first},{second}"
                    );
                }
            }
            let array = volume(level, &members, chunk, &[0, 1, 2]);
            assert!(
                array.read_at(0, usize::MAX).is_err(),
                "layout {raw}: three absent"
            );
        }
    }
}

/// A hand-computed md RAID6 row: left-symmetric row 0 of four columns is
/// "Q D0 D1 P", so Q = D0 ⊕ {02}·D1.
#[test]
fn raid6_q_syndrome_matches_md_definition() {
    let d0 = vec![0x01u8; 4];
    let d1 = vec![0x81u8; 4];
    let p: Vec<u8> = d0.iter().zip(&d1).map(|(a, b)| a ^ b).collect();
    let q: Vec<u8> = d0
        .iter()
        .zip(&d1)
        .map(|(a, b)| a ^ gf256::mul(2, *b))
        .collect();
    assert_eq!(q, vec![0x01 ^ 0x1f; 4]);
    let members = vec![q, d0.clone(), d1.clone(), p];
    let level = RaidLevel::Raid6(Raid6Layout::LeftSymmetric);
    let both_data_absent = volume(level, &members, 4, &[1, 2]);
    assert_eq!(
        both_data_absent.read_at(0, 8).expect("read"),
        [d0.as_slice(), d1.as_slice()].concat()
    );
}

#[test]
fn raid0_absent_member_fails_instead_of_returning_holes() {
    let members = encode(RaidLevel::Raid0, &pattern(64, 1), 2, 8);
    let array = volume(RaidLevel::Raid0, &members, 8, &[1]);
    assert!(array.read_at(0, 8).is_ok());
    assert!(array.read_at(0, 16).is_err());
}

#[test]
fn set_degraded_never_reads_the_dropped_member() {
    let chunk = 8;
    let level = RaidLevel::Raid5(Raid5Layout::LeftSymmetric);
    let logical = pattern(chunk * 2 * 4, 9);
    let mut members = encode(level, &logical, 3, chunk);
    members[1].fill(0xEE); // stale contents must not leak into the result
    let array = RaidVolume::set_degraded(
        level,
        512,
        chunk as u64,
        None,
        members.into_iter().map(vec_member).collect(),
        Some(1),
    );
    assert_eq!(array.missing_slot(), Some(1));
    assert_eq!(array.read_at(0, usize::MAX).expect("read"), logical);
}

#[test]
fn explicit_logical_size_clamps_reads() {
    let members = vec![vec_member(vec![0xAA; 64]), vec_member(vec![0xBB; 64])];
    let array = RaidVolume::set(RaidLevel::Raid0, 512, 8, Some(16), members);
    assert_eq!(array.total_bytes(), 16);
    assert_eq!(array.read_at(0, 1024).expect("read").len(), 16);
}

#[test]
fn capacity_counts_whole_chunks_of_the_smallest_member() {
    let members = vec![
        Some(vec_member(vec![0; 100])),
        Some(vec_member(vec![0; 96])),
        None,
    ];
    let level = RaidLevel::Raid5(Raid5Layout::LeftSymmetric);
    assert_eq!(level.capacity_bytes(&members, 32), 2 * 96);
    assert_eq!(level.capacity_bytes(&members, 40), 2 * 80);
    assert_eq!(RaidLevel::Raid0.capacity_bytes(&members, 32), 3 * 96);
}

/// The bytes a read must report: a data byte is synthesized exactly when its
/// column is erased — absent, or synthesized by its member at that offset — and
/// its row has more erasures there than the level has parity columns.
fn expected_faults(
    level: RaidLevel,
    n: usize,
    chunk: usize,
    len: usize,
    absent: &[usize],
    bad: &[Vec<Range<u64>>],
) -> Vec<BadRegion> {
    regions_where(len, |index| {
        let cursor = index as u64;
        let step = match level.parity_kind() {
            None => stripe_step(cursor, chunk as u64, n as u64, 1),
            Some(kind) => parity_step(cursor, chunk as u64, n as u64, kind, 1),
        };
        let erased: Vec<usize> = (0..n)
            .filter(|&column| {
                absent.contains(&column)
                    || bad[column]
                        .iter()
                        .any(|span| span.contains(&step.member_offset))
            })
            .collect();
        erased.contains(&step.column) && erased.len() > level.parity_disks()
    })
}

#[test]
fn member_faults_are_rebuilt_from_parity_or_reported_at_logical_offsets() {
    let chunk = 16;
    let mut rng = Lcg(3);
    let mut levels = vec![RaidLevel::Raid0, RaidLevel::Raid4];
    levels.extend(RAID5_LAYOUTS.iter().map(|&layout| RaidLevel::Raid5(layout)));
    levels.extend(
        RAID6_LAYOUTS
            .iter()
            .filter_map(|&raw| Raid6Layout::from_raw(raw).map(RaidLevel::Raid6)),
    );
    for level in levels {
        for n in [4usize, 5] {
            let data_disks = n - level.parity_disks();
            let logical = pattern(chunk * data_disks * 6, n as u8);
            let members = encode(level, &logical, n, chunk);
            let member_len = members[0].len() as u64;
            for absent_count in 0..=level.parity_disks() {
                for case in 0..6 {
                    let mut absent = Vec::new();
                    while absent.len() < absent_count {
                        let slot = rng.below(n as u64) as usize;
                        if !absent.contains(&slot) {
                            absent.push(slot);
                        }
                    }
                    // One to three members with bad spans; span ends often fall
                    // mid-chunk, so rebuilds see partly synthesized columns.
                    let faulted = 1 + rng.below(3);
                    let bad: Vec<Vec<Range<u64>>> = (0..n)
                        .map(|column| {
                            if (column as u64 + case) % n as u64 <= faulted - 1 {
                                rng.spans(member_len, 2, chunk as u64 * 2)
                            } else {
                                Vec::new()
                            }
                        })
                        .collect();
                    let slots = members
                        .iter()
                        .zip(&bad)
                        .enumerate()
                        .map(|(column, (bytes, spans))| {
                            (!absent.contains(&column)).then(|| faulty_member(bytes.clone(), spans))
                        })
                        .collect();
                    let array = RaidVolume::from_slots(level, 512, chunk as u64, None, slots);
                    let context = format!("{level:?} n={n} absent={absent:?} bad={bad:?}");

                    let (bytes, faults) = array.read_traced_at(0, usize::MAX).expect("read");
                    let expected = expected_faults(level, n, chunk, logical.len(), &absent, &bad);
                    assert_eq!(faults, expected, "{context}");
                    assert_audited(&bytes, &faults, &logical, &context);
                    assert_eq!(array.read_at(0, usize::MAX).expect("plain read"), bytes);

                    // An unaligned partial read agrees with the full one.
                    let (start, len) = (5, chunk * 3);
                    let (part, part_faults) = array.read_traced_at(start, len).expect("partial");
                    assert_eq!(part, bytes[5..5 + len], "{context}");
                    assert_eq!(
                        part_faults,
                        rebase_bad_regions(&faults, start, len as u64, start),
                        "{context}"
                    );
                }
            }
        }
    }
}

/// Left-symmetric over three members is "D0 D1 P / D3 P D2 / P D4 D5". With
/// member 2 gone, a bad span on member 0 cannot be rebuilt in row 0 (P is
/// gone) and poisons the rebuild of D2 in row 1: both come back as zeros and
/// are reported, instead of data computed from synthesized bytes.
#[test]
fn degraded_rebuild_reports_synthesized_survivor_bytes() {
    let chunk = 8;
    let level = RaidLevel::Raid5(Raid5Layout::LeftSymmetric);
    let logical = pattern(chunk * 2 * 3, 6);
    let members = encode(level, &logical, 3, chunk);
    let array = RaidVolume::from_slots(
        level,
        512,
        chunk as u64,
        None,
        vec![
            Some(faulty_member(members[0].clone(), &[2..4, 10..12])),
            Some(vec_member(members[1].clone())),
            None,
        ],
    );
    let (bytes, faults) = array.read_traced_at(0, usize::MAX).expect("read");
    let region = |offset, len| BadRegion { offset, len };
    // D0 (logical 2..4), D2 rebuilt from D3 (18..20) and D3 itself (26..28).
    assert_eq!(faults, vec![region(2, 2), region(18, 2), region(26, 2)]);
    assert_audited(&bytes, &faults, &logical, "degraded RAID5");
}

#[test]
fn truncated_member_fails_the_read() {
    let chunk = 8;
    let level = RaidLevel::Raid5(Raid5Layout::LeftSymmetric);
    let logical = pattern(chunk * 2 * 4, 5);
    let members = encode(level, &logical, 3, chunk);
    let mut slots: Vec<Option<StripedMember>> = members
        .iter()
        .cloned()
        .map(|bytes| Some(vec_member(bytes)))
        .collect();
    // The metadata promises 32 bytes but the image holds 20.
    slots[2] = Some(StripedMember {
        reader: Arc::new(VecMember(members[2][..20].to_vec())),
        data_offset_bytes: 0,
        data_size_bytes: 32,
    });
    let array = RaidVolume::from_slots(level, 512, chunk as u64, None, slots);
    assert!(array.read_at(0, usize::MAX).is_err());
    assert_eq!(array.read_at(0, 8).expect("first chunk"), logical[..8]);
}
