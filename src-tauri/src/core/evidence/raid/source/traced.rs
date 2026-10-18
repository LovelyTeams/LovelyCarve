//! Synthesized-byte bookkeeping for composite reads.
//!
//! A tolerant member backend zero-fills sectors it cannot read and reports
//! them as [`BadRegion`]s. The array readers carry those spans with the bytes,
//! piece by piece, so the assembled device reports them at its own offsets;
//! redundant levels also use them to serve such bytes from another copy or
//! from parity instead.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::evidence::image::BadRegion;

/// One contiguous piece of a read: its bytes and the piece-relative ranges of
/// them that are synthesized. Synthesized bytes are zero.
pub(crate) struct TracedBytes {
    pub(crate) bytes: Vec<u8>,
    /// Sorted, disjoint and non-adjacent.
    pub(crate) synthetic: Vec<Range<usize>>,
}

impl TracedBytes {
    /// The bytes a source returned for a read at `start`, with the absolute
    /// `faults` it reported for them. Faulted bytes are forced to zero, so a
    /// source that fills them otherwise cannot pass that filler on.
    pub(crate) fn from_read(mut bytes: Vec<u8>, faults: &[BadRegion], start: u64) -> Self {
        let synthetic = relative_ranges(faults, start, bytes.len());
        for range in &synthetic {
            if let Some(span) = bytes.get_mut(range.clone()) {
                span.fill(0);
            }
        }
        Self { bytes, synthetic }
    }

    pub(crate) fn is_authentic(&self) -> bool {
        self.synthetic.is_empty()
    }

    /// This piece as a whole read at `start`: its bytes and absolute regions.
    pub(crate) fn into_read(self, start: u64) -> (Vec<u8>, Vec<BadRegion>) {
        let faults = self
            .synthetic
            .iter()
            .map(|range| BadRegion {
                offset: start.saturating_add(range.start as u64),
                len: range.len() as u64,
            })
            .collect();
        (self.bytes, faults)
    }

    /// Replace synthesized bytes with authentic ones from another source of
    /// the same data. `alternative(range)` reads that piece-relative range
    /// from it: the bytes it returns authentic replace the synthesized ones,
    /// and only the spans both sources synthesized remain. A failed read leaves
    /// its range synthesized. Returns the number of bytes replaced.
    pub(crate) fn repair_from(
        &mut self,
        mut alternative: impl FnMut(Range<usize>) -> anyhow::Result<TracedBytes>,
    ) -> usize {
        let mut repaired = 0;
        let mut remaining = Vec::with_capacity(self.synthetic.len());
        for range in std::mem::take(&mut self.synthetic) {
            let replacement = match alternative(range.clone()) {
                Ok(replacement) if replacement.bytes.len() == range.len() => replacement,
                Ok(replacement) => {
                    tracing::debug!(
                        expected = range.len(),
                        returned = replacement.bytes.len(),
                        "redundant copy returned a different length; keeping synthesized bytes"
                    );
                    remaining.push(range);
                    continue;
                }
                Err(error) => {
                    tracing::debug!(
                        "redundant copy unavailable ({error:#}); keeping synthesized bytes"
                    );
                    remaining.push(range);
                    continue;
                }
            };
            for authentic in complement(&replacement.synthetic, range.len()) {
                let target = range.start + authentic.start..range.start + authentic.end;
                let (Some(target), Some(source)) =
                    (self.bytes.get_mut(target), replacement.bytes.get(authentic))
                else {
                    continue;
                };
                target.copy_from_slice(source);
                repaired += source.len();
            }
            remaining.extend(
                replacement
                    .synthetic
                    .iter()
                    .map(|span| range.start + span.start..range.start + span.end),
            );
        }
        self.synthetic = remaining;
        repaired
    }
}

/// A composite read assembled piece by piece in logical order.
pub(crate) struct TracedRead {
    start: u64,
    bytes: Vec<u8>,
    faults: Vec<BadRegion>,
}

impl TracedRead {
    pub(crate) fn new(start: u64, capacity: usize) -> Self {
        Self {
            start,
            bytes: Vec::with_capacity(capacity),
            faults: Vec::new(),
        }
    }

    /// Append the next piece; its synthesized ranges become absolute regions.
    pub(crate) fn push(&mut self, piece: TracedBytes) {
        let base = self.start.saturating_add(self.bytes.len() as u64);
        for range in piece.synthetic {
            let offset = base.saturating_add(range.start as u64);
            let len = range.len() as u64;
            match self.faults.last_mut() {
                Some(last) if last.offset.saturating_add(last.len) == offset => last.len += len,
                _ => self.faults.push(BadRegion { offset, len }),
            }
        }
        self.bytes.extend_from_slice(&piece.bytes);
    }

