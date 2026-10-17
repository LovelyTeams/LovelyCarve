//! Member samples read once per equal-size group.
//!
//! Subset ranking, the RAID5 parity vote, the stale-member occupancy count and
//! the data-offset derivation all inspect the same few hundred small blocks of
//! every member. They are read once per group — members in parallel, each
//! member's requests in ascending offset order so neighbouring reads share the
//! image's 64 KiB block cache — and every check and candidate subset is then
//! scored in memory instead of re-reading (and, for compressed evidence,
//! re-decompressing) the members.
use super::*;

use rayon::prelude::*;

/// Size of each parity / occupancy sample.
pub(super) const SAMPLE_BLOCK: usize = 4096;
/// Bytes read at each data-offset probe: enough for the MBR signature, the
/// NTFS OEM id and the ext4 superblock magic at 0x438.
const SIGNATURE_PROBE_BYTES: usize = 2048;

/// Outcome of one sample read, kept exactly as the read returned it.
#[derive(Clone)]
pub(super) enum Sample {
    /// The read failed.
    Failed,
    /// The read returned this many bytes, all zero.
    Zero(usize),
    /// The read returned these bytes, at least one of them non-zero.
    Data(Arc<[u8]>),
}

/// Everything the heuristic reads from one member before the geometry search.
pub(super) struct MemberSamples {
    /// One sample per [`parity_sample_offsets`] entry, duplicates included.
    pub(super) blocks: Vec<Sample>,
    /// Data-offset probes carrying an MBR, NTFS or ext4 boot signature, in the
    /// order [`derive_data_offsets`] consumes them.
    pub(super) signature_offsets: Vec<u64>,
}

impl MemberSamples {
    fn read(image: &DiskImage, size: u64) -> Self {
        let sample_offsets = parity_sample_offsets(size);
        let probe_offsets = data_offset_probes(image.len());
        let mut requests: Vec<(u64, usize)> = sample_offsets
            .iter()
            .map(|&offset| (offset, SAMPLE_BLOCK))
            .chain(
                probe_offsets
                    .iter()
                    .map(|&offset| (offset, SIGNATURE_PROBE_BYTES)),
            )
            .collect();
        requests.sort_unstable();
        requests.dedup();
        let reads: BTreeMap<(u64, usize), Sample> = requests
            .into_iter()
            .map(|(offset, len)| {
                let sample = match image.read_range(offset, len) {
                    Err(_) => Sample::Failed,
                    Ok(bytes) if bytes.iter().all(|&byte| byte == 0) => Sample::Zero(bytes.len()),
                    Ok(bytes) => Sample::Data(bytes.into()),
                };
                ((offset, len), sample)
            })
            .collect();
        let blocks = sample_offsets
            .iter()
            .map(|&offset| {
                reads
                    .get(&(offset, SAMPLE_BLOCK))
                    .cloned()
                    .unwrap_or(Sample::Failed)
            })
            .collect();
        let signature_offsets = probe_offsets
            .into_iter()
            .filter(
                |&offset| match reads.get(&(offset, SIGNATURE_PROBE_BYTES)) {
                    Some(Sample::Data(bytes)) => has_boot_signature(bytes),
                    Some(Sample::Zero(_) | Sample::Failed) | None => false,
                },
            )
            .collect();
        Self {
            blocks,
            signature_offsets,
        }
    }

    /// Sampled blocks holding at least one non-zero byte — a cheap proxy for
    /// how much data the member carries. A dropped/stale RAID5 member stopped
    /// receiving writes and is far emptier than its live peers.
    pub(super) fn occupancy(&self) -> u64 {
        self.blocks
            .iter()
            .filter(|sample| matches!(sample, Sample::Data(_)))
            .count() as u64
    }
}

/// Read the samples of every member of `group` (all `size` bytes long),
/// members in parallel. The result is indexed like `group`.
pub(super) fn read_group_samples(group: &[Arc<DiskImage>], size: u64) -> Vec<MemberSamples> {
    group
        .par_iter()
        .map(|image| MemberSamples::read(image, size))
        .collect()
}

/// Offsets probed for a boot signature when deriving data offsets: the fixed
/// reserve sizes, then a bounded 512 KiB grid, keeping the probes that fit.
fn data_offset_probes(image_len: u64) -> Vec<u64> {
    let scan_end = image_len.min(DATA_OFFSET_SCAN_BYTES);
    DATA_OFFSET_PROBE_SECTORS
        .iter()
        .map(|sector| sector * 512)
        .chain((DATA_OFFSET_SCAN_STEP..=scan_end).step_by(DATA_OFFSET_SCAN_STEP as usize))
        .filter(|offset| {
            offset
                .checked_add(SIGNATURE_PROBE_BYTES as u64)
                .is_some_and(|end| end <= image_len)
        })
        .collect()
}

/// An MBR, NTFS or ext4 boot signature at the start of `bytes`.
fn has_boot_signature(bytes: &[u8]) -> bool {
    if bytes.len() < 0x440 {
        return false;
    }
    let mbr = bytes[510] == 0x55 && bytes[511] == 0xAA;
    let ntfs = &bytes[3..11] == b"NTFS    ";
    let ext4 = bytes[0x438] == 0x53 && bytes[0x439] == 0xEF;
    mbr || ntfs || ext4
}

#[cfg(test)]
#[path = "samples_tests.rs"]
mod samples_tests;
