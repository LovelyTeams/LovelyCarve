//! Multi-segment logical addressing.
//!
//! AD1 tree/metadata/chunk addresses are logical offsets into a virtual space
//! formed by concatenating each segment's body with its 512-byte margin removed
//! (the C reference's `arbitrary_read`). [`SegmentSet`] owns the open segment
//! files and translates a logical offset + length into physical reads, spanning
//! segment boundaries. A single capacity check bounds every allocation to the
//! image's real on-disk size.
//!
//! Reads are positioned (`pread` / `ReadFile` at an offset) rather than
//! seek-then-read on a shared cursor, so one reader serves concurrent callers
//! without a lock.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::Ad1Error;

/// Bytes skipped at the start of every segment (header / margin).
pub(crate) const MARGIN: u64 = 512;

/// Largest accepted segment count. A 64 Ki-segment image at 1.5 GB/segment is
/// ~96 TB — generous; the cap stops an attacker-controlled `segment_number` from
/// driving a multi-gigabyte `Vec` allocation or a billions-long open loop.
const MAX_SEGMENTS: u32 = 65_536;

#[derive(Debug)]
struct Seg {
    file: File,
    /// Usable bytes in this segment = file length − [`MARGIN`].
    data_len: u64,
}

#[derive(Debug)]
pub(crate) struct SegmentSet {
    /// Index 0 == segment 1; `None` for a declared-but-missing segment.
    segs: Vec<Option<Seg>>,
    /// Path of every declared segment, present or not.
    paths: Vec<PathBuf>,
    /// Nominal per-segment stride = `fragments_size * 65536 − MARGIN`.
    stride: u64,
    /// Total usable bytes across all present segments (allocation bound).
    capacity: u64,
}

impl SegmentSet {
    /// Open segments `1..=segment_count` discovered alongside `first` (whose
    /// trailing character is replaced by the segment number, matching FTK's
    /// `.ad1`/`.ad2`/… naming). Segment 1 must already exist (the caller opened
    /// it for the headers); later missing segments are recorded as gaps.
    pub(crate) fn open(
        first: &Path,
        segment_count: u32,
        fragments_size: u32,
    ) -> Result<Self, Ad1Error> {
        // cov:unreachable — `Ad1Reader::open` validates `fragments_size != 0`
        // before constructing the SegmentSet; kept as a defense-in-depth guard so
        // a future direct caller cannot trigger the stride underflow below.
        if fragments_size == 0 {
            return Err(Ad1Error::Malformed(
                "segment header fragments_size is 0".into(),
            ));
        }
        if segment_count > MAX_SEGMENTS {
            return Err(Ad1Error::Malformed(format!(
                "segment header declares {segment_count} segments (> {MAX_SEGMENTS})"
            )));
        }
        let stride = u64::from(fragments_size) * 65536 - MARGIN;

        let first_str = first.to_string_lossy().to_string();
        // Base = path without its last character (the '1' of ".ad1").
        let mut base = first_str.clone();
        base.pop();

        // Grow as segments are found; never pre-allocate the attacker's count.
        let mut segs = Vec::new();
        let mut paths = Vec::new();
        let mut capacity = 0u64;
        for i in 1..=segment_count {
            let path = if i == 1 {
                first_str.clone()
            } else {
                format!("{base}{i}")
            };
            match File::open(&path) {
                Ok(f) => {
                    let len = f.metadata().map_or(0, |m| m.len());
                    let data_len = len.saturating_sub(MARGIN);
                    capacity += data_len;
                    segs.push(Some(Seg { file: f, data_len }));
                }
                Err(_) => segs.push(None),
            }
            paths.push(PathBuf::from(path));
        }

        Ok(Self {
            segs,
            paths,
            stride,
            capacity,
        })
    }

    /// Total usable bytes across present segments.
    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Path of every declared segment, in order (segment 1 first).
    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// 1-based indices of declared-but-missing segments.
    pub(crate) fn missing(&self) -> Vec<u32> {
        self.segs
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.is_none().then_some(i as u32 + 1))
            .collect()
    }

    /// How many bytes [`Self::read`] can deliver from logical `offset` before it
    /// runs out of data or reaches a missing segment, following the same
    /// segment-to-segment continuation as `read`.
    pub(crate) fn readable_from(&self, offset: u64) -> u64 {
        let mut total = 0u64;
        let mut seg_idx = (offset / self.stride) as usize;
        let mut within = offset % self.stride;
        while let Some(Some(seg)) = self.segs.get(seg_idx) {
            if within >= seg.data_len {
                break;
            }
            total += seg.data_len - within;
            seg_idx += 1;
            within = 0;
        }
        total
    }

    /// Read exactly `len` bytes starting at logical `offset`, spanning segments.
    ///
    /// Returns [`Ad1Error::Malformed`] if the range runs past available data or
    /// crosses into a missing segment, and refuses up-front to allocate more
    /// than the image's total data size.
    pub(crate) fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, Ad1Error> {
        if len == 0 {
            return Ok(Vec::new());
        }
        if len as u64 > self.capacity {
            return Err(Ad1Error::Malformed(format!(
                "read of {len} bytes exceeds image data size {}",
                self.capacity
            )));
        }
        let mut out = vec![0u8; len];
        let mut filled = 0usize;
        let mut off = offset;
        while filled < len {
            let seg_idx = (off / self.stride) as usize;
            let within = off % self.stride;
            let seg = self
                .segs
                .get(seg_idx)
                .and_then(|s| s.as_ref())
                .ok_or_else(|| {
                    Ad1Error::Malformed(format!(
                        "logical offset {off} needs missing segment {}",
                        seg_idx + 1
                    ))
                })?;
            if within >= seg.data_len {
                return Err(Ad1Error::Malformed(format!(
                    "logical offset {off} past segment {} data ({} bytes)",
                    seg_idx + 1,
                    seg.data_len
                )));
            }
            let avail = (seg.data_len - within) as usize;
            let want = (len - filled).min(avail);
            read_exact_at(&seg.file, &mut out[filled..filled + want], within + MARGIN)?;
            filled += want;
            off += want as u64;
        }
        Ok(out)
    }
}

/// Fill `buf` from `offset` without touching a shared file cursor.
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// Fill `buf` from `offset` without touching a shared file cursor.
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(Error::from(ErrorKind::UnexpectedEof)),
            Ok(read) => {
                let rest = std::mem::take(&mut buf);
                buf = &mut rest[read..];
                offset += read as u64;
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Positioned reads are only wired up for Unix and Windows.
#[cfg(not(any(unix, windows)))]
fn read_exact_at(_file: &File, _buf: &mut [u8], _offset: u64) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "positioned reads are not available on this platform",
    ))
}
