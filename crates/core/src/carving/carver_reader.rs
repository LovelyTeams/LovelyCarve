//! Bounded, buffered reader used by the Digler-compatible format decoders.
//!
//! A decoder sees its artifact as a byte stream that starts at the signature.
//! Bytes still inside the current scan window are served from that window
//! without touching the evidence; bytes past it come from a worker-owned
//! look-ahead buffer. A decoder that actually read most of the buffered bytes
//! before asking for more is streaming, and its next refill doubles (up to
//! 1 MiB), so byte-at-a-time decoders and long marker searches cost one bulk
//! evidence read per refill. Anything else (skipping member data to the next
//! ZIP header, peeking at a PDF xref offset) refills with a small read.

use anyhow::{bail, Context};
use memchr::memmem;
use tokio_util::sync::CancellationToken;

use crate::carving::carver::{CarveScanError, CarveSource};

/// First refill of a decoder streaming on past the end of the scan window.
const FIRST_REFILL_BYTES: usize = 64 * 1024;
/// Refill for a decoder that is not streaming.
const SMALL_REFILL_BYTES: usize = 4 * 1024;
const MAX_REFILL_BYTES: usize = 1024 * 1024;

pub(crate) struct CarveReader<'a, S> {
    source: &'a S,
    /// Rest of the scan window, starting at the signature (relative offset 0).
    window: &'a [u8],
    /// Look-ahead bytes covering `[lookahead_start, lookahead_start + len)`.
    lookahead: &'a mut Vec<u8>,
    lookahead_start: u64,
    next_refill: usize,
    /// Bytes handed to the decoder (not skipped) since the last refill.
    read_since_refill: u64,
    cancellation: &'a CancellationToken,
    start: u64,
    size: u64,
    position: u64,
}

impl<'a, S: CarveSource> CarveReader<'a, S> {
    pub(crate) fn new(
        source: &'a S,
        start: u64,
        size: u64,
        window: &'a [u8],
        lookahead: &'a mut Vec<u8>,
        cancellation: &'a CancellationToken,
    ) -> Self {
        lookahead.clear();
        Self {
            source,
            window,
            lookahead,
            lookahead_start: 0,
            next_refill: FIRST_REFILL_BYTES,
            read_since_refill: 0,
            cancellation,
            start,
            size,
            position: 0,
        }
    }

    /// Bytes consumed so far, i.e. the current offset from the signature.
    pub(crate) fn bytes_read(&self) -> u64 {
        self.position
    }

    /// Copies `destination.len()` bytes at `relative` (from the signature)
    /// without moving the reader.
    pub(crate) fn peek_at(&mut self, relative: u64, destination: &mut [u8]) -> anyhow::Result<()> {
        let length = destination.len();
        destination.copy_from_slice(&self.slice_at(relative, length)?[..length]);
        Ok(())
    }

    pub(crate) fn remaining(&self) -> u64 {
        self.size.saturating_sub(self.position)
    }

    /// Bytes readable from `relative` (from the signature) to the bound.
    pub(crate) fn remaining_from(&self, relative: u64) -> u64 {
        self.size.saturating_sub(relative)
    }

    pub(crate) fn read_byte(&mut self) -> anyhow::Result<u8> {
        let byte = self.slice_at(self.position, 1)?[0];
        self.advance(1);
        Ok(byte)
    }

    pub(crate) fn read_exact(&mut self, destination: &mut [u8]) -> anyhow::Result<()> {
        let length = destination.len();
        destination.copy_from_slice(&self.slice_at(self.position, length)?[..length]);
        self.advance(length as u64);
        Ok(())
    }

