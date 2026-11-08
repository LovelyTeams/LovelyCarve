//! NTFS source identity and immutable deleted-record projections.
use std::collections::HashMap;

/// Where an NTFS volume lives inside a `DiskImage`. Used as the cache key for
/// deleted-MFT snapshots and as a namespace for IDs emitted by
/// `core::filesystem::volumes::ntfs`. `Partition` covers both raw NTFS partitions and
/// BitLocker-unlocked NTFS (since a BitLocker volume IS a partition); `Lvm`
/// covers NTFS volumes that live inside an LVM2 logical volume on top of a
/// partition (Windows-side-of-dual-boot installs that use Linux LVM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NtfsSource {
    Partition(u8),
    Lvm { part_id: u8, lv_index: usize },
}

/// Single deleted MFT record extracted from a one-shot scan. The evidence
/// session caches this immutable projection without retaining an open NTFS
/// reader. Directory listings convert records into `FsNode`s on demand.
#[derive(Debug, Clone)]
pub struct NtfsDeletedRaw {
    pub record_number: u64,
    pub parent_record_number: u64,
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified: Option<String>,
    pub accessed: Option<String>,
    pub created: Option<String>,
    pub mft_changed: Option<String>,
    /// Absolute image byte runs `(offset, len)` of the deleted file's unnamed
    /// `$DATA` stream, captured at scan time and clamped to the volume. Empty
    /// for directories, resident data, or unrecoverable runlists. Lets a
    /// deleted node be read even after its MFT record is reused (P1-5).
    pub data_runs: Vec<(u64, u64)>,
}

/// Snapshot of every deleted MFT record on a single partition, indexed by
/// parent record number for O(1) directory listings.
#[derive(Debug, Default)]
pub struct NtfsDeletedSnapshot {
    pub entries: Vec<NtfsDeletedRaw>,
    pub by_parent: HashMap<u64, Vec<usize>>,
}

impl NtfsDeletedSnapshot {
    pub fn new(entries: Vec<NtfsDeletedRaw>) -> Self {
        let mut by_parent: HashMap<u64, Vec<usize>> = HashMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            by_parent
                .entry(entry.parent_record_number)
                .or_default()
                .push(idx);
        }
        Self { entries, by_parent }
    }

    pub fn for_parent(&self, parent_record: u64) -> impl Iterator<Item = &NtfsDeletedRaw> {
        self.by_parent
            .get(&parent_record)
            .into_iter()
            .flat_map(|idxs| idxs.iter().map(|i| &self.entries[*i]))
    }
}
