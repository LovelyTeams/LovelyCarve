use crate::backing::Backing;
use crate::bytes::{le_arr16, le_u32, le_u64};
use crate::error::{Result, VhdxError};
use crate::header::{crc32c, parse_active_header, VhdxHeader};

/// Apply a dirty VHDX log to the in-memory buffer before parsing.
///
/// If the active header's `LogGuid` is all-zeros or `LogLength` is zero the image
/// is clean and this is a no-op.  Otherwise every valid log entry whose
/// `LogGuid` matches the header is applied to `data` in ascending sequence
/// order so that subsequent region/metadata/BAT parsing sees the committed
/// state.
///
/// This is the whole-buffer path used by [`crate::VhdxReader::from_bytes`],
/// which already owns the bytes. The bounded `open(path)` path uses
/// [`build_overlay`] instead, which records the same patched sectors into a
/// small in-RAM overlay applied on top of positioned reads — so the two paths
/// are byte-identical for the same image.
pub(crate) fn apply(data: &mut [u8]) -> Result<()> {
    let header = parse_active_header(data)?;
    if header.log_length == 0 || header.log_guid == [0u8; 16] {
        return Ok(());
    }
    let log_start = header.log_offset as usize;
    let log_end = log_start.saturating_add(header.log_length as usize);
    if data.len() < log_end {
        return Err(VhdxError::OffsetOutOfBounds);
    }
    let entries = collect_entries(&data[log_start..log_end], &header.log_guid);
    for entry in entries {
        apply_entry(data, &entry);
    }
    Ok(())
}

/// A committed-log overlay: the sectors the log replays on top of the on-disk
/// container, keyed by their absolute container file offset.
///
/// For a cleanly-closed image (the common case) this is empty and reads are
/// pure positioned reads with no buffer. For a dirty image it holds only the
/// replaced sectors — bounded by the log length (~1 MB), never the whole image.
#[derive(Debug, Default, Clone)]
pub(crate) struct LogOverlay {
    /// `(file_offset, replacement bytes)` for each replayed run, in **replay
    /// order** (ascending log sequence number). Data descriptors contribute
    /// 4096-byte sectors; zero descriptors contribute a run of zeros. Applied
    /// in this exact order so a later entry overwrites an earlier overlapping
    /// one — identical to the in-place [`apply`] path, which writes the same
    /// runs to the buffer in the same order.
    runs: Vec<(u64, Vec<u8>)>,
}

impl LogOverlay {
    fn push(&mut self, file_offset: u64, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.runs.push((file_offset, bytes));
    }

    /// Patch `buf` (which was filled from container offset `read_off`) with any
    /// overlay run that overlaps `[read_off, read_off + buf.len())`, applied in
    /// replay order.
    ///
    /// Mirrors exactly what [`apply_entry`] writes to the whole buffer, so a
    /// positioned read followed by `patch` yields the same bytes the in-place
    /// `apply` path would have produced.
    pub(crate) fn patch(&self, buf: &mut [u8], read_off: u64) {
        if self.runs.is_empty() || buf.is_empty() {
            return;
        }
        let read_end = read_off.saturating_add(buf.len() as u64);
        for (run_start, run) in &self.runs {
            let run_start = *run_start;
            let run_end = run_start.saturating_add(run.len() as u64);
            // Overlap window in absolute container coordinates.
            let ov_start = run_start.max(read_off);
            let ov_end = run_end.min(read_end);
            if ov_start >= ov_end {
                continue;
            }
            let dst = (ov_start - read_off) as usize;
            let src = (ov_start - run_start) as usize;
            let n = (ov_end - ov_start) as usize;
            buf[dst..dst + n].copy_from_slice(&run[src..src + n]);
        }
    }
}

/// Build the [`LogOverlay`] for an image read through a positioned [`Backing`].
///
/// Returns an empty overlay for a clean image. For a dirty one it reads only the
/// bounded log region from the backing and records the replayed sectors — never
/// the whole container.
pub(crate) fn build_overlay(backing: &Backing, header: &VhdxHeader) -> Result<LogOverlay> {
    if header.log_length == 0 || header.log_guid == [0u8; 16] {
        return Ok(LogOverlay::default());
    }
    let log_start = header.log_offset;
    let log_end = log_start.saturating_add(u64::from(header.log_length));
    if backing.len() < log_end {
        return Err(VhdxError::OffsetOutOfBounds);
    }
    let log = backing.read_exact_at(log_start, header.log_length as usize)?;
    if log.len() < header.log_length as usize {
        return Err(VhdxError::OffsetOutOfBounds);
    }
    let entries = collect_entries(&log, &header.log_guid);
    let mut overlay = LogOverlay::default();
    for entry in entries {
        overlay_entry(&mut overlay, &entry);
    }
    Ok(overlay)
}

