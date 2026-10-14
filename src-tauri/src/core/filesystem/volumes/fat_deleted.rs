//! Raw FAT12/16/32 deleted-entry recovery — the forensic depth the
//! vendored `fatfs` crate does not provide (it silently drops `0xE5` slots and
//! exposes neither the BPB geometry nor raw directory clusters).
//!
//! Everything here works on raw bytes read through [`DiskImage::read_range`]:
//!
//!   * [`FatGeom::parse`] decodes the BPB (cluster size, FAT layout, data
//!     region) for cluster-to-byte address conversion.
//!   * [`scan_dir_deleted`] walks a directory's raw bytes slot-by-slot,
//!     reassembling `0x0F` long-file-name runs (TSK `fatxxfs_dent.c`) and
//!     surfacing `0xE5`-marked short entries as recovered children.
//!
//! Deleted FAT files have a zeroed FAT chain, so data runs are reconstructed
//! best-effort as a single contiguous run from the entry's first cluster for
//! `ceil(size / cluster_size)` clusters, clamped to the volume — the same
//! fallback `fatfs_meta.c` uses when the chain cannot be followed.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use crate::core::evidence::image::DiskImage;
use crate::core::filesystem::tree::fs_tree::{FsDataRun, Mactb};

const DENTRY_LEN: usize = 32;
const SLOT_DELETED: u8 = 0xE5;
const ATTR_LFN: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const LFN_SEQ_MASK: u8 = 0x3F;
/// Bound directory-cluster-chain following so a corrupt/looping FAT can't hang
/// a recovery scan.
const MAX_DIR_CLUSTERS: usize = 1 << 20;
/// Standard LBA of the FAT32 backup boot sector (BPB offset 50-51 defaults to
/// 6). Used as the recovery fallback when the primary boot sector is wiped.
const FAT32_BACKUP_BOOT_SECTOR: u64 = 6;
const BOOT_SECTOR_LEN: usize = 512;

/// One recovered FAT directory entry from a deleted slot.
#[derive(Debug, Clone)]
pub struct RecoveredEntry {
    /// Absolute byte offset (in the image) of this directory slot, preserving
    /// distinct records and matching live entries' `inode` scale.
    pub slot_offset: u64,
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub first_cluster: u32,
    pub timestamps: Mactb,
    /// Absolute image byte runs reconstructed best-effort.
    pub runs: Vec<FsDataRun>,
}

/// Parsed FAT BIOS Parameter Block geometry, anchored at a partition slice.
#[derive(Debug, Clone, Copy)]
pub struct FatGeom {
    pub base_offset: u64,
    pub bytes_per_sector: u32,
    pub sectors_per_cluster: u32,
    pub reserved_sectors: u32,
    pub num_fats: u32,
    pub sectors_per_fat: u32,
    pub root_entries: u32,
    pub total_sectors: u32,
    /// FAT32 root directory first cluster; 0 for FAT12/16 (fixed root region).
    pub root_cluster: u32,
    pub is_fat32: bool,
    /// Selected FAT32 table when mirroring is disabled, otherwise table zero.
    pub active_fat: u32,
}

impl FatGeom {
    /// Parse the geometry from a boot sector. `base_offset` is the partition's
    /// absolute byte offset in the image.
    pub fn parse(boot: &[u8], base_offset: u64) -> Result<Self> {
        if boot.len() < 512 {
            anyhow::bail!("FAT boot sector too short");
        }
        let bytes_per_sector = u16::from_le_bytes([boot[11], boot[12]]) as u32;
        let sectors_per_cluster = boot[13] as u32;
        if !matches!(
            bytes_per_sector,
            512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768
        ) || sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
        {
            anyhow::bail!("FAT BPB geometry invalid");
        }
        let reserved_sectors = u16::from_le_bytes([boot[14], boot[15]]) as u32;
        let num_fats = boot[16] as u32;
        let root_entries = u16::from_le_bytes([boot[17], boot[18]]) as u32;
        let total_16 = u16::from_le_bytes([boot[19], boot[20]]) as u32;
        let total_32 = u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]]);
        let sectors_per_fat_16 = u16::from_le_bytes([boot[22], boot[23]]) as u32;
        let sectors_per_fat_32 = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]);
        let total_sectors = if total_16 != 0 { total_16 } else { total_32 };
        let is_fat32 = sectors_per_fat_16 == 0 && root_entries == 0;
        let sectors_per_fat = if sectors_per_fat_16 != 0 {
            sectors_per_fat_16
        } else {
            sectors_per_fat_32
        };
        let root_cluster = if is_fat32 {
            u32::from_le_bytes([boot[44], boot[45], boot[46], boot[47]]) & 0x0fff_ffff
        } else {
            0
        };
        if num_fats == 0 || sectors_per_fat == 0 || total_sectors == 0 {
            anyhow::bail!("FAT BPB has zero FATs / sectors");
        }
        let first_data = u64::from(reserved_sectors)
            + u64::from(num_fats) * u64::from(sectors_per_fat)
            + (u64::from(root_entries) * 32).div_ceil(u64::from(bytes_per_sector));
        let active_fat = if is_fat32 && boot[40] & 0x80 != 0 {
            u32::from(boot[40] & 0x0f)
        } else {
            0
        };
        if reserved_sectors == 0 || first_data >= u64::from(total_sectors) || active_fat >= num_fats
        {
            anyhow::bail!("FAT invalid data region / active table");
        }
        base_offset
            .checked_add(u64::from(total_sectors) * u64::from(bytes_per_sector))
            .ok_or_else(|| anyhow!("FAT volume end overflow"))?;
        Ok(Self {
            base_offset,
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            sectors_per_fat,
            root_entries,
            total_sectors,
            root_cluster,
            is_fat32,
            active_fat: if is_fat32 && boot[40] & 0x80 != 0 {
                u32::from(boot[40] & 0x0f)
            } else {
                0
            },
        })
    }

    fn cluster_size(&self) -> u64 {
        self.bytes_per_sector as u64 * self.sectors_per_cluster as u64
    }

    /// First sector of the fixed root directory region (FAT12/16). For FAT32
    /// this equals `first_data_sector` (the root is a normal cluster chain).
    fn root_dir_sectors(&self) -> u32 {
        (self.root_entries * 32).div_ceil(self.bytes_per_sector)
    }

    fn first_data_sector(&self) -> u32 {
        self.reserved_sectors + (self.num_fats * self.sectors_per_fat) + self.root_dir_sectors()
    }

    /// Absolute byte offset (in the image) of a data cluster (cluster >= 2).
    pub fn cluster_offset(&self, cluster: u32) -> u64 {
        let sector = self.first_data_sector() as u64
            + (cluster as u64 - 2) * self.sectors_per_cluster as u64;
        self.base_offset + sector * self.bytes_per_sector as u64
    }

    /// Total number of data clusters in the volume.
    pub fn cluster_count(&self) -> u32 {
        let data_sectors = self.total_sectors.saturating_sub(self.first_data_sector());
        data_sectors / self.sectors_per_cluster.max(1)
    }
}