    pub(crate) fn finish(self) -> (Vec<u8>, Vec<BadRegion>) {
        (self.bytes, self.faults)
    }
}

/// Logs the first time a device serves synthesized member bytes from its
/// redundancy as a warning, and later repairs at debug level.
#[derive(Default)]
pub(crate) struct RepairLog(AtomicBool);

impl RepairLog {
    /// `bytes` synthesized bytes read from `slot` were replaced at logical
    /// `offset` of `device`.
    pub(crate) fn note(&self, device: &str, slot: usize, offset: u64, bytes: usize) {
        if self.0.swap(true, Ordering::Relaxed) {
            tracing::debug!(
                device,
                slot,
                offset,
                bytes,
                "served synthesized member bytes from redundancy"
            );
        } else {
            tracing::warn!(
                device,
                slot,
                offset,
                bytes,
                "member returned synthesized bytes; served them from another copy or parity \
                 (later repairs are logged at debug level)"
            );
        }
    }
}

/// `faults` (absolute offsets) inside a read of `len` bytes at `start`, as
/// ranges relative to `start`, sorted and coalesced.
pub(crate) fn relative_ranges(faults: &[BadRegion], start: u64, len: usize) -> Vec<Range<usize>> {
    let end = start.saturating_add(len as u64);
    let mut ranges: Vec<Range<usize>> = faults
        .iter()
        .filter_map(|region| {
            let from = region.offset.max(start);
            let to = region.offset.saturating_add(region.len).min(end);
            // Both lie within `start..=start + len`, so the differences fit.
            (to > from).then(|| (from - start) as usize..(to - start) as usize)
        })
        .collect();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// The parts of `0..len` outside `ranges` (sorted and disjoint).
pub(crate) fn complement(ranges: &[Range<usize>], len: usize) -> Vec<Range<usize>> {
    let mut gaps = Vec::with_capacity(ranges.len() + 1);
    let mut cursor = 0;
    for range in ranges {
        let start = range.start.min(len);
        if start > cursor {
            gaps.push(cursor..start);
        }
        cursor = cursor.max(range.end.min(len));
    }
    if cursor < len {
        gaps.push(cursor..len);
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(offset: u64, len: u64) -> BadRegion {
        BadRegion { offset, len }
    }

    #[test]
    fn relative_ranges_clip_sort_and_coalesce() {
        let faults = [
            region(120, 8),
            region(90, 20),
            region(108, 4),
            region(200, 9),
        ];
        assert_eq!(
            relative_ranges(&faults, 100, 104),
            vec![0..12, 20..28, 100..104]
        );
        assert_eq!(relative_ranges(&faults, 300, 8), Vec::<Range<usize>>::new());
    }

    #[test]
    fn from_read_zeroes_faulted_bytes() {
        let piece = TracedBytes::from_read(vec![0xAA; 8], &[region(1002, 3)], 1000);
        assert_eq!(piece.bytes, [0xAA, 0xAA, 0, 0, 0, 0xAA, 0xAA, 0xAA]);
        assert_eq!(piece.synthetic, vec![2..5]);
    }

    #[test]
    fn complement_lists_the_gaps() {
        assert_eq!(complement(&[2..4, 6..8], 10), vec![0..2, 4..6, 8..10]);
        assert_eq!(complement(&[0..10], 10), Vec::<Range<usize>>::new());
        assert_eq!(complement(&[], 3), vec![0..3]);
    }

    #[test]
    fn repair_keeps_only_spans_every_source_synthesized() {
        let mut piece = TracedBytes {
            bytes: vec![1, 1, 0, 0, 0, 0, 1, 1],
            synthetic: vec![2..6],
        };
        let repaired = piece.repair_from(|range| {
            assert_eq!(range, 2..6);
            Ok(TracedBytes {
                bytes: vec![9, 0, 0, 9],
                synthetic: vec![1..3],
            })
        });
        assert_eq!(repaired, 2);
        assert_eq!(piece.bytes, [1, 1, 9, 0, 0, 9, 1, 1]);
        assert_eq!(piece.synthetic, vec![3..5]);

        let unchanged = piece.repair_from(|_| anyhow::bail!("unreadable"));
        assert_eq!(unchanged, 0);
        assert_eq!(piece.synthetic, vec![3..5]);
    }

    #[test]
    fn pieces_join_into_absolute_coalesced_regions() {
        let mut read = TracedRead::new(4096, 16);
        read.push(TracedBytes {
            bytes: vec![0; 8],
            synthetic: vec![4..8],
        });
        read.push(TracedBytes {
            bytes: vec![0; 8],
            synthetic: vec![0..2, 6..8],
        });
        let (bytes, faults) = read.finish();
        assert_eq!(bytes.len(), 16);
        assert_eq!(faults, vec![region(4100, 6), region(4110, 2)]);
    }
}