/// Walk a log region, returning the CRC-valid, GUID-matching entries in
/// ascending sequence order (the order in which they must be replayed).
fn collect_entries(log: &[u8], expected_guid: &[u8; 16]) -> Vec<Vec<u8>> {
    let mut entries: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut pos = 0usize;

    loop {
        if pos + 64 > log.len() {
            break;
        }
        if &log[pos..pos + 4] != b"loge" {
            break;
        }
        let entry_len = le_u32(log, pos + 8) as usize;
        if entry_len < 64 || pos + entry_len > log.len() {
            break;
        }

        let entry_bytes = log[pos..pos + entry_len].to_vec();

        // Validate CRC32C (computed with [4..8] zeroed).
        let stored = le_u32(&entry_bytes, 4);
        let mut tmp = entry_bytes.clone();
        tmp[4..8].fill(0);
        if crc32c(&tmp) != stored {
            pos += entry_len;
            continue;
        }

        // Skip entries whose LogGuid doesn't match the active header's.
        let entry_guid: [u8; 16] = le_arr16(&entry_bytes, 32);
        if &entry_guid != expected_guid {
            pos += entry_len;
            continue;
        }

        let seq = le_u64(&entry_bytes, 16);
        entries.push((seq, entry_bytes));
        pos += entry_len;
    }

    entries.sort_by_key(|(seq, _)| *seq);
    entries.into_iter().map(|(_, e)| e).collect()
}

fn apply_entry(data: &mut [u8], entry: &[u8]) {
    let descriptor_count = le_u32(entry, 24) as usize;
    let desc_start = 64;
    let data_sector_base = desc_start + descriptor_count * 32;
    let mut sector_idx = 0usize;

    for i in 0..descriptor_count {
        let d = &entry[desc_start + i * 32..desc_start + (i + 1) * 32];
        match &d[0..4] {
            b"desc" => {
                // Data descriptor: copy 4096-byte sector to FileOffset in the container.
                let file_off = le_u64(d, 24) as usize;
                let sector_off = data_sector_base + sector_idx * 4096;
                if sector_off + 4096 <= entry.len() && file_off + 4096 <= data.len() {
                    data[file_off..file_off + 4096]
                        .copy_from_slice(&entry[sector_off..sector_off + 4096]);
                }
                sector_idx += 1;
            }
            b"zero" => {
                // Zero descriptor: fill ZeroLength bytes at FileOffset with zeros.
                let zero_len = le_u64(d, 8) as usize;
                let file_off = le_u64(d, 16) as usize;
                let end = file_off.saturating_add(zero_len).min(data.len());
                if file_off < data.len() {
                    data[file_off..end].fill(0);
                }
            }
            _ => {}
        }
    }
}

/// Record one log entry's descriptors into the overlay — the bounded-read
/// counterpart to [`apply_entry`], writing the same sectors into RAM-side runs
/// instead of a whole-image buffer.
fn overlay_entry(overlay: &mut LogOverlay, entry: &[u8]) {
    let descriptor_count = le_u32(entry, 24) as usize;
    let desc_start = 64;
    let data_sector_base = desc_start + descriptor_count * 32;
    let mut sector_idx = 0usize;

    for i in 0..descriptor_count {
        let d = &entry[desc_start + i * 32..desc_start + (i + 1) * 32];
        match &d[0..4] {
            b"desc" => {
                let file_off = le_u64(d, 24);
                let sector_off = data_sector_base + sector_idx * 4096;
                if sector_off + 4096 <= entry.len() {
                    overlay.push(file_off, entry[sector_off..sector_off + 4096].to_vec());
                }
                sector_idx += 1;
            }
            b"zero" => {
                let zero_len = le_u64(d, 8) as usize;
                let file_off = le_u64(d, 16);
                if zero_len > 0 {
                    overlay.push(file_off, vec![0u8; zero_len]);
                }
            }
            _ => {}
        }
    }
}