/// Read a directory's raw bytes plus the absolute image offset of each 32-byte
/// slot. For FAT12/16 root (`first_cluster == 0` and not FAT32) reads the fixed
/// root region; otherwise follows the cluster chain from `first_cluster`
/// through the FAT.
pub fn read_dir_bytes(
    image: &Arc<DiskImage>,
    geom: &FatGeom,
    fat: &FatTable,
    first_cluster: u32,
) -> Result<(Vec<u8>, Vec<u64>)> {
    if first_cluster < 2 {
        if geom.is_fat32 {
            // FAT32 root lives at root_cluster, never the fixed region.
            return read_cluster_chain(image, geom, fat, geom.root_cluster);
        }
        // FAT12/16 fixed root region.
        let root_start_sector = geom.reserved_sectors + geom.num_fats * geom.sectors_per_fat;
        let offset = geom.base_offset + root_start_sector as u64 * geom.bytes_per_sector as u64;
        let len = geom.root_entries as usize * DENTRY_LEN;
        let bytes = image.read_range(offset, len)?;
        let slot_offsets = (0..bytes.len() / DENTRY_LEN)
            .map(|i| offset + (i * DENTRY_LEN) as u64)
            .collect();
        return Ok((bytes, slot_offsets));
    }
    read_cluster_chain(image, geom, fat, first_cluster)
}

fn read_cluster_chain(
    image: &Arc<DiskImage>,
    geom: &FatGeom,
    fat: &FatTable,
    first_cluster: u32,
) -> Result<(Vec<u8>, Vec<u64>)> {
    let mut out = Vec::new();
    let mut slot_offsets = Vec::new();
    let cluster_size = geom.cluster_size() as usize;
    let slots_per_cluster = cluster_size / DENTRY_LEN;
    let max_cluster = geom.cluster_count() + 2;
    let mut cluster = first_cluster;
    let mut guard = 0usize;
    let mut visited = std::collections::HashSet::new();
    while cluster >= 2 && cluster < max_cluster && guard < MAX_DIR_CLUSTERS {
        if !visited.insert(cluster) {
            anyhow::bail!("FAT cyclic directory chain");
        }
        let bytes = image.read_range(geom.cluster_offset(cluster), cluster_size)?;
        if bytes.len() != cluster_size {
            anyhow::bail!("FAT directory short read");
        }
        let cluster_start = geom.cluster_offset(cluster);
        for i in 0..slots_per_cluster {
            slot_offsets.push(cluster_start + (i * DENTRY_LEN) as u64);
        }
        out.extend_from_slice(&bytes);
        // The end-of-chain marker lives on the LAST cluster, so it must be read
        // *before* we stop — testing it at the top of the loop would drop the
        // final cluster, and read nothing at all for a single-cluster directory
        // such as the FAT32 root (whose own FAT entry is already EOC). That was
        // why root-directory `0xE5` deletions never surfaced.
        if fat.is_end_of_chain(cluster) {
            break;
        }
        let next = fat.next_cluster(cluster);
        if next < 2 || next >= max_cluster {
            anyhow::bail!("FAT invalid directory chain");
        }
        cluster = next;
        guard += 1;
    }
    if guard >= MAX_DIR_CLUSTERS {
        anyhow::bail!("FAT directory exceeds recovery limit");
    }
    Ok((out, slot_offsets))
}

/// Scan a directory's raw bytes for `0xE5`-deleted entries, reassembling any
/// preceding LFN run. Live entries are ignored (the vendored crate already
/// lists them).
pub fn scan_dir_deleted(
    dir_bytes: &[u8],
    slot_offsets: &[u64],
    geom: &FatGeom,
) -> Vec<RecoveredEntry> {
    let mut out = Vec::new();
    let mut lfn_parts: Vec<(u8, [u16; 13])> = Vec::new();
    let slots = dir_bytes.len() / DENTRY_LEN;
    for i in 0..slots {
        let slot = &dir_bytes[i * DENTRY_LEN..(i + 1) * DENTRY_LEN];
        let first = slot[0];
        let attr = slot[11];

        if first == 0x00 {
            // Unused-from-here marker: in live directories this ends the dir,
            // but deleted entries can sit beyond it in slack — keep scanning,
            // just reset any partial LFN run.
            lfn_parts.clear();
            continue;
        }

        if attr == ATTR_LFN {
            // LFN slot (live or deleted). Collect its 13 UTF-16 chars keyed by
            // sequence number; the 0xE5 deletion marker is stripped first.
            let seq = if first == SLOT_DELETED {
                // A deleted LFN run loses its real seq high byte; treat order
                // by appearance instead (push with synthetic descending seq).
                0
            } else {
                first & LFN_SEQ_MASK
            };
            lfn_parts.push((seq, decode_lfn_chars(slot)));
            continue;
        }

        if first != SLOT_DELETED {
            // Live short entry — not our concern; flush any LFN run.
            lfn_parts.clear();
            continue;
        }

        if attr & ATTR_VOLUME_ID != 0 {
            lfn_parts.clear();
            continue;
        }

        // Deleted short (8.3) entry.
        let name = reassemble_name(&lfn_parts, slot);
        lfn_parts.clear();
        if name.is_empty() {
            continue;
        }
        let is_directory = attr & ATTR_DIRECTORY != 0;
        let size = u32::from_le_bytes([slot[28], slot[29], slot[30], slot[31]]) as u64;
        let high = if geom.is_fat32 {
            u16::from_le_bytes([slot[20], slot[21]]) as u32 & 0x0fff
        } else {
            0
        };
        let first_cluster = (high << 16) | u16::from_le_bytes([slot[26], slot[27]]) as u32;
        let runs = contiguous_runs(geom, first_cluster, size);
        out.push(RecoveredEntry {
            slot_offset: slot_offsets.get(i).copied().unwrap_or((i * DENTRY_LEN) as u64),
            name,
            is_directory,
            size,
            first_cluster,
            timestamps: short_entry_timestamps(slot),
            runs,
        });
    }
    out
}

