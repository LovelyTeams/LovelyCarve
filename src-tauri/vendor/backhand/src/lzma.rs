//! Shared adaptive LZMA decompression
//!
//! Some vendor firmware images use LZMA streams that do not record their own
//! parameters. To read them, this module finds the parameters by trial: it
//! decompresses one block with each candidate until one succeeds. The result is
//! kept in an [`LzmaCache`], which lives on the image's `Kind`, so the search
//! is reused while the blocks share parameters. Decoding is pure Rust, requires
//! an end marker, consumes the complete block and bounds its output and history.

use no_std_io2::io::Read;
use std::sync::Mutex;

use crate::error::BackhandError;

/// The pull decoder currently tolerates an EOF during final normalisation.
/// Record that attempt so missing range-coder bytes cannot look authentic.
struct CheckedInput<'a> {
    remaining: &'a [u8],
    read_past_end: bool,
}

impl Read for CheckedInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = output.len().min(self.remaining.len());
        if count == 0 && !output.is_empty() {
            self.read_past_end = true;
        }
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        Ok(count)
    }
}

/// LZMA parameters that the stream itself does not record
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LzmaParams {
    lc: u32,
    lp: u32,
    pb: u32,
    dict_size: u32,
    offset: usize,
}

/// What the blocks of one image need in order to decompress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LzmaFormat {
    /// No block decompressed yet
    Unknown,
    /// The blocks are standard LZMA streams, which carry their own parameters
    Standard,
    /// The blocks need these parameters, found by search
    Adaptive(LzmaParams),
    /// No candidate decompressed the first block, so later blocks do not retry
    Undecodable,
}

/// What one image taught us about its LZMA blocks
///
/// A [`Kind`](crate::kind::Kind) shares one of these between readers, so blocks
/// with unchanged parameters avoid repeating the search.
pub(crate) struct LzmaCache(Mutex<LzmaFormat>);

impl LzmaCache {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(LzmaFormat::Unknown))
    }

    /// A poisoned lock reads as `Unknown`, which costs a repeated search but
    /// stays correct.
    fn get(&self) -> LzmaFormat {
        self.0
            .lock()
            .map(|format| *format)
            .unwrap_or(LzmaFormat::Unknown)
    }

    fn set(&self, format: LzmaFormat) {
        if let Ok(mut cached) = self.0.lock() {
            *cached = format;
        }
    }
}

/// A clone describes a different image, so it starts with no knowledge.
///
/// `Kind::with_magic` and the other builders clone the inner kind. Carrying the
/// parameters over would apply one image's findings to another.
impl Clone for LzmaCache {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for LzmaCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("LzmaCache").field(&self.get()).finish()
    }
}

impl Default for LzmaCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Largest block size SquashFS allows, so the most a block can decompress to
pub(crate) const MAX_BLOCK_SIZE: usize = 1 << 20;

/// Block size to assume when the caller does not know the image's real one
///
/// Every search attempt allocates a buffer of this size, and the allocation is
/// zeroed, so a large value here is costly. 128 KiB is the usual SquashFS block
/// size. The reader passes the real block size and never uses this.
pub(crate) const DEFAULT_BLOCK_SIZE: usize = 1 << 17;

/// Parameter grid supported by the upstream adaptive decoder (lc + lp <= 4).
const LZMA_MAX_LC_PLUS_LP: u32 = 4;
const LZMA_MAX_LC: u32 = 4;
const LZMA_MAX_LP: u32 = 4;
const LZMA_MAX_PB: u32 = 4;
const LZMA_MAX_OFFSET: usize = 10;

/// The parameters that `lzma` uses unless told otherwise. Most images match
/// these, so try them before the search.
const LZMA_DEFAULT_PARAMS: LzmaParams = LzmaParams {
    lc: 3,
    lp: 0,
    pb: 2,
    dict_size: DICT_SIZE_DEFAULT,
    offset: 0,
};

/// Dictionary size that sasquatch uses. The value only sizes the decoder
/// window, so one that is too large still decompresses correctly.
const DICT_SIZE_DEFAULT: u32 = 0x800000;