    pub(crate) fn read_array<const N: usize>(&mut self) -> anyhow::Result<[u8; N]> {
        let mut bytes = [0u8; N];
        self.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn read_vec(&mut self, length: usize) -> anyhow::Result<Vec<u8>> {
        let bytes = self.slice_at(self.position, length)?[..length].to_vec();
        self.advance(length as u64);
        Ok(bytes)
    }

    pub(crate) fn peek_vec(&mut self, length: usize) -> anyhow::Result<Vec<u8>> {
        Ok(self.slice_at(self.position, length)?[..length].to_vec())
    }

    /// Consumes `length` bytes, handing them to `visit` as contiguous slices.
    pub(crate) fn consume_with(
        &mut self,
        length: u64,
        mut visit: impl FnMut(&[u8]),
    ) -> anyhow::Result<()> {
        if length > self.remaining() {
            bail!("unexpected end of carve reader");
        }
        let end = self.position + length;
        while self.position < end {
            let wanted = (end - self.position).min(MAX_REFILL_BYTES as u64) as usize;
            let slice = self.slice_at(self.position, 1)?;
            let taken = slice.len().min(wanted);
            visit(&slice[..taken]);
            self.advance(taken as u64);
        }
        Ok(())
    }

    /// Consumes bytes up to and including the next `needle` byte.
    pub(crate) fn skip_past_byte(&mut self, needle: u8) -> anyhow::Result<()> {
        loop {
            let slice = self.slice_at(self.position, 1)?;
            let visible = slice.len() as u64;
            if let Some(index) = memchr::memchr(needle, slice) {
                self.advance(index as u64 + 1);
                return Ok(());
            }
            self.advance(visible);
        }
    }

    pub(crate) fn discard_up_to(&mut self, length: u64) -> anyhow::Result<u64> {
        if self.cancellation.is_cancelled() {
            return Err(CarveScanError::Cancelled.into());
        }
        let discarded = length.min(self.remaining());
        self.position += discarded;
        Ok(discarded)
    }

    pub(crate) fn discard_exact(&mut self, length: u64) -> anyhow::Result<()> {
        if self.discard_up_to(length)? != length {
            bail!("unexpected end of carve reader");
        }
        Ok(())
    }

    pub(crate) fn unread(&mut self, length: u64) -> anyhow::Result<()> {
        if length > self.position {
            bail!("cannot unread before carve artifact start");
        }
        self.position -= length;
        Ok(())
    }

    /// Moves past bytes the decoder read (as opposed to skipped).
    fn advance(&mut self, length: u64) {
        self.position += length;
        self.read_since_refill += length;
    }

    /// Returns at least `wanted` bytes starting at `relative` (more when the
    /// window or look-ahead already holds them), never past the reader bound.
    fn slice_at(&mut self, relative: u64, wanted: usize) -> anyhow::Result<&[u8]> {
        let end = relative
            .checked_add(wanted as u64)
            .context("carve reader offset overflow")?;
        if end > self.size {
            bail!("unexpected end of carve reader");
        }
        let visible = self.size - relative;
        if end <= self.window.len() as u64 {
            let from = relative as usize;
            let to = self.window.len().min(from.saturating_add(visible as usize));
            return Ok(&self.window[from..to]);
        }
        let lookahead_end = self.lookahead_start + self.lookahead.len() as u64;
        if relative < self.lookahead_start || end > lookahead_end {
            self.refill(relative, wanted)?;
        }
        let from = (relative - self.lookahead_start) as usize;
        Ok(&self.lookahead[from..])
    }

    fn refill(&mut self, relative: u64, wanted: usize) -> anyhow::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(CarveScanError::Cancelled.into());
        }
        let (buffered_start, buffered_end) = if self.lookahead.is_empty() {
            (0, self.window.len() as u64)
        } else {
            (
                self.lookahead_start,
                self.lookahead_start + self.lookahead.len() as u64,
            )
        };
        let streaming = (buffered_start..=buffered_end).contains(&relative)
            && self.read_since_refill.saturating_mul(2) >= buffered_end - buffered_start;
        let hint = if streaming {
            self.next_refill
        } else {
            SMALL_REFILL_BYTES
        };
        self.next_refill = (hint * 2).min(MAX_REFILL_BYTES);
        self.read_since_refill = 0;
        let limit = self.size - relative;
        let length = (wanted.max(hint) as u64).min(limit) as usize;
        self.lookahead.clear();
        self.lookahead.resize(length, 0);
        let mut filled = 0usize;
        if let Ok(window_offset) = usize::try_from(relative) {
            if window_offset < self.window.len() {
                filled = (self.window.len() - window_offset).min(length);
                self.lookahead[..filled]
                    .copy_from_slice(&self.window[window_offset..window_offset + filled]);
            }
        }
        while filled < length {
            let absolute = self
                .start
                .checked_add(relative + filled as u64)
                .context("carve reader absolute offset overflow")?;
            let read = self
                .source
                .read_into(absolute, &mut self.lookahead[filled..length])
                .with_context(|| format!("read carve artifact at offset {absolute}"))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        self.lookahead.truncate(filled);
        self.lookahead_start = relative;
        if filled < wanted {
            bail!("unexpected end of carve source");
        }
        Ok(())
    }
}