/// Repair deleted FAT32 entries whose 32-bit start cluster lost its **high 16
/// bits** when Windows zeroed `DIR_FstClusHI` (offset 20-21) on delete — a
/// historical quirk that silently corrupts the start cluster of any file that
/// began at or above cluster 65536 (8 KiB clusters → the 512 MiB mark). The
/// surviving low word (`DIR_FstClusLO`) is only `true_cluster % 65536`, so the
/// recorded value can land deep inside another file's allocated extent and
/// point recovery at garbage.
///
/// A deleted file's FAT chain is cleared, so its true clusters are now **free**.
/// We exploit that: the real start cluster heads a contiguous free run long
/// enough to hold the file (the contiguity FAT recovery already assumes). For
/// each entry whose high word is zero we search ascending high words
/// (`(high << 16) | low`) for the first candidate that heads such a run, and
/// rewrite the cluster + data run when it differs from the recorded value. The
/// recorded low word is tried first (`high == 0`): a file that genuinely began
/// below cluster 65536 keeps its cluster untouched.
///
/// Applies to **deleted** entries only (the caller never passes live entries) —
/// a live file's clusters are still allocated, so the free-run test would
/// misfire. Best-effort: when no candidate heads a full free run, the recorded
/// cluster is left as-is.
pub fn apply_cluster_reconstruction(
    entries: &mut [RecoveredEntry],
    geom: &FatGeom,
    fat: &FatTable,
) {
    if !geom.is_fat32 {
        // FAT12/16 cluster numbers never exceed 16 bits, so `DIR_FstClusHI` is
        // always zero by definition — nothing was lost.
        return;
    }
    for entry in entries.iter_mut() {
        if entry.is_directory || entry.size == 0 || entry.first_cluster < 2 {
            continue;
        }
        if entry.first_cluster >> 16 != 0 {
            // High word intact (this OS/tool did not zero it) — trust it.
            continue;
        }
        if let Some(repaired) =
            reconstruct_truncated_first_cluster(geom, fat, entry.first_cluster, entry.size)
        {
            if repaired != entry.first_cluster {
                entry.first_cluster = repaired;
                entry.runs = contiguous_runs(geom, repaired, entry.size);
            }
        }
    }
}

/// Find the true start cluster for a deleted entry whose high word was zeroed.
/// Returns the recorded low word when it already heads a full contiguous free
/// run (high word was genuinely zero), the first higher-word candidate that
/// does, or `None` when none qualifies (caller keeps the recorded value).
fn reconstruct_truncated_first_cluster(
    geom: &FatGeom,
    fat: &FatTable,
    recorded: u32,
    size: u64,
) -> Option<u32> {
    let low = recorded & 0xFFFF;
    if low < 2 {
        return None;
    }
    let cluster_size = geom.cluster_size();
    if cluster_size == 0 {
        return None;
    }
    let need = size.div_ceil(cluster_size);
    let max_cluster = 2 + geom.cluster_count() as u64;
    if heads_free_run(fat, low, need, max_cluster) {
        return Some(low);
    }
    // The high word was zeroed: try (high << 16) | low for ascending high words,
    // bounded by the volume's cluster count, and take the first that heads a
    // full free run of the file's length.
    let max_high = ((max_cluster >> 16) + 1) as u32;
    for high in 1..=max_high {
        let candidate = (high << 16) | low;
        if u64::from(candidate) >= max_cluster {
            break;
        }
        if heads_free_run(fat, candidate, need, max_cluster) {
            return Some(candidate);
        }
    }
    None
}

/// True when `need` consecutive in-range clusters starting at `start` are all
/// free in the FAT — i.e. `start` heads a contiguous free region large enough
/// to hold a `need`-cluster file. `need == 0` and out-of-range starts are false.
fn heads_free_run(fat: &FatTable, start: u32, need: u64, max_cluster: u64) -> bool {
    if start < 2 || need == 0 || u64::from(start) >= max_cluster {
        return false;
    }
    let mut cluster = u64::from(start);
    let mut remaining = need;
    while remaining > 0 {
        if cluster >= max_cluster || !fat.is_free(cluster as u32) {
            return false;
        }
        cluster += 1;
        remaining -= 1;
    }
    true
}

/// Reconstruct a best-effort contiguous data run from the entry's first cluster
/// spanning `ceil(size / cluster_size)` clusters, clamped to the volume.
fn contiguous_runs(geom: &FatGeom, first_cluster: u32, size: u64) -> Vec<FsDataRun> {
    if first_cluster < 2 || size == 0 {
        return Vec::new();
    }
    let cluster_size = geom.cluster_size();
    if cluster_size == 0 {
        return Vec::new();
    }
    let clusters_needed = size.div_ceil(cluster_size);
    let max_cluster = 2 + geom.cluster_count() as u64;
    if (first_cluster as u64) >= max_cluster {
        return Vec::new();
    }
    let available = max_cluster - first_cluster as u64;
    let clusters = clusters_needed.min(available);
    if clusters == 0 {
        return Vec::new();
    }
    vec![FsDataRun {
        logical_offset: 0,
        physical_offset: geom.cluster_offset(first_cluster),
        length: (clusters * cluster_size).min(size),
    }]
}

/// Decode the 13 UTF-16 code units carried by one LFN slot.
fn decode_lfn_chars(slot: &[u8]) -> [u16; 13] {
    let mut chars = [0u16; 13];
    let offsets = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (i, &off) in offsets.iter().enumerate() {
        chars[i] = u16::from_le_bytes([slot[off], slot[off + 1]]);
    }
    chars
}

