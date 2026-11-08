use super::*;

/// In-memory member whose reads can fail or come back short, to pin the
/// sample bookkeeping to what the heuristic's checks see on direct reads.
struct TestSource {
    bytes: Vec<u8>,
    /// Reads intersecting one of these `(offset, len)` spans fail.
    failing: Vec<(u64, u64)>,
    /// A read starting at this offset returns only this many bytes.
    short: Option<(u64, usize)>,
}

impl SectorSource for TestSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let end = offset + len as u64;
        if self
            .failing
            .iter()
            .any(|&(start, span)| offset < start + span && start < end)
        {
            anyhow::bail!("injected read failure at {offset}");
        }
        let len = match self.short {
            Some((at, short_len)) if at == offset => short_len.min(len),
            Some(_) | None => len,
        };
        let start = (offset as usize).min(self.bytes.len());
        let stop = (start + len).min(self.bytes.len());
        Ok(self.bytes[start..stop].to_vec())
    }

    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

fn image(source: TestSource) -> Arc<DiskImage> {
    Arc::new(
        DiskImage::from_assembled("samples".into(), Arc::new(source), Vec::new(), 512)
            .expect("assembled image"),
    )
}

fn plain(bytes: Vec<u8>) -> TestSource {
    TestSource {
        bytes,
        failing: Vec::new(),
        short: None,
    }
}

/// Deterministic xorshift fill for sampled blocks.
fn fill(bytes: &mut [u8], seed: u64) {
    let mut state = seed | 1;
    for chunk in bytes.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let word = state.to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
}

// The pre-sampling implementations, which read the members directly for every
// check. The sampled versions must reproduce them exactly.