/// Positions the reader at the first `signature` that lies entirely within
/// the next `maximum` bytes, or consumes that whole span and returns `false`.
pub(crate) fn seek_at<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    signature: &[u8],
    maximum: u64,
) -> anyhow::Result<bool> {
    if signature.is_empty() {
        return Ok(true);
    }
    let origin = reader.position;
    let limit = origin + maximum.min(reader.remaining());
    let finder = memmem::Finder::new(signature);
    let needle = signature.len() as u64;
    let mut cursor = origin;
    while cursor + needle <= limit {
        let slice = reader.slice_at(cursor, signature.len())?;
        let searchable = (slice.len() as u64).min(limit - cursor) as usize;
        if let Some(index) = finder.find(&slice[..searchable]) {
            reader.read_since_refill += index as u64;
            reader.position = cursor + index as u64;
            return Ok(true);
        }
        // Every byte passed is scanned, which counts as reading it.
        reader.read_since_refill += searchable as u64;
        cursor += searchable as u64 - (needle - 1);
    }
    reader.position = limit;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CountingSource {
        bytes: Vec<u8>,
        reads: AtomicUsize,
    }

    impl CountingSource {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                bytes,
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl CarveSource for CountingSource {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(self.bytes.len());
            let length = destination.len().min(self.bytes.len() - start);
            destination[..length].copy_from_slice(&self.bytes[start..start + length]);
            Ok(length)
        }

        fn worker_handle(&self) -> Self {
            Self::new(self.bytes.clone())
        }
    }

    /// The pre-buffering search (4 KiB peeks with a `signature - 1` byte
    /// carry), kept as the oracle for where `seek_at` stops.
    fn legacy_seek(bytes: &[u8], signature: &[u8], maximum: u64) -> (bool, u64) {
        let mut position = 0u64;
        let remaining = |position: u64| bytes.len() as u64 - position;
        let mut searched = 0u64;
        let mut tail = Vec::new();
        while searched < maximum && remaining(position) > 0 {
            let length = remaining(position).min(maximum - searched).min(4096) as usize;
            let chunk = &bytes[position as usize..position as usize + length];
            let mut search = tail.clone();
            search.extend_from_slice(chunk);
            if let Some(index) = search
                .windows(signature.len())
                .position(|window| window == signature)
            {
                return (true, position + index as u64 - tail.len() as u64);
            }
            let keep = (signature.len() - 1).min(search.len());
            tail = search[search.len() - keep..].to_vec();
            position += length as u64;
            searched += length as u64;
        }
        (false, position)
    }

    fn seek(bytes: &[u8], window: usize, signature: &[u8], maximum: u64) -> (bool, u64) {
        let source = CountingSource::new(bytes.to_vec());
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(
            &source,
            0,
            bytes.len() as u64,
            &bytes[..window.min(bytes.len())],
            &mut lookahead,
            &cancellation,
        );
        let found = seek_at(&mut reader, signature, maximum).expect("seek");
        (found, reader.bytes_read())
    }

    #[test]
    fn window_prefix_avoids_source_reads_until_exhausted() {
        let source = CountingSource::new(b"abcdefgh".to_vec());
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(&source, 0, 8, b"abcdef", &mut lookahead, &cancellation);

        assert_eq!(reader.read_vec(4).expect("prefix read"), b"abcd");
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
        assert_eq!(reader.read_vec(4).expect("spanning read"), b"efgh");
        assert_eq!(source.reads.load(Ordering::Relaxed), 1);
        assert!(reader.read_byte().is_err());
    }

    #[test]
    fn byte_reads_past_the_window_share_one_refill() {
        let bytes = (0..=255u8).cycle().take(10_000).collect::<Vec<_>>();
        let source = CountingSource::new(bytes.clone());
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(
            &source,
            0,
            bytes.len() as u64,
            &bytes[..100],
            &mut lookahead,
            &cancellation,
        );
        let read = (0..bytes.len())
            .map(|_| reader.read_byte().expect("byte"))
            .collect::<Vec<_>>();
        assert_eq!(read, bytes);
        assert_eq!(source.reads.load(Ordering::Relaxed), 1);
        assert_eq!(reader.bytes_read(), bytes.len() as u64);
    }

    #[test]
    fn reader_stays_inside_its_bound() {
        let bytes = vec![7u8; 64];
        let source = CountingSource::new(bytes.clone());
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(&source, 0, 10, &bytes, &mut lookahead, &cancellation);
        assert!(reader.read_vec(11).is_err());
        reader.discard_exact(9).expect("discard");
        assert_eq!(reader.read_byte().expect("last byte"), 7);
        assert!(reader.read_byte().is_err());
        assert!(reader.skip_past_byte(7).is_err());
    }

    #[test]
    fn skip_past_byte_and_consume_with_cross_the_window() {
        let mut bytes = vec![1u8; 300_000];
        bytes[250_000] = 0xff;
        let source = CountingSource::new(bytes.clone());
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(
            &source,
            0,
            bytes.len() as u64,
            &bytes[..1000],
            &mut lookahead,
            &cancellation,
        );
        reader.skip_past_byte(0xff).expect("skip");
        assert_eq!(reader.bytes_read(), 250_001);
        let mut consumed = 0usize;
        reader
            .consume_with(49_999, |slice| consumed += slice.len())
            .expect("consume");
        assert_eq!(consumed, 49_999);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn seek_at_matches_the_chunked_search_it_replaced() {
        let signature = b"%%EOF";
        let mut cases = Vec::new();
        for position in [0usize, 1, 4090, 4091, 4092, 4095, 4096, 8190, 9000, 12_286] {
            let mut bytes = vec![b'x'; 16_384];
            bytes[position..position + signature.len()].copy_from_slice(signature);
            cases.push(bytes);
        }
        cases.push(vec![b'x'; 10_000]);
        for bytes in &cases {
            for maximum in [3u64, 4096, 4100, 9000, u64::MAX] {
                for window in [0usize, 100, 4094, 8192, bytes.len()] {
                    assert_eq!(
                        seek(bytes, window, signature, maximum),
                        legacy_seek(bytes, signature, maximum),
                        "window={window} maximum={maximum}"
                    );
                }
            }
        }
    }
}
