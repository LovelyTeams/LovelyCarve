//! Unallocated-space virtual files.
//!
//! Every filesystem / disk / volume-group exposes the bytes it does *not*
//! account for as a **virtual file**: a synthetic [`FsNode`] whose `id`
//! encodes a `(absolute_offset, length)` range into the image and whose reads
//! land straight on [`EvidenceSession::read_range`], bypassing any filesystem
//! parsing. A logical search walks the node tree, and because these
//! nodes are `kind == File` they are scanned like any real file.
//!
//! Three placements share one addressing scheme (image-absolute byte
//! offset + length, both hex):
//!
//! | placement        | id format                       |
//! |------------------|---------------------------------|
//! | disk-level gap   | `diskgap:0x{off}:0x{len}`       |
//! | filesystem-level | `unalloc:part:{N}:0x{off}:0x{len}` |
//! | LVM volume-group | `unalloc:lvm:{N}:0x{off}:0x{len}`  |
//! | FS metadata file | `sysfile:part:{N}:0x{off}:0x{len}` |
//!
//! The middle `part:N` / `lvm:N` segment is informational; addressing only
//! reads the trailing two hex fields, so [`parse_virtual_unalloc_id`] handles
//! all of them uniformly. The `sysfile:` placement reuses the same machinery to
//! surface on-disk filesystem structures (FAT boot sector / FAT tables, etc.)
//! as readable virtual files.

use std::sync::Arc;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{detect_mime_bytes, shannon_entropy, FsDataRun, FsStat};

/// Id prefixes for the three virtual-file placements. Reads only look at the
/// trailing `0x{off}:0x{len}` so a single matcher serves all of them.
pub const DISK_GAP_PREFIX: &str = "diskgap:";
pub const UNALLOC_PREFIX: &str = "unalloc:";
/// Filesystem metadata structures surfaced as readable virtual files (FAT boot
/// sector / FAT tables, …). Shares the `0x{off}:0x{len}` addressing tail.
pub const SYSFILE_PREFIX: &str = "sysfile:";

/// Smallest gap worth surfacing on disk, so partition-alignment slivers and
/// the GPT backup header don't litter the tree with noise nodes. 512 KiB.
pub const MIN_DISK_GAP: u64 = 512 * 1024;

/// How a volume reports the space it does not allocate, in decreasing order of
/// precision. `children_for_image` turns this into one or more virtual nodes.
pub enum VolumeFree {
    /// Exact: the set of allocated `(absolute_offset, len)` extents. The
    /// complement within the volume is the unallocated set (possibly many
    /// segments → many nodes).
    Allocated(Vec<(u64, u64)>),
    /// Exact total but unknown layout: a single node covering the whole
    /// volume, but with `size` set to the true free-byte count.
    FreeBytes(u64),
    /// Degraded: no bitmap available — one node covering the entire volume
    /// physical range, `size` = whole volume (flagged approximate in the name).
    WholeVolume,
}

/// Compute the unallocated segments of `[base, base + total)` once the
/// `occupied` ranges (absolute offsets) are removed, dropping any leftover
/// shorter than `min_gap`. Pure arithmetic, no I/O — the shared core behind
/// disk gaps, LVM volume-group gaps, and exact filesystem unallocated space.
///
/// `occupied` may be unsorted and may overlap or spill outside the window;
/// segments are clamped to `[base, base + total)` and coalesced by sorting.
pub fn free_intervals(
    base: u64,
    total: u64,
    occupied: &[(u64, u64)],
    min_gap: u64,
) -> Vec<(u64, u64)> {
    let end = base.saturating_add(total);
    if end <= base {
        return Vec::new();
    }

    // Clamp each occupied range into the window, drop empties, sort by start.
    let mut ranges: Vec<(u64, u64)> = occupied
        .iter()
        .filter_map(|&(off, len)| {
            let start = off.max(base);
            let stop = off.saturating_add(len).min(end);
            (stop > start).then_some((start, stop))
        })
        .collect();
    ranges.sort_unstable_by_key(|&(start, _)| start);

    let mut free = Vec::new();
    let mut cursor = base;
    for (start, stop) in ranges {
        if start > cursor {
            let len = start - cursor;
            if len >= min_gap {
                free.push((cursor, len));
            }
        }
        cursor = cursor.max(stop);
    }
    if end > cursor {
        let len = end - cursor;
        if len >= min_gap {
            free.push((cursor, len));
        }
    }
    free
}

/// Decode a virtual unallocated-file id into `(absolute_offset, length)`.
/// Recognises both the `diskgap:` and `unalloc:` prefixes and reads the last
/// two `0x`-prefixed hex segments — the middle `part:N` / `lvm:N` is ignored
/// because addressing is uniform across placements.
pub fn parse_virtual_unalloc_id(id: &str) -> Option<(u64, u64)> {
    if !id.starts_with(DISK_GAP_PREFIX)
        && !id.starts_with(UNALLOC_PREFIX)
        && !id.starts_with(SYSFILE_PREFIX)
    {
        return None;
    }
    let mut segments = id.split(':');
    // The two trailing segments are offset then length.
    let mut last_two = ["", ""];
    let mut count = 0usize;
    for seg in segments.by_ref() {
        last_two[count % 2] = seg;
        count += 1;
    }
    if count < 3 {
        return None;
    }
    // `count % 2` now points at the slot *after* the last write; the last two
    // values live at the two slots, ordered by parity of `count`.
    let (off_str, len_str) = if count.is_multiple_of(2) {
        (last_two[0], last_two[1])
    } else {
        (last_two[1], last_two[0])
    };
    let offset = parse_hex_u64(off_str)?;
    let len = parse_hex_u64(len_str)?;
    Some((offset, len))
}