/// Reassemble the display name: prefer the LFN run (ordered by sequence number,
/// highest first per the FAT spec); fall back to the short 8.3 name with the
/// lost first character substituted by `_` (TSK convention).
fn reassemble_name(lfn_parts: &[(u8, [u16; 13])], short_slot: &[u8]) -> String {
    if !lfn_parts.is_empty() {
        let mut parts = lfn_parts.to_vec();
        // The slot with seq=1 carries the FIRST 13 characters, seq=2 the next,
        // etc., so the name is the parts concatenated in ASCENDING sequence
        // order. On disk the slots are stored reversed (highest seq first).
        // When seq numbers are intact, sort ascending; deleted runs (seq 0)
        // lose their sequence bytes, so we fall back to reversing the on-disk
        // appearance order (which is reverse) to recover ascending order.
        if parts.iter().any(|(seq, _)| *seq != 0) {
            parts.sort_by_key(|(seq, _)| *seq);
        } else {
            parts.reverse();
        }
        let mut units: Vec<u16> = Vec::new();
        for (_, chars) in &parts {
            for &c in chars {
                if c == 0x0000 || c == 0xFFFF {
                    break;
                }
                units.push(c);
            }
        }
        let name = String::from_utf16_lossy(&units);
        let trimmed = name.trim_end_matches('\u{0}').to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    short_name_8_3(short_slot)
}

/// Decode the short 8.3 name, substituting `_` for the deleted first char.
fn short_name_8_3(slot: &[u8]) -> String {
    let mut raw = [0u8; 11];
    raw.copy_from_slice(&slot[0..11]);
    if raw[0] == SLOT_DELETED {
        raw[0] = b'_';
    }
    let base = String::from_utf8_lossy(&raw[0..8]).trim_end().to_string();
    let ext = String::from_utf8_lossy(&raw[8..11]).trim_end().to_string();
    let base: String = base.chars().filter(|c| !c.is_control()).collect();
    let ext: String = ext.chars().filter(|c| !c.is_control()).collect();
    if base.is_empty() {
        return String::new();
    }
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

fn short_entry_timestamps(slot: &[u8]) -> Mactb {
    let wtime = u16::from_le_bytes([slot[22], slot[23]]);
    let wdate = u16::from_le_bytes([slot[24], slot[25]]);
    let ctime = u16::from_le_bytes([slot[14], slot[15]]);
    let cdate = u16::from_le_bytes([slot[16], slot[17]]);
    let adate = u16::from_le_bytes([slot[18], slot[19]]);
    let modified = fat_datetime(wdate, wtime);
    let created = fat_datetime(cdate, ctime).and_then(|value| {
        let naive = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
        if slot[13] > 199 {
            return None;
        }
        Some(
            (naive + chrono::Duration::milliseconds(i64::from(slot[13]) * 10))
                .format("%Y-%m-%dT%H:%M:%S%.3f")
                .to_string(),
        )
    });
    Mactb {
        modified: modified.clone(),
        accessed: fat_date(adate),
        created: created.clone(),
        mft_changed: None,
        birth: created,
    }
}

fn fat_datetime(date: u16, time: u16) -> Option<String> {
    let year = 1980 + (date >> 9) as i32;
    let month = ((date >> 5) & 0x0F) as u32;
    let day = (date & 0x1F) as u32;
    let hour = (time >> 11) as u32;
    let minute = ((time >> 5) & 0x3F) as u32;
    let second = ((time & 0x1F) * 2) as u32;
    let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
    Some(naive.format("%Y-%m-%dT%H:%M:%S%.3f").to_string())
}

fn fat_date(date: u16) -> Option<String> {
    let year = 1980 + (date >> 9) as i32;
    let month = ((date >> 5) & 0x0F) as u32;
    let day = (date & 0x1F) as u32;
    let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(0, 0, 0)?;
    Some(naive.format("%Y-%m-%dT%H:%M:%S%.3f").to_string())
}

/// In-memory FAT table for chain following and free-cluster detection.
pub struct FatTable {
    entries: Vec<u32>,
    is_fat32: bool,
    is_fat12: bool,
}

impl FatTable {
    /// Load the selected active FAT from the image.
    pub fn load(image: &Arc<DiskImage>, geom: &FatGeom) -> Result<Self> {
        let fat_offset = geom.base_offset
            + (geom.reserved_sectors as u64 + geom.active_fat as u64 * geom.sectors_per_fat as u64)
                * geom.bytes_per_sector as u64;
        let fat_bytes_len = geom.sectors_per_fat as u64 * geom.bytes_per_sector as u64;
        let raw = image.read_range(fat_offset, fat_bytes_len as usize)?;
        let cluster_count = geom.cluster_count() as usize + 2;
        let max_cluster = geom.cluster_count() + 2;
        let is_fat32 = geom.is_fat32;
        let is_fat12 = !is_fat32 && max_cluster < 4085 + 2;
        let needed_bytes = if is_fat32 {
            cluster_count * 4
        } else if is_fat12 {
            (cluster_count * 3).div_ceil(2)
        } else {
            cluster_count * 2
        };
        if raw.len() as u64 != fat_bytes_len || raw.len() < needed_bytes {
            anyhow::bail!("FAT truncated allocation table");
        }
        let mut entries = Vec::with_capacity(cluster_count);
        if is_fat32 {
            for i in 0..cluster_count {
                let off = i * 4;
                if off + 4 > raw.len() {
                    break;
                }
                entries.push(
                    u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
                        & 0x0FFF_FFFF,
                );
            }
        } else if is_fat12 {
            for i in 0..cluster_count {
                let off = i * 3 / 2;
                if off + 2 > raw.len() {
                    break;
                }
                let pair = u16::from_le_bytes([raw[off], raw[off + 1]]);
                let val = if i & 1 == 0 { pair & 0x0FFF } else { pair >> 4 };
                entries.push(val as u32);
            }
        } else {
            for i in 0..cluster_count {
                let off = i * 2;
                if off + 2 > raw.len() {
                    break;
                }
                entries.push(u16::from_le_bytes([raw[off], raw[off + 1]]) as u32);
            }
        }
        Ok(Self {
            entries,
            is_fat32,
            is_fat12,
        })
    }

    fn next_cluster(&self, cluster: u32) -> u32 {
        self.entries.get(cluster as usize).copied().unwrap_or(0)
    }

    fn is_end_of_chain(&self, cluster: u32) -> bool {
        let val = self.next_cluster(cluster);
        if self.is_fat32 {
            val >= 0x0FFF_FFF8
        } else if self.is_fat12 {
            val >= 0x0FF8
        } else {
            val >= 0xFFF8
        }
    }

    /// A cluster is free when its FAT entry is 0.
    pub fn is_free(&self, cluster: u32) -> bool {
        self.entries.get(cluster as usize) == Some(&0)
    }
}

/// Read a partition's boot sector and parse FAT geometry.
///
/// A forensic image can carry a wiped or maliciously corrupted primary boot
/// sector (sector 0) while the FAT32 backup at sector 6 stays intact — the case
/// `D:\case\UDisk.img` hits, where sector 0 has no `0x55AA` signature and a
/// poisoned BPB (`root_entries=512`) that mis-parses as FAT16. The live
/// `FatVolume` path already overlays the backup BPB into its cursor so live
/// listing works; the recovery path used to read the broken primary directly
/// and silently recover nothing. Mirror the live path here: prefer the primary,
/// fall back to the sector-6 backup when the primary is not a structurally valid
/// FAT boot sector.
pub fn open_geom(image: &Arc<DiskImage>, base_offset: u64) -> Result<FatGeom> {
    let boot = read_effective_boot_sector(image, base_offset)?;
    FatGeom::parse(&boot, base_offset)
}

/// Resolve the boot sector that should drive recovery geometry: the primary at
/// `base_offset`, or the FAT32 backup at sector 6 when the primary is unusable.
fn read_effective_boot_sector(image: &Arc<DiskImage>, base_offset: u64) -> Result<[u8; 512]> {
    let primary = image
        .read_range(base_offset, BOOT_SECTOR_LEN)
        .map_err(|err| anyhow!("FAT boot sector read: {err}"))?;
    // The backup BPB is a faithful copy of the real geometry, so every region
    // offset still resolves relative to `base_offset` (the volume start) — only
    // the BPB *bytes* come from sector 6, never the addressing base.
    let logical_sector = primary
        .get(11..13)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
        .filter(|size| matches!(size, 512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768))
        .map(u64::from)
        .unwrap_or(image.sector_size as u64);
    let backup_sector = primary
        .get(50..52)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
        .filter(|sector| *sector > 0 && *sector != u16::MAX)
        .map(u64::from)
        .unwrap_or(FAT32_BACKUP_BOOT_SECTOR);
    let backup_offset = base_offset
        .checked_add(backup_sector * logical_sector)
        .ok_or_else(|| anyhow!("FAT backup offset overflow"))?;
    let backup = image.read_range(backup_offset, BOOT_SECTOR_LEN).ok();
    let chosen = choose_boot_sector(&primary, backup.as_deref());
    boot_sector_array(chosen)
}

/// Pure boot-sector selection: prefer a structurally valid primary, else a
/// structurally valid sector-6 backup, else the primary (best-effort — let
/// `FatGeom::parse` surface a precise error instead of masking it here).
fn choose_boot_sector<'a>(primary: &'a [u8], backup: Option<&'a [u8]>) -> &'a [u8] {
    if boot_sector_looks_valid(primary) {
        return primary;
    }
    if let Some(backup) = backup {
        if boot_sector_looks_valid(backup) {
            return backup;
        }
    }
    primary
}

/// Structural sanity check that an intact FAT boot sector (FAT12/16/32) passes
/// but a wiped or garbage sector fails: the `0x55AA` signature, a sane BPB, and
/// a `FAT…` filesystem-type label in either the FAT12/16 slot (offset 54) or the
/// FAT32 slot (offset 82). Mirrors `volumes::fat::is_valid_fat_boot_sector` (the
/// live path) without needing the partition length.
fn boot_sector_looks_valid(boot: &[u8]) -> bool {
    if boot.len() < BOOT_SECTOR_LEN || boot[510..512] != [0x55, 0xAA] {
        return false;
    }
    let bytes_per_sector = u16::from_le_bytes([boot[11], boot[12]]);
    if !matches!(
        bytes_per_sector,
        512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768
    ) {
        return false;
    }
    let sectors_per_cluster = boot[13];
    if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
        return false;
    }
    let reserved_sectors = u16::from_le_bytes([boot[14], boot[15]]);
    let num_fats = boot[16];
    if reserved_sectors == 0 || !matches!(num_fats, 1 | 2) {
        return false;
    }
    let total_16 = u16::from_le_bytes([boot[19], boot[20]]);
    let total_32 = u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]]);
    if total_16 == 0 && total_32 == 0 {
        return false;
    }
    FatGeom::parse(boot, 0).is_ok()
}

