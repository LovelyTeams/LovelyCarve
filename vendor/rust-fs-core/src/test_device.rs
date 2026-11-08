//! In-memory devices for this crate's own tests.
//!
//! # What is shared, and what deliberately is not
//!
//! Four modules each declared a byte-buffer device — `stream::Bytes`,
//! `slice::Bytes`, `slice::RwBytes`, `readonly::WritableBytes` — and
//! three of the four were identical.
//!
//! The fourth was not, and the difference mattered: `WritableBytes`
//! read **without a bounds check**, so a read past the end panicked
//! where the other three returned [`Error::ShortRead`]. That is the
//! kind of divergence a consolidation has to find rather than flatten,
//! and it is why the eleven doubles were worth checking one at a time
//! instead of merging on sight.
//!
//! `ShortRead` is the right answer for all of them. A device that
//! panics on a past-end read turns a caller's arithmetic bug into a
//! crash in the test harness rather than an error the caller can be
//! asserted against.
//!
//! **Three doubles stay where they are**, because each exists to
//! misbehave in one specific way and a shared device is the opposite of
//! that: `stream::AlwaysFails` (every read errors), `ffi::Panicking`
//! (every method panics) and `tests/cache.rs::CountingDev` (counts
//! reads). Those are not duplicates; they are the point of their tests.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::sync::Mutex;

/// A read-only byte buffer.
pub(crate) struct Bytes(pub Mutex<Vec<u8>>);

impl Bytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Bytes(Mutex::new(bytes))
    }
}

impl BlockRead for Bytes {
    /// A read past the end is [`Error::ShortRead`] with `got: 0` — not
    /// a zero-fill, not a panic, and not a count of what was there. It
    /// refuses before copying, so nothing reaches the buffer and
    /// nothing is what it reports.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.0.lock().unwrap();
        read_into(&b, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
}

/// A writable byte buffer, with the same read behaviour.
pub(crate) struct RwBytes(pub Mutex<Vec<u8>>);

impl RwBytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        RwBytes(Mutex::new(bytes))
    }
}

impl BlockRead for RwBytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.0.lock().unwrap();
        read_into(&b, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
}

impl BlockDevice for RwBytes {
    /// A write past the end grows nothing and transfers nothing — a
    /// device is not a `Vec` — so it reports `got: 0` rather than what
    /// it could have taken.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.0.lock().unwrap();
        // The same arithmetic as the read, from the same place: this
        // pair of lines carried the identical overflow.
        let (start, end) = range_within(b.len(), offset, buf.len())?;
        b[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn is_writable(&self) -> bool {
        true
    }
}

/// Where `offset` lands in a buffer of `len` bytes — or the
/// [`Error::ShortRead`] that says the range does not fit, which always
/// reports `got: 0` because nothing is copied on that path.
///
/// ONE PLACE DOES THE ARITHMETIC, because the arithmetic is what went
/// wrong. `offset as usize` followed by `start + buf.len()` computed
/// the end BEFORE the bounds check meant to catch a past-end range, so
/// `u64::MAX` panicked with "attempt to add with overflow" in a debug
/// build — which is every `cargo test` run. This module exists because
/// four hand-rolled doubles disagreed about exactly this, one of them
/// by panicking; the consolidated version still panicked, for a larger
/// past-end than the one the header discusses.
///
/// The width is the other half of it. Everything here is `u64`, and
/// the narrowing to `usize` happens only *after* the bounds check has
/// proved the range fits inside `len` — which is a `usize` — rather
/// than before it, where a cast would turn a wild offset into a
/// plausible one instead of refusing it.
pub(crate) fn range_within(len: usize, offset: u64, want: usize) -> Result<(usize, usize)> {
    let len64 = len as u64;
    let short = || Error::ShortRead {
        offset,
        want,
        // ZERO, ALWAYS — AND NOT "WHAT WAS AVAILABLE".
        //
        // `got` counts the bytes placed in the CALLER'S BUFFER. This
        // refuses before `copy_from_slice` runs, on every path that
        // builds this error, so the buffer is provably untouched and
        // the count is provably nothing.
        //
        // It reported `len - offset`, which is a different number
        // whenever the read starts inside the buffer and only overruns
        // at the far end: 16 bytes from offset 0 of an 8-byte double
        // claimed `got: 8` for a read that copied none. That is
        // `FileDevice`'s answer, not this one's. A file really does
        // hand back the readable prefix and report its length; the
        // slice adapters refuse outright and report 0; `error.rs`
        // spells out that `got: 0` means nothing was transferred and
        // says nothing about what was there. This double refuses, so
        // it is the second kind.
        got: 0,
    };
    let end = offset.checked_add(want as u64).ok_or_else(short)?;
    if end > len64 {
        return Err(short());
    }
    // `offset <= end <= len`, and `len` is a `usize`, so both of these
    // narrowings are exact.
    Ok((offset as usize, end as usize))
}

/// The one read implementation both share.
fn read_into(b: &[u8], offset: u64, buf: &mut [u8]) -> Result<()> {
    let (start, end) = range_within(b.len(), offset, buf.len())?;
    buf.copy_from_slice(&b[start..end]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WILD OFFSET IS A SHORT READ, NOT A CRASH -- the same rule as
    /// the test above, at an offset large enough that the arithmetic
    /// used to panic before the bounds check could refuse it.
    ///
    /// This is the device every bounds test in `slice.rs`,
    /// `stream.rs`, `readonly.rs` and `caching_device.rs` is asserted
    /// against, and the wild-offset guards in `caching_device.rs` and
    /// `slice.rs` cannot be tested with a device that dies on the
    /// input. The failure mode was "the test you needed to write is
    /// the one this device cannot support".
    #[test]
    fn a_read_at_a_wild_offset_is_a_short_read_not_a_panic() {
        let dev = Bytes::new(vec![0xAB; 8]);
        for offset in [u64::MAX, u64::MAX - 7, 1 << 40, 9] {
            let mut buf = [0u8; 8];
            match dev.read_at(offset, &mut buf).expect_err("past the end") {
                Error::ShortRead {
                    offset: o,
                    want,
                    got,
                } => {
                    assert_eq!((o, want), (offset, 8));
                    assert_eq!(got, 0, "nothing is available at {offset}");
                }
                other => panic!("expected ShortRead at {offset}, got {other:?}"),
            }
            assert_eq!(buf, [0u8; 8], "a refused read leaves the buffer alone");
        }
    }

    /// And the write, which carried the identical pair of lines.
    #[test]
    fn a_write_at_a_wild_offset_is_refused_rather_than_panicking() {
        let dev = RwBytes::new(vec![0u8; 8]);
        for offset in [u64::MAX, u64::MAX - 7, 1 << 40, 9] {
            match dev.write_at(offset, &[1u8; 8]).expect_err("past the end") {
                Error::ShortRead {
                    offset: o, want, ..
                } => {
                    assert_eq!((o, want), (offset, 8))
                }
                other => panic!("expected ShortRead at {offset}, got {other:?}"),
            }
        }
        let mut buf = [0u8; 8];
        dev.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 8], "no refused write touched the buffer");
    }