fn parse_hex_u64(s: &str) -> Option<u64> {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok()
}

/// Whether `id` names a virtual byte-range file (unallocated space, disk gap, or
/// a filesystem-metadata `sysfile:` structure). Cheap prefix test for call sites
/// that only need a yes/no (e.g. recursive-export skip).
pub fn is_virtual_unalloc_id(id: &str) -> bool {
    id.starts_with(DISK_GAP_PREFIX)
        || id.starts_with(UNALLOC_PREFIX)
        || id.starts_with(SYSFILE_PREFIX)
}

/// Build an [`FsStat`] for a virtual unallocated file by sampling its first
/// bytes — same shape `file_stat` returns for real files, so the detail pane
/// renders mime / entropy / magic without special-casing.
pub fn virtual_unalloc_stat(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    offset: u64,
    len: u64,
) -> FsStat {
    let sample = image
        .read_range(offset, (len as usize).min(4096))
        .unwrap_or_default();
    let magic_hex = (!sample.is_empty()).then(|| {
        sample
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    });
    let entropy = (!sample.is_empty()).then(|| shannon_entropy(&sample));
    FsStat {
        node_id: node_id.to_string(),
        size: len,
        inode: None,
        mode: None,
        mime: detect_mime_bytes(&sample).to_string(),
        magic_hex,
        entropy,
        // One run mapping the whole virtual file straight onto the image.
        data_runs: vec![FsDataRun {
            logical_offset: 0,
            physical_offset: offset,
            length: len,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disk_is_a_gap_when_no_partitions() {
        assert_eq!(free_intervals(0, 1000, &[], 1), vec![(0, 1000)]);
    }

    #[test]
    fn adjacent_partitions_leave_no_gap() {
        let occ = [(0, 400), (400, 600)];
        assert_eq!(free_intervals(0, 1000, &occ, 1), Vec::<(u64, u64)>::new());
    }

    #[test]
    fn leading_middle_and_trailing_gaps() {
        // occupied at [100,200) and [500,600); window [0,1000).
        let occ = [(100, 100), (500, 100)];
        assert_eq!(
            free_intervals(0, 1000, &occ, 1),
            vec![(0, 100), (200, 300), (600, 400)]
        );
    }

    #[test]
    fn min_gap_filters_small_slivers() {
        // gap [200,300)=100 dropped, gap [600,1000)=400 kept, with min_gap=200.
        let occ = [(100, 100), (300, 300)];
        assert_eq!(free_intervals(0, 1000, &occ, 200), vec![(600, 400)]);
    }

    #[test]
    fn unsorted_and_overlapping_occupied() {
        // Out of order, with an overlap [400,700) ∪ [600,800) = [400,800).
        let occ = [(600, 200), (100, 100), (400, 300)];
        assert_eq!(
            free_intervals(0, 1000, &occ, 1),
            vec![(0, 100), (200, 200), (800, 200)]
        );
    }

    #[test]
    fn occupied_spilling_outside_window_is_clamped() {
        // base=100,total=800 → window [100,900). Occupied spills both ends.
        let occ = [(0, 200), (850, 500)];
        // clamped to [100,200) and [850,900) → free [200,850).
        assert_eq!(free_intervals(100, 800, &occ, 1), vec![(200, 650)]);
    }

    #[test]
    fn lvm_base_offset_window() {
        // PV data area [4096, 4096+8192) with one LV [4096, 4096+4096).
        let occ = [(4096, 4096)];
        assert_eq!(free_intervals(4096, 8192, &occ, 1), vec![(8192, 4096)]);
    }

    #[test]
    fn parse_diskgap_id_roundtrip() {
        let id = "diskgap:0x100000:0x2cc000000";
        assert_eq!(parse_virtual_unalloc_id(id), Some((0x100000, 0x2cc000000)));
    }

    #[test]
    fn parse_unalloc_part_id() {
        let id = "unalloc:part:0:0x2800000:0xcc000";
        assert_eq!(parse_virtual_unalloc_id(id), Some((0x2800000, 0xcc000)));
    }

    #[test]
    fn parse_unalloc_lvm_id() {
        let id = "unalloc:lvm:2:0x40000:0x4c000000";
        assert_eq!(parse_virtual_unalloc_id(id), Some((0x40000, 0x4c000000)));
    }

    #[test]
    fn parse_sysfile_id() {
        // FAT $FAT1: reserved=3274 sectors → 0x199400, 14747 sectors → 0x732e00.
        let id = "sysfile:part:0:0x199400:0x732e00";
        assert_eq!(parse_virtual_unalloc_id(id), Some((0x199400, 0x732e00)));
        assert!(is_virtual_unalloc_id(id));
    }

    #[test]
    fn parse_rejects_non_virtual_ids() {
        assert_eq!(parse_virtual_unalloc_id("part:0"), None);
        assert_eq!(parse_virtual_unalloc_id("ntfs:part:0:5"), None);
        assert_eq!(parse_virtual_unalloc_id("ext4:part:0:2"), None);
        // Missing fields / non-hex.
        assert_eq!(parse_virtual_unalloc_id("diskgap:"), None);
        assert_eq!(parse_virtual_unalloc_id("diskgap:0x10"), None);
        assert_eq!(parse_virtual_unalloc_id("diskgap:zz:0x10"), None);
    }
}