fn boot_sector_array(bytes: &[u8]) -> Result<[u8; 512]> {
    let slice = bytes
        .get(..BOOT_SECTOR_LEN)
        .ok_or_else(|| anyhow!("FAT boot sector too short"))?;
    let mut out = [0u8; BOOT_SECTOR_LEN];
    out.copy_from_slice(slice);
    Ok(out)
}

/// Resolve an absolute volume path (leading `/`, or empty for root) to the
/// directory's first cluster, walking live short/LFN entries from the root.
/// Returns `0` for the root (signals the fixed root region on FAT12/16).
pub fn resolve_dir_cluster(
    image: &Arc<DiskImage>,
    geom: &FatGeom,
    fat: &FatTable,
    path: &str,
) -> Result<u32> {
    let mut cluster = 0u32; // 0 = root
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let (dir_bytes, _) = read_dir_bytes(image, geom, fat, cluster)?;
        let found = find_live_child_cluster(&dir_bytes, segment);
        match found {
            Some(next) => cluster = next,
            None => anyhow::bail!("FAT directory segment not found: {segment}"),
        }
    }
    Ok(cluster)
}

/// Find a live (non-deleted) directory child by name, returning its first
/// cluster. Matches against both the reassembled LFN and the short 8.3 name.
fn find_live_child_cluster(dir_bytes: &[u8], name: &str) -> Option<u32> {
    let mut lfn_parts: Vec<(u8, [u16; 13])> = Vec::new();
    let slots = dir_bytes.len() / DENTRY_LEN;
    for i in 0..slots {
        let slot = &dir_bytes[i * DENTRY_LEN..(i + 1) * DENTRY_LEN];
        let first = slot[0];
        let attr = slot[11];
        if first == 0x00 {
            break;
        }
        if first == SLOT_DELETED {
            lfn_parts.clear();
            continue;
        }
        if attr == ATTR_LFN {
            lfn_parts.push((first & LFN_SEQ_MASK, decode_lfn_chars(slot)));
            continue;
        }
        if attr & ATTR_VOLUME_ID != 0 {
            lfn_parts.clear();
            continue;
        }
        let entry_name = reassemble_name(&lfn_parts, slot);
        lfn_parts.clear();
        if entry_name.eq_ignore_ascii_case(name) {
            let cluster = ((u16::from_le_bytes([slot[20], slot[21]]) as u32) << 16)
                | u16::from_le_bytes([slot[26], slot[27]]) as u32;
            return Some(cluster);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lfn_slot(seq: u8, chars: &[u16], checksum: u8) -> [u8; 32] {
        let mut slot = [0u8; 32];
        slot[0] = seq;
        slot[11] = ATTR_LFN;
        slot[13] = checksum;
        let offsets = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        for (i, &off) in offsets.iter().enumerate() {
            let c = chars.get(i).copied().unwrap_or(0xFFFF);
            slot[off] = (c & 0xFF) as u8;
            slot[off + 1] = (c >> 8) as u8;
        }
        slot
    }

    fn short_slot(name8_3: &[u8; 11], attr: u8, first_cluster: u32, size: u32) -> [u8; 32] {
        let mut slot = [0u8; 32];
        slot[0..11].copy_from_slice(name8_3);
        slot[11] = attr;
        slot[20] = (first_cluster >> 16) as u8;
        slot[21] = (first_cluster >> 24) as u8;
        slot[26] = (first_cluster & 0xFF) as u8;
        slot[27] = ((first_cluster >> 8) & 0xFF) as u8;
        slot[28..32].copy_from_slice(&size.to_le_bytes());
        slot
    }

    fn geom() -> FatGeom {
        FatGeom {
            base_offset: 0,
            bytes_per_sector: 512,
            sectors_per_cluster: 1,
            reserved_sectors: 1,
            num_fats: 2,
            sectors_per_fat: 1,
            root_entries: 512,
            total_sectors: 100_000,
            root_cluster: 0,
            is_fat32: false,
            active_fat: 0,
        }
    }

    #[test]
    fn recovers_deleted_short_entry_with_substituted_first_char() {
        let g = geom();
        let mut buf = Vec::new();
        let mut short = short_slot(b"XELLO   TXT", 0x20, 5, 1234);
        short[0] = SLOT_DELETED; // deleted: first char lost
        buf.extend_from_slice(&short);
        let recovered = scan_dir_deleted(&buf, &[], &g);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].name, "_ELLO.TXT");
        assert_eq!(recovered[0].size, 1234);
        assert_eq!(recovered[0].first_cluster, 5);
        assert_eq!(recovered[0].runs.len(), 1);
        assert_eq!(recovered[0].runs[0].length, 1234);
    }

    #[test]
    fn reassembles_deleted_lfn_run() {
        let g = geom();
        // "report.doc" split across one LFN slot, both LFN and short deleted.
        let name: Vec<u16> = "report.doc".encode_utf16().collect();
        let mut lfn = lfn_slot(0x40 | 1, &name, 0x00);
        lfn[0] = SLOT_DELETED;
        let mut short = short_slot(b"REPORT~1DOC", 0x20, 7, 4096);
        short[0] = SLOT_DELETED;
        let mut buf = Vec::new();
        buf.extend_from_slice(&lfn);
        buf.extend_from_slice(&short);
        let recovered = scan_dir_deleted(&buf, &[], &g);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].name, "report.doc");
    }

    #[test]
    fn skips_live_and_volume_entries() {
        let g = geom();
        let mut buf = Vec::new();
        buf.extend_from_slice(&short_slot(b"LIVE    TXT", 0x20, 3, 10)); // live
        buf.extend_from_slice(&short_slot(b"VOLUME  ID ", ATTR_VOLUME_ID, 0, 0)); // volid
        let recovered = scan_dir_deleted(&buf, &[], &g);
        assert!(recovered.is_empty());
    }

    #[test]
    fn parses_fat32_geometry() {
        let mut boot = vec![0u8; 512];
        boot[11] = 0x00;
        boot[12] = 0x02; // 512 bytes/sector
        boot[13] = 8; // sectors/cluster
        boot[14] = 32; // reserved
        boot[16] = 2; // num fats
        boot[36] = 0x00;
        boot[37] = 0x08; // sectors per fat32 = 2048
        boot[32..36].copy_from_slice(&200_000u32.to_le_bytes()); // total32
        boot[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
        let g = FatGeom::parse(&boot, 1024).unwrap();
        assert!(g.is_fat32);
        assert_eq!(g.sectors_per_cluster, 8);
        assert_eq!(g.root_cluster, 2);
        assert_eq!(g.base_offset, 1024);
    }

    /// Build a structurally valid FAT32 boot sector (the geometry of
    /// `D:\case\UDisk.img`'s sector-6 backup).
    fn valid_fat32_boot() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"MSDOS5.0");
        b[11..13].copy_from_slice(&512u16.to_le_bytes());
        b[13] = 16; // sectors/cluster
        b[14..16].copy_from_slice(&3274u16.to_le_bytes()); // reserved
        b[16] = 2; // num fats
                   // root_entries (17..19) and total_16 (19..21) stay 0 → FAT32.
        b[32..36].copy_from_slice(&30_233_588u32.to_le_bytes()); // total32
        b[36..40].copy_from_slice(&14_747u32.to_le_bytes()); // sectors per fat32
        b[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
        b[82..90].copy_from_slice(b"FAT32   ");
        b[510..512].copy_from_slice(&[0x55, 0xAA]);
        b
    }

    /// Reproduce the wiped/poisoned primary boot sector of `D:\case\UDisk.img`:
    /// FAT-ish BPB but no `0x55AA` signature and `root_entries=512` (illegal for
    /// FAT32, so a naive parse mis-classifies it as FAT16).
    fn wiped_primary_boot() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
        b[3..11].copy_from_slice(b"MSDOS5.0");
        b[11..13].copy_from_slice(&512u16.to_le_bytes());
        b[13] = 8; // wrong sectors/cluster
        b[14..16].copy_from_slice(&3274u16.to_le_bytes());
        b[16] = 2;
        b[17..19].copy_from_slice(&512u16.to_le_bytes()); // poison: illegal for FAT32
        b[32..36].copy_from_slice(&50_000_000u32.to_le_bytes()); // > image
        b[36..40].copy_from_slice(&30_000u32.to_le_bytes());
        b[44..48].copy_from_slice(&5u32.to_le_bytes());
        b[82..90].copy_from_slice(b"  T32   "); // corrupted label
                                                // no 0x55AA signature
        b
    }

    #[test]
    fn boot_sector_validity_rejects_wiped_primary_accepts_backup() {
        assert!(!boot_sector_looks_valid(&wiped_primary_boot()));
        assert!(boot_sector_looks_valid(&valid_fat32_boot()));
    }

    #[test]
    fn boot_sector_validity_accepts_fat16_label() {
        let mut b = valid_fat32_boot();
        // Move the FAT label to the FAT12/16 slot and clear the FAT32 one.
        b[54..62].copy_from_slice(b"FAT16   ");
        b[82..90].copy_from_slice(&[0u8; 8]);
        assert!(boot_sector_looks_valid(&b));
    }

    /// Regression for `D:\case\UDisk.img`: the wiped primary must be skipped in
    /// favour of the sector-6 backup, yielding a correct FAT32 geometry instead
    /// of the bogus FAT16 the primary would parse as.
    #[test]
    fn choose_boot_sector_falls_back_to_fat32_backup() {
        let primary = wiped_primary_boot();
        let backup = valid_fat32_boot();
        let chosen = choose_boot_sector(&primary, Some(&backup));
        let g = FatGeom::parse(chosen, 0).expect("parse backup geometry");
        assert!(g.is_fat32, "must recover as FAT32, not FAT16");
        assert_eq!(g.sectors_per_cluster, 16);
        assert_eq!(g.root_cluster, 2);
        assert_eq!(g.total_sectors, 30_233_588);
    }

    #[test]
    fn choose_boot_sector_keeps_valid_primary() {
        let primary = valid_fat32_boot();
        // A garbage backup must never be preferred over a valid primary.
        let backup = vec![0u8; 512];
        let chosen = choose_boot_sector(&primary, Some(&backup));
        assert_eq!(chosen[510..512], [0x55, 0xAA]);
        assert_eq!(chosen[13], 16);
    }

    #[test]
    fn choose_boot_sector_falls_back_to_primary_when_both_invalid() {
        let primary = wiped_primary_boot();
        let backup = vec![0u8; 512];
        let chosen = choose_boot_sector(&primary, Some(&backup));
        // Neither validates: best-effort primary, so parse can surface an error.
        assert_eq!(chosen[13], 8); // the primary's (wrong) sectors/cluster
    }

    /// End-to-end of the actual fix wiring (`read_effective_boot_sector`): a raw
    /// image with a wiped primary at sector 0 and a valid FAT32 backup at sector
    /// 6 — the on-disk shape of `D:\case\UDisk.img`. `open_geom` must read the
    /// backup, not the primary. Would FAIL before the fix (old `open_geom` read
    /// sector 0 and mis-parsed it as FAT16).
    #[test]
    fn open_geom_resolves_sector6_backup_through_diskimage() {
        use crate::core::evidence::image::DiskImage;
        use std::io::Write;
        use std::sync::Arc;

        let mut bytes = vec![0u8; 512 * 8];
        bytes[..512].copy_from_slice(&wiped_primary_boot());
        let backup = 512 * FAT32_BACKUP_BOOT_SECTOR as usize;
        bytes[backup..backup + 512].copy_from_slice(&valid_fat32_boot());

        let mut tmp = tempfile::NamedTempFile::new().expect("temp file");
        tmp.write_all(&bytes).expect("write image");
        tmp.flush().expect("flush");

        let image = Arc::new(DiskImage::open(tmp.path()).expect("open raw image"));
        assert_eq!(image.sector_size, 512);
        let geom = open_geom(&image, 0).expect("open_geom resolves geometry");
        assert!(geom.is_fat32, "must recover FAT32 from the sector-6 backup");
        assert_eq!(geom.sectors_per_cluster, 16);
        assert_eq!(geom.root_cluster, 2);
        assert_eq!(geom.total_sectors, 30_233_588);
    }

    /// Regression for the `read_cluster_chain` off-by-one that hid every
    /// root-directory deletion (e.g. `_00W.csv` on `D:\case\UDisk.img`): the
    /// FAT32 root is a single cluster whose own FAT entry is already the
    /// end-of-chain marker, so the old top-of-loop EOC test read zero bytes.
    /// A deleted entry sitting in that cluster must now be recovered.
    #[test]
    fn read_cluster_chain_reads_single_eoc_root_cluster() {
        use crate::core::evidence::image::DiskImage;
        use std::io::Write;
        use std::sync::Arc;

        let sector = 512usize;
        let total_sectors = 16u32;
        let mut img = vec![0u8; sector * total_sectors as usize];

        // FAT32 boot sector: sec/clus=1, reserved=4, one 1-sector FAT, root=2.
        {
            let b = &mut img[..sector];
            b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
            b[3..11].copy_from_slice(b"MSDOS5.0");
            b[11..13].copy_from_slice(&512u16.to_le_bytes());
            b[13] = 1;
            b[14..16].copy_from_slice(&4u16.to_le_bytes());
            b[16] = 1;
            b[32..36].copy_from_slice(&total_sectors.to_le_bytes());
            b[36..40].copy_from_slice(&1u32.to_le_bytes());
            b[44..48].copy_from_slice(&2u32.to_le_bytes());
            b[82..90].copy_from_slice(b"FAT32   ");
            b[510..512].copy_from_slice(&[0x55, 0xAA]);
        }
        // FAT (sector 4): FAT[2] = EOC, so the root is exactly one cluster.
        {
            let fat = &mut img[4 * sector..5 * sector];
            fat[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
            fat[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
            fat[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        }
        // Root cluster 2 (sector 5): one deleted entry, first char lost (like
        // `_00W.csv`): on-disk 8.3 is `00W    CSV` after the 0xE5 marker.
        {
            let root = &mut img[5 * sector..6 * sector];
            root[0] = SLOT_DELETED;
            root[1..11].copy_from_slice(b"00W    CSV");
            root[11] = 0x20;
            root[26..28].copy_from_slice(&7u16.to_le_bytes());
            root[28..32].copy_from_slice(&1234u32.to_le_bytes());
        }

        let mut tmp = tempfile::NamedTempFile::new().expect("temp file");
        tmp.write_all(&img).expect("write image");
        tmp.flush().expect("flush");
        let image = Arc::new(DiskImage::open(tmp.path()).expect("open raw image"));

        let geom = open_geom(&image, 0).expect("geometry");
        assert!(geom.is_fat32);
        assert_eq!(geom.root_cluster, 2);
        let fat = FatTable::load(&image, &geom).expect("fat table");
        let (dir, slot_offsets) = read_dir_bytes(&image, &geom, &fat, 0).expect("root dir bytes");
        assert_eq!(
            dir.len(),
            512,
            "single end-of-chain root cluster must be read, not skipped"
        );
        let recovered = scan_dir_deleted(&dir, &slot_offsets, &geom);
        assert_eq!(recovered.len(), 1, "the deleted root entry must surface");
        assert_eq!(recovered[0].name, "_00W.CSV");
        assert_eq!(recovered[0].size, 1234);
        // Root cluster 2 sits at sector 5; its first slot must report the
        // absolute image offset, matching live entries' `inode` scale.
        assert_eq!(recovered[0].slot_offset, (5 * sector) as u64);
    }

    /// Build a FAT32 geometry with 8 KiB clusters (the `UDisk.img` layout) and a
    /// cluster count comfortably above the test's highest cluster.
    fn fat32_geom_8k(cluster_count_min: u32) -> FatGeom {
        let sectors_per_cluster = 16; // 16 × 512 = 8192 B/cluster
        let reserved = 3274u32;
        let num_fats = 2u32;
        let sectors_per_fat = 14_747u32;
        let first_data = reserved + num_fats * sectors_per_fat;
        let total_sectors = first_data + (cluster_count_min + 16) * sectors_per_cluster;
        FatGeom {
            base_offset: 0,
            bytes_per_sector: 512,
            sectors_per_cluster,
            reserved_sectors: reserved,
            num_fats,
            sectors_per_fat,
            root_entries: 0,
            total_sectors,
            root_cluster: 2,
            is_fat32: true,
            active_fat: 0,
        }
    }

    /// A FAT table where every cluster is free except those in `allocated`.
    fn fat32_table_with_allocated(len: usize, allocated: &[u32]) -> FatTable {
        let mut entries = vec![0u32; len];
        for &c in allocated {
            if (c as usize) < entries.len() {
                entries[c as usize] = 0x0FFF_FFFF; // any non-zero = allocated
            }
        }
        FatTable {
            entries,
            is_fat32: true,
            is_fat12: false,
        }
    }

    /// The crux of `D:\Temp\disk\UDisk.img` (report §4): Windows zeroed the
    /// deleted CSV entry's start-cluster high word, leaving the low word 51564 —
    /// which falls inside the movie's still-recorded extent. The real file began
    /// at cluster 313708 (= 4 × 65536 + 51564), heading a contiguous free run.
    /// Reconstruction must rewrite 51564 → 313708 and re-point the data run.
    #[test]
    fn reconstructs_zeroed_high_word_to_true_cluster() {
        let true_cluster = 313_708u32;
        let low = 51_564u32; // 313708 mod 65536
        assert_eq!(true_cluster & 0xFFFF, low);
        let geom = fat32_geom_8k(true_cluster + 8);
        let len = (true_cluster + 8) as usize;
        // The CSV needs 3 clusters here (scaled down). Mirror the real disk: the
        // volume is nearly full, so every cluster is allocated *except* the true
        // run 313708..=313710 — including the lower-word candidates (51564,
        // 117100, 182636, 248172) the search would otherwise stop at. This makes
        // 313708 the first (and only) candidate that heads a 3-cluster free run.
        let mut allocated: Vec<u32> = (2..len as u32).collect();
        allocated.retain(|&c| c < true_cluster || c > true_cluster + 2);
        let fat = fat32_table_with_allocated(len, &allocated);
        let size = 2 * geom.cluster_size() + 1; // ceil → 3 clusters
        let mut entries = vec![RecoveredEntry {
            slot_offset: 0,
            name: "_00W.CSV".to_string(),
            is_directory: false,
            size,
            first_cluster: low, // high word zeroed: only the low word survived
            timestamps: Mactb::default(),
            runs: contiguous_runs(&geom, low, size),
        }];
        apply_cluster_reconstruction(&mut entries, &geom, &fat);
        assert_eq!(
            entries[0].first_cluster, true_cluster,
            "high word must be reconstructed to 4"
        );
        assert_eq!(entries[0].runs.len(), 1);
        assert_eq!(
            entries[0].runs[0].physical_offset,
            geom.cluster_offset(true_cluster),
            "data run must re-point to the true cluster's byte offset"
        );
    }

    /// A file that genuinely began below cluster 65536 (low word heads a full
    /// free run) must keep its recorded cluster — never relocated to a phantom
    /// higher-word candidate.
    #[test]
    fn keeps_low_cluster_when_it_heads_a_free_run() {
        let geom = fat32_geom_8k(1000);
        let fat = fat32_table_with_allocated(1000, &[]); // everything free
        let size = geom.cluster_size(); // 1 cluster
        let mut entries = vec![RecoveredEntry {
            slot_offset: 0,
            name: "SMALL.BIN".to_string(),
            is_directory: false,
            size,
            first_cluster: 100,
            timestamps: Mactb::default(),
            runs: contiguous_runs(&geom, 100, size),
        }];
        apply_cluster_reconstruction(&mut entries, &geom, &fat);
        assert_eq!(entries[0].first_cluster, 100, "low cluster must be kept");
    }

    /// When no candidate (low word or any higher word) heads a free run long
    /// enough, the recorded cluster is left untouched (best-effort).
    #[test]
    fn leaves_cluster_untouched_when_no_free_run_fits() {
        let geom = fat32_geom_8k(1000);
        // Mark the whole volume allocated so no run of the needed length is free.
        let allocated: Vec<u32> = (2..1000).collect();
        let fat = fat32_table_with_allocated(1000, &allocated);
        let size = geom.cluster_size();
        let mut entries = vec![RecoveredEntry {
            slot_offset: 0,
            name: "X.DAT".to_string(),
            is_directory: false,
            size,
            first_cluster: 500,
            timestamps: Mactb::default(),
            runs: Vec::new(),
        }];
        apply_cluster_reconstruction(&mut entries, &geom, &fat);
        assert_eq!(entries[0].first_cluster, 500);
    }

    /// FAT12/16 never carry a high word, so reconstruction is a no-op there even
    /// if the low word happens to be allocated.
    #[test]
    fn fat16_reconstruction_is_noop() {
        let mut geom = fat32_geom_8k(1000);
        geom.is_fat32 = false;
        let fat = fat32_table_with_allocated(1000, &[100]);
        let mut entries = vec![RecoveredEntry {
            slot_offset: 0,
            name: "OLD.TXT".to_string(),
            is_directory: false,
            size: 10,
            first_cluster: 100,
            timestamps: Mactb::default(),
            runs: Vec::new(),
        }];
        apply_cluster_reconstruction(&mut entries, &geom, &fat);
        assert_eq!(entries[0].first_cluster, 100, "FAT16 must not relocate");
    }
}