/// Decompress one LZMA block, finding the parameters if they are not yet known
///
/// `max_out` is the largest size the block can decompress to: `block_size` for
/// file data, `METADATA_MAXSIZE` for metadata. Each attempt allocates a buffer
/// of this size, so an accurate value matters for both memory and speed.
pub(crate) fn decompress_adaptive(
    bytes: &[u8],
    out: &mut Vec<u8>,
    cache: &LzmaCache,
    max_out: usize,
) -> Result<(), BackhandError> {
    if max_out == 0 || max_out > MAX_BLOCK_SIZE {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    if bytes.is_empty() {
        return Ok(());
    }

    match cache.get() {
        LzmaFormat::Adaptive(params) => {
            if let Ok(result) = try_lzma_with_params(bytes, params, max_out) {
                out.extend_from_slice(&result);
                return Ok(());
            }
            trace!("cached LZMA parameters failed, searching again");
        }
        LzmaFormat::Standard => {
            if decompress_standard(bytes, out, max_out).is_ok() {
                return Ok(());
            }
            trace!("cached standard LZMA failed, searching again");
        }
        // A block that no candidate decompressed means the image does not match
        // this kind. Searching again for every later block only wastes time.
        LzmaFormat::Undecodable => {
            return Err(BackhandError::UnsupportedCompression(err_text!(
                "no LZMA parameters decompress this image"
            )));
        }
        LzmaFormat::Unknown => {}
    }

    // Standard LZMA is one attempt, so try it before the search. Some images,
    // such as the fragments in le_v3_0_lzma_swap, use it for every block.
    if decompress_standard(bytes, out, max_out).is_ok() {
        trace!("standard LZMA decompressed the block");
        cache.set(LzmaFormat::Standard);
        return Ok(());
    }

    if let Some((result, params)) = search_lzma_params(bytes, max_out) {
        trace!("found LZMA parameters {:?}", params);
        cache.set(LzmaFormat::Adaptive(params));
        out.extend_from_slice(&result);
        return Ok(());
    }

    cache.set(LzmaFormat::Undecodable);
    Err(BackhandError::UnsupportedCompression(err_text!(
        "no LZMA parameters decompress this image"
    )))
}

/// Decompress a standard LZMA stream, which records its own parameters
///
/// Supports the 13-byte alone header and the five-byte SquashFS prefix.
/// Decode to an end marker, then verify any declared length. A failed attempt
/// never publishes partial output or clears bytes already in the caller's buffer.
pub(crate) fn decompress_standard(
    bytes: &[u8],
    out: &mut Vec<u8>,
    max_out: usize,
) -> Result<(), BackhandError> {
    if max_out == 0 || max_out > MAX_BLOCK_SIZE || bytes.len() < 5 || bytes[0] > 224 {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    let dict_size = u32::from_le_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?,
    );
    if dict_size > DICT_SIZE_DEFAULT {
        return Err(BackhandError::UnsupportedCompression(err_text!(
            "LZMA dictionary exceeds 8 MiB"
        )));
    }
    let properties = bytes[0];
    let mut params = LzmaParams {
        lc: u32::from(properties % 9),
        lp: u32::from((properties / 9) % 5),
        pb: u32::from(properties / 45),
        dict_size,
        offset: 13,
    };
    if let Some(length_bytes) = bytes.get(5..13) {
        let declared = u64::from_le_bytes(
            length_bytes
                .try_into()
                .map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?,
        );
        if (declared == u64::MAX || declared <= max_out as u64)
            && let Ok(decoded) = try_lzma_with_params(bytes, params, max_out)
            && (declared == u64::MAX || declared == decoded.len() as u64)
        {
            out.extend_from_slice(&decoded);
            return Ok(());
        }
    }
    params.offset = 5;
    let decoded = try_lzma_with_params(bytes, params, max_out)?;
    out.extend_from_slice(&decoded);
    Ok(())
}

/// Decompress with one set of parameters
fn try_lzma_with_params(
    bytes: &[u8],
    params: LzmaParams,
    max_out: usize,
) -> Result<Vec<u8>, BackhandError> {
    if params.offset >= bytes.len() {
        return Err(BackhandError::UnsupportedCompression(err_text!(
            "invalid offset"
        )));
    }

    // No back reference can reach further than this block's output. Capping
    // the history avoids allocating 8 MiB for each small metadata candidate.
    let dict_size = params.dict_size.max(4096).min(max_out.max(4096) as u32);
    let mut decoder =
        lzma_rust2::LzmaStream::new(u64::MAX, params.lc, params.lp, params.pb, dict_size, None)?;
    let input = &bytes[params.offset..];
    let mut decoded = vec![0; max_out + 1];
    let result = decoder.process(input, &mut decoded, lzma_rust2::Action::Finish)?;
    if result.status != lzma_rust2::Status::StreamEnd
        || result.bytes_produced == 0
        || result.bytes_produced > max_out
        || result.bytes_consumed != input.len()
        || !decoder.unused_input().is_empty()
    {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    decoded.truncate(result.bytes_produced);
    // The push decoder validates the final range-code state but may accept
    // implicit zero bytes in its carry buffer. The pull decoder consumes source
    // bytes directly; audit its final normalisation as an independent boundary
    // check. Both must consume the entire block and produce identical bytes.
    drop(decoder);
    let mut reader = lzma_rust2::LzmaReader::new(
        CheckedInput {
            remaining: input,
            read_past_end: false,
        },
        u64::MAX,
        params.lc,
        params.lp,
        params.pb,
        dict_size,
        None,
    )?;
    let mut verified = Vec::new();
    reader
        .by_ref()
        .take(max_out as u64 + 1)
        .read_to_end(&mut verified)?;
    let source = reader.into_inner();
    if source.read_past_end || !source.remaining.is_empty() || verified != decoded {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    Ok(decoded)
}

/// Find parameters that decompress this block
///
/// The search is ordered by how likely each candidate is, and skips candidates
/// that the LZMA format rules out. All candidate dictionaries are at least as
/// large as a permitted block, so changing their size cannot change valid output.
fn search_lzma_params(bytes: &[u8], max_out: usize) -> Option<(Vec<u8>, LzmaParams)> {
    trace!("searching for LZMA parameters");

    // Most images use the default parameters, so spend one attempt on them
    // before walking the grid.
    if let Ok(result) = try_lzma_with_params(bytes, LZMA_DEFAULT_PARAMS, max_out) {
        return Some((result, LZMA_DEFAULT_PARAMS));
    }

    for offset in 0..=LZMA_MAX_OFFSET {
        // The range coder writes a zero as the first byte after the header, so
        // a nonzero byte rules out this offset without a decompression attempt.
        match bytes.get(offset) {
            Some(0) => {}
            _ => continue,
        }

        for lc in 0..=LZMA_MAX_LC {
            for lp in 0..=LZMA_MAX_LP {
                // Match the upstream adaptive decoder's supported grid.
                if lc + lp > LZMA_MAX_LC_PLUS_LP {
                    continue;
                }

                for pb in 0..=LZMA_MAX_PB {
                    let params = LzmaParams {
                        lc,
                        lp,
                        pb,
                        dict_size: DICT_SIZE_DEFAULT,
                        offset,
                    };
                    if params == LZMA_DEFAULT_PARAMS {
                        continue;
                    }

                    if let Ok(result) = try_lzma_with_params(bytes, params, max_out) {
                        return Some((result, params));
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
#[path = "lzma_tests.rs"]
mod fixture_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Metadata blocks are the smallest thing the reader decompresses, so use
    /// that bound in tests.
    const MAX_OUT: usize = 0x2000;

    #[test]
    fn empty_input_writes_nothing() {
        let cache = LzmaCache::new();
        let mut out = Vec::new();
        decompress_adaptive(&[], &mut out, &cache, MAX_OUT).unwrap();
        assert!(out.is_empty());
        // An empty block must not teach the cache anything.
        assert_eq!(cache.get(), LzmaFormat::Unknown);
    }

    #[test]
    fn garbage_input_reports_unsupported() {
        let cache = LzmaCache::new();
        let mut out = Vec::new();
        let error = decompress_adaptive(&[0xff; 64], &mut out, &cache, MAX_OUT).unwrap_err();
        assert!(matches!(error, BackhandError::UnsupportedCompression(_)));
        assert!(out.is_empty());
    }

    #[test]
    fn undecodable_block_is_not_searched_twice() {
        let cache = LzmaCache::new();
        let mut out = Vec::new();

        assert!(decompress_adaptive(&[0xff; 64], &mut out, &cache, MAX_OUT).is_err());
        assert_eq!(cache.get(), LzmaFormat::Undecodable);

        // The second call must fail from the cache, without another search.
        let error = decompress_adaptive(&[0xff; 64], &mut out, &cache, MAX_OUT).unwrap_err();
        assert!(matches!(error, BackhandError::UnsupportedCompression(_)));
    }

    #[test]
    fn caches_are_independent() {
        let first = LzmaCache::new();
        let second = LzmaCache::new();

        first.set(LzmaFormat::Standard);

        assert_eq!(first.get(), LzmaFormat::Standard);
        assert_eq!(second.get(), LzmaFormat::Unknown);
    }

    #[test]
    fn clone_starts_empty() {
        let cache = LzmaCache::new();
        cache.set(LzmaFormat::Adaptive(LZMA_DEFAULT_PARAMS));

        // A cloned Kind describes a different image and must search again.
        assert_eq!(cache.clone().get(), LzmaFormat::Unknown);
        assert_eq!(cache.get(), LzmaFormat::Adaptive(LZMA_DEFAULT_PARAMS));
    }

    #[test]
    fn search_skips_offsets_that_cannot_start_a_stream() {
        // No byte is zero, so every offset is rejected before any attempt.
        assert!(search_lzma_params(&[0xab; 32], MAX_OUT).is_none());
    }

    #[test]
    fn search_ends_when_every_offset_looks_valid() {
        // All zeroes passes the offset guard, so this walks the whole grid and
        // shows the search ends rather than running on.
        assert!(search_lzma_params(&[0x00; 128], MAX_OUT).is_none());
    }

    #[test]
    fn truncated_input_does_not_panic() {
        // A block shorter than the offsets the search tries must not index out
        // of bounds.
        for len in 0..LZMA_MAX_OFFSET + 2 {
            let cache = LzmaCache::new();
            let mut out = Vec::new();
            let result = decompress_adaptive(&vec![0x00; len], &mut out, &cache, MAX_OUT);
            assert_eq!(result.is_ok(), len == 0);
        }
    }
}