fn reference_parity_vote(group: &[Arc<DiskImage>], size: u64) -> f64 {
    const BLOCK: usize = 4096;
    let mut dense = 0u64;
    let mut zero = 0u64;
    for offset in parity_sample_offsets(size) {
        let mut bufs = Vec::with_capacity(group.len());
        for image in group {
            match image.read_range(offset, BLOCK) {
                Ok(b) if b.len() == BLOCK => bufs.push(b),
                _ => {
                    bufs.clear();
                    break;
                }
            }
        }
        if bufs.len() != group.len() {
            continue;
        }
        let nonzero_members = bufs.iter().filter(|b| b.iter().any(|&x| x != 0)).count();
        if nonzero_members < 2 {
            continue;
        }
        dense += 1;
        let mut xor = vec![0u8; BLOCK];
        for b in &bufs {
            for (x, &y) in xor.iter_mut().zip(b.iter()) {
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

fn reference_occupancy(image: &Arc<DiskImage>) -> u64 {
    let mut count = 0u64;
    for offset in parity_sample_offsets(image.len()) {
        if let Ok(b) = image.read_range(offset, 4096) {
            if b.iter().any(|&x| x != 0) {
                count += 1;
            }
        }
    }
    count
}

fn reference_stale_seed(group: &[Arc<DiskImage>]) -> Option<usize> {
    if group.len() < 3 {
        return None;
    }
    let occ: Vec<u64> = group.iter().map(reference_occupancy).collect();
    let (min_idx, &min_occ) = occ.iter().enumerate().min_by_key(|(_, &v)| v)?;
    let mut others: Vec<u64> = occ
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
    (strong_outlier || empty_member).then_some(min_idx)
}

fn reference_data_offsets(group: &[Arc<DiskImage>]) -> Vec<u64> {
    let mut found = std::collections::BTreeSet::new();
    found.insert(0u64);
    for image in group {
        let scan_end = image.len().min(DATA_OFFSET_SCAN_BYTES);
        let offsets = DATA_OFFSET_PROBE_SECTORS
            .iter()
            .map(|sector| sector * 512)
            .chain((DATA_OFFSET_SCAN_STEP..=scan_end).step_by(DATA_OFFSET_SCAN_STEP as usize));
        for off in offsets {
            if off.checked_add(2048).is_none_or(|end| end > image.len()) {
                continue;
            }
            let Ok(buf) = image.read_range(off, 2048) else {
                continue;
            };
            if buf.len() < 0x440 {
                continue;
            }
            let mbr = buf[510] == 0x55 && buf[511] == 0xAA;
            let ntfs = &buf[3..11] == b"NTFS    ";
            let ext4 = buf[0x438] == 0x53 && buf[0x439] == 0xEF;
            if mbr || ntfs || ext4 {
                found.insert(off);
                if found.len() >= MAX_DATA_OFFSETS {
                    return found.into_iter().collect();
                }
            }
        }
    }
    found.into_iter().collect()
}

fn reference_subset_seed(occ: &[u64], subset: &[usize]) -> Option<usize> {
    if subset.len() < 3 {
        return None;
    }
    let occ: Vec<u64> = subset.iter().map(|&d| occ[d]).collect();
    let (min_pos, &min_occ) = occ.iter().enumerate().min_by_key(|(_, &v)| v)?;
    let mut others: Vec<u64> = occ
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != min_pos)
        .map(|(_, &v)| v)
        .collect();
    others.sort_unstable();
    let median = others[others.len() / 2];
    let nonzero_others = others.iter().filter(|&&v| v > 0).count();
    ((median >= 8 && (min_occ as f64) < 0.3 * (median as f64))
        || (min_occ == 0 && nonzero_others >= 2))
        .then_some(min_pos)
}

fn reference_sample_cache(group: &[Arc<DiskImage>], size: u64) -> (Vec<Vec<u64>>, Vec<u64>) {
    let offsets = parity_sample_offsets(size);
    let mut fold = Vec::new();
    let mut occ = Vec::new();
    for image in group {
        let disk: Vec<u64> = offsets
            .iter()
            .map(|&off| match image.read_range(off, 4096) {
                Ok(b) if b.len() == 4096 => fold64(&b),
                _ => 0,
            })
            .collect();
        occ.push(disk.iter().filter(|&&f| f != 0).count() as u64);
        fold.push(disk);
    }
    (fold, occ)
}

/// Five equal-size members: three dense data disks, their XOR parity disk and
/// a nearly empty disk, with boot signatures, a 0xFF block (non-zero data
/// whose XOR fold is zero), failed sample and probe reads and a short read.
fn mixed_group(size: usize) -> Vec<Arc<DiskImage>> {
    const MIB: usize = 1024 * 1024;
    let mut data = vec![vec![0u8; size]; 3];
    for (disk, bytes) in data.iter_mut().enumerate() {
        for (row, offset) in parity_sample_offsets(size as u64).into_iter().enumerate() {
            // Leave every fifth row empty on each disk, staggered per disk.
            if (row + disk) % 5 == 0 {
                continue;
            }
            let offset = offset as usize;
            fill(
                &mut bytes[offset..offset + 4096],
                (row * 7 + disk + 1) as u64,
            );
        }
    }
    let mut parity = vec![0u8; size];
    for bytes in &data {
        for (p, &b) in parity.iter_mut().zip(bytes) {
            *p ^= b;
        }
    }
    let [mut first, mut second, mut third] = data.try_into().expect("three data disks");
    third[5 * MIB..5 * MIB + 4096].fill(0xFF);
    first[3 * MIB / 2 + 510..3 * MIB / 2 + 512].copy_from_slice(&[0x55, 0xAA]);
    second[5 * MIB / 2 + 3..5 * MIB / 2 + 11].copy_from_slice(b"NTFS    ");
    third[512 + 0x438..512 + 0x43a].copy_from_slice(&[0x53, 0xEF]);
    let mut sparse = vec![0u8; size];
    fill(&mut sparse[2 * MIB..2 * MIB + 4096], 99);
    fill(&mut sparse[4 * MIB..4 * MIB + 4096], 98);
    sparse[7 * MIB / 2 + 510..7 * MIB / 2 + 512].copy_from_slice(&[0x55, 0xAA]);
    vec![
        image(TestSource {
            bytes: first,
            failing: Vec::new(),
            short: Some((11 * MIB as u64, 1000)),
        }),
        image(TestSource {
            bytes: second,
            failing: vec![(7 * MIB as u64, 4096), (7 * MIB as u64 / 2, 2048)],
            short: None,
        }),
        image(plain(third)),
        image(plain(parity)),
        image(TestSource {
            bytes: sparse,
            failing: vec![(7 * MIB as u64 / 2, 2048)],
            short: None,
        }),
    ]
}

#[test]
fn sampled_checks_match_direct_reads_for_every_subset() {
    let size = 12 * 1024 * 1024;
    let group = mixed_group(size);
    let samples = read_group_samples(&group, size as u64);
    let cache = SampleCache::from_samples(&samples);
    let (reference_fold, reference_occ) = reference_sample_cache(&group, size as u64);
    assert_eq!(
        (cache.fold.clone(), cache.occ.clone()),
        (reference_fold, reference_occ.clone())
    );
    // The 0xFF block is data whose fold is zero: exact and fold occupancy differ.
    assert_eq!(samples[2].occupancy(), cache.occ[2] + 1);
    let mut fractional_votes = 0;
    let mut seeded = 0;
    for mask in 1u32..(1 << group.len()) {
        let subset: Vec<usize> = (0..group.len()).filter(|&i| mask & (1 << i) != 0).collect();
        for order in [subset.clone(), subset.iter().rev().copied().collect()] {
            let images: Vec<Arc<DiskImage>> = order.iter().map(|&i| group[i].clone()).collect();
            let members: Vec<&MemberSamples> = order.iter().map(|&i| &samples[i]).collect();
            let vote = parity_vote(&members);
            assert_eq!(
                vote,
                reference_parity_vote(&images, size as u64),
                "{order:?}"
            );
            assert_eq!(
                stale_seed(&members),
                reference_stale_seed(&images),
                "{order:?}"
            );
            assert_eq!(
                derive_data_offsets(&members),
                reference_data_offsets(&images),
                "{order:?}"
            );
            assert_eq!(
                cache.subset_seed(&order),
                reference_subset_seed(&reference_occ, &order),
                "{order:?}"
            );
            if vote > 0.0 && vote < 1.0 {
                fractional_votes += 1;
            }
            if stale_seed(&members).is_some() {
                seeded += 1;
            }
        }
    }
    // The fixture exercises partial votes and the stale-member path.
    assert!(fractional_votes > 0);
    assert!(seeded > 0);
    let all: Vec<&MemberSamples> = samples.iter().collect();
    assert_eq!(
        derive_data_offsets(&all),
        vec![0, 512, 3 * 512 * 1024, 5 * 512 * 1024]
    );
}

#[test]
fn sampled_data_offsets_stop_at_the_cap_in_member_order() {
    let size = 24 * 1024 * 1024;
    let step = DATA_OFFSET_SCAN_STEP as usize;
    let mut odd = vec![0u8; size];
    let mut even = vec![0u8; size];
    for multiple in 1..size / step {
        let bytes = if multiple % 2 == 1 {
            &mut odd
        } else {
            &mut even
        };
        let offset = multiple * step;
        bytes[offset + 510..offset + 512].copy_from_slice(&[0x55, 0xAA]);
    }
    let group = vec![image(plain(odd)), image(plain(even))];
    let samples = read_group_samples(&group, size as u64);
    for order in [[0, 1], [1, 0]] {
        let images: Vec<Arc<DiskImage>> = order.iter().map(|&i| group[i].clone()).collect();
        let members: Vec<&MemberSamples> = order.iter().map(|&i| &samples[i]).collect();
        let offsets = derive_data_offsets(&members);
        assert_eq!(offsets.len(), MAX_DATA_OFFSETS);
        assert_eq!(offsets, reference_data_offsets(&images), "{order:?}");
    }
}