    /// An offset that DOES fit, of a length that would overflow when
    /// added to it. The bounds check alone cannot catch this -- the sum
    /// is what wraps -- so it is the case `checked_add` is there for
    /// rather than `try_from`.
    #[test]
    fn a_length_that_overflows_when_added_to_the_offset_is_refused() {
        let dev = Bytes::new(vec![0xAB; 8]);
        let mut buf = vec![0u8; 16];
        let offset = (usize::MAX - 8) as u64;
        match dev.read_at(offset, &mut buf).expect_err("cannot fit") {
            Error::ShortRead { want, got, .. } => {
                assert_eq!(want, 16);
                assert_eq!(got, 0);
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }
    }

    /// The behaviour the four copies were supposed to share, and one
    /// did not.
    ///
    /// THE `got` HERE WAS 8, AND 8 WAS WRONG. This is the one shape in
    /// the suite where "bytes available" and "bytes copied" differ —
    /// the read starts inside the buffer and only overruns at the far
    /// end — so it was the one assertion pinning the defect rather
    /// than passing over it. Every other case uses an offset already
    /// past the end, where `len - offset` saturates to 0 and the two
    /// numbers agree by accident.
    ///
    /// The line below asserts the buffer was not touched, in the same
    /// test, two lines apart. `got: 8` and "nothing was copied" cannot
    /// both be true.
    #[test]
    fn a_read_past_the_end_is_a_short_read_not_a_panic() {
        let dev = Bytes::new(vec![0xAB; 8]);
        let mut buf = [0u8; 16];
        match dev.read_at(0, &mut buf).expect_err("past the end") {
            Error::ShortRead { offset, want, got } => assert_eq!((offset, want, got), (0, 16, 0)),
            other => panic!("expected ShortRead, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 16], "a refused read leaves the buffer alone");
    }

    #[test]
    fn the_writable_one_reads_the_same_way() {
        let dev = RwBytes::new(vec![0xAB; 8]);
        let mut buf = [0u8; 16];
        assert!(dev.read_at(0, &mut buf).is_err());
        assert!(dev.is_writable());
    }

    #[test]
    fn a_write_past_the_end_is_refused_rather_than_growing_the_buffer() {
        let dev = RwBytes::new(vec![0u8; 4]);
        assert!(dev.write_at(2, &[1, 2, 3, 4]).is_err());
        assert_eq!(dev.size_bytes(), 4, "the device did not grow");
    }

    #[test]
    fn reads_and_writes_inside_the_buffer_round_trip() {
        let dev = RwBytes::new(vec![0u8; 16]);
        dev.write_at(4, &[1, 2, 3, 4]).unwrap();
        let mut buf = [0u8; 4];
        dev.read_at(4, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4]);
    }
}
