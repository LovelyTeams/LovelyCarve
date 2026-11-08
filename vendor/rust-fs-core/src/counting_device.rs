//! A device that counts what a driver asks of it.
//!
//! # Why this is in the shared crate rather than a test file
//!
//! Every driver in this family is about to be measured and then made
//! faster, and a measurement is only worth having if the drivers can be
//! compared against each other and against themselves later. Two
//! drivers each counting reads with their own wrapper would produce two
//! numbers that look alike and are not: one might count a read of a
//! whole extent as one, the other as one per block, and nothing in
//! either number would say so.
//!
//! One instrument, in the crate every driver already depends on. It
//! wraps a [`BlockRead`] and forwards every call, so a driver mounted
//! on it behaves exactly as it would otherwise.
//!
//! # What the numbers mean
//!
//! - **reads** — calls to [`BlockRead::read_at`]. This is the number a
//!   cache moves: a metadata block read twice is two reads here and one
//!   after a cache is put underneath.
//! - **bytes** — the size of every buffer those calls asked to have
//!   filled. This is not the same number as the read count, and that is
//!   why both are here: a driver that reads a 4 KiB block to look at
//!   8 bytes of it is one read either way, and only the byte count
//!   shows the 4 KiB it asked the device for to use 8.
//!
//! Both are worth having. A change that halves reads and doubles bytes
//! is a readahead that guessed wrong, and one number alone would call
//! it a win.
//!
//! # Both numbers are what was asked for, not what moved
//!
//! `bytes` is the buffer a call presented, added before the call is
//! forwarded and never adjusted afterwards, so a read the device
//! refuses contributes its whole buffer. That is the same rule as
//! `reads`, for the same reason: a driver looping on an out-of-range
//! offset is exactly the shape these numbers exist to make visible, and
//! a counter that sat still through it would hide the loop.
//!
//! It is worth being plain that this is the only rule available, rather
//! than a convenience. **Bytes-moved is not reachable through
//! [`BlockRead`] at all**: [`BlockRead::read_at`] returns `Result<()>`
//! and carries no transfer count, so the only place a real figure ever
//! appears is `got` inside [`crate::Error::ShortRead`] — off the
//! success path, and absent from the other errors an over-read can
//! produce. Counting only on `Ok` would not recover it either, because
//! a refused read is not reliably a transfer of nothing: a
//! [`crate::FileDevice`] copies the readable prefix into the buffer
//! before reporting the shortfall, so a 64-byte request against a
//! 16-byte file really moves 16 bytes, while the same request against
//! an in-memory device moves none. Counting on `Ok` reports 0 for both;
//! this counter reports 64 for both.
//!
//! Reporting the request is therefore the one answer that does not
//! depend on which device happens to be underneath, which is what makes
//! two drivers' numbers comparable — the whole point of the module. The
//! price is that these counters are not a transfer total for a run with
//! failed reads in it, and should not be read as one.

use crate::block::BlockRead;
use crate::error::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Wraps a device and counts the reads passing through it.
///
/// The counters are atomic and the type is `Sync`, so a driver reading
/// from several threads is measured correctly rather than approximately.
pub struct CountingDevice {
    inner: Arc<dyn BlockRead>,
    reads: AtomicU64,
    bytes: AtomicU64,
}

impl CountingDevice {
    /// Wrap `inner`, counting from zero.
    pub fn new(inner: Arc<dyn BlockRead>) -> Self {
        CountingDevice {
            inner,
            reads: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// How many times the driver called `read_at`.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// How many bytes those calls asked for, including the buffers of
    /// reads the device refused.
    ///
    /// This is the request, not the transfer. See the module header for
    /// why bytes-moved is not reachable through [`BlockRead`] and why
    /// counting the request is what makes two drivers comparable.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Start counting again from zero.
    ///
    /// A mount reads a superblock and headers before the work being
    /// measured begins, and counting that in makes a small operation
    /// look like a large one. Reset after mounting, measure the
    /// operation, read the counters.
    pub fn reset(&self) {
        self.reads.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
    }
}

impl BlockRead for CountingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.inner.read_at(offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.inner.size_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_device::Bytes;

    fn device(len: usize) -> Arc<CountingDevice> {
        Arc::new(CountingDevice::new(Arc::new(Bytes::new(vec![7u8; len]))))
    }

    /// One call is one read, whatever it asked for, and the bytes are
    /// what the buffer wanted rather than what the device holds.
    #[test]
    fn it_counts_calls_and_the_bytes_they_asked_for() {
        let dev = device(4096);
        let mut small = [0u8; 8];
        let mut block = [0u8; 512];

        dev.read_at(0, &mut small).expect("read");
        assert_eq!((dev.reads(), dev.bytes()), (1, 8));

        dev.read_at(1024, &mut block).expect("read");
        assert_eq!(
            (dev.reads(), dev.bytes()),
            (2, 520),
            "two calls, and the bytes are the sum of both buffers"
        );
    }

    /// A read the device refuses is still a read the driver made.
    ///
    /// The point of the count is what the driver ASKED for, so a failed
    /// call belongs in it: a driver looping on an out-of-range offset is
    /// exactly the shape this is here to make visible.
    #[test]
    fn a_failed_read_still_counts() {
        let dev = device(16);
        let mut buf = [0u8; 64];
        assert!(dev.read_at(0, &mut buf).is_err(), "past the end");
        assert_eq!(dev.reads(), 1, "the driver asked, so it counts");
    }

    /// The byte counter follows the same rule as the read counter, and
    /// nothing was pinning that.
    ///
    /// A refused read leaves three candidate answers a reader of the
    /// module might expect, and this separates them. The device holds
    /// 16 bytes, the call asks for 64, and the in-memory device copies
    /// nothing before refusing:
    ///
    /// - 0, if the counter charged only successful reads;
    /// - 16, if it charged what the error says was available;
    /// - 64, the buffer presented — which is what it does.
    ///
    /// The read count on a failed read has a test above; the byte count
    /// had none, and that is how the module header came to describe a
    /// counter of bytes moved while the code counted bytes requested.
    #[test]
    fn bytes_counts_what_was_asked_for_including_a_failed_read() {
        let dev = device(16);
        let mut buf = [0u8; 64];

        assert!(dev.read_at(0, &mut buf).is_err(), "past the end");
        assert_eq!(
            dev.bytes(),
            64,
            "the whole buffer the driver presented, not the 16 bytes \
             available and not the 0 bytes delivered"
        );
        assert_eq!(
            buf, [0u8; 64],
            "this device refused without copying, so 64 bytes were \
             charged for a transfer of none"
        );
    }

    /// And the answer does not change when the device *did* move bytes,
    /// which is the property that makes two drivers comparable.
    ///
    /// A [`crate::FileDevice`] copies the readable prefix into the
    /// buffer before reporting the shortfall, so this same 64-byte
    /// request against a 16-byte file genuinely transfers 16 bytes
    /// where the in-memory device above transferred none. Both are
    /// charged 64. A counter of bytes moved would have to report 16
    /// here and 0 there for identical driver behaviour, which is the
    /// "two numbers that look alike and are not" failure this module
    /// exists to prevent.
    #[test]
    fn bytes_is_the_request_even_when_the_device_moved_a_prefix() {
        use std::sync::atomic::AtomicU64;

        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fs_core_counting_prefix_{}_{}.bin",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, [7u8; 16]).expect("write the fixture");

        let file = crate::FileDevice::open(&path).expect("open the fixture");
        assert_eq!(file.size_bytes(), 16, "the fixture is the size it claims");

        let dev = CountingDevice::new(Arc::new(file));
        let mut buf = [0u8; 64];
        let err = dev.read_at(0, &mut buf).expect_err("past the end");

        // Read everything out before asserting, so a failure does not
        // leave the fixture behind.
        let (reads, bytes) = (dev.reads(), dev.bytes());
        let moved = buf.iter().filter(|b| **b == 7).count();
        drop(dev);
        std::fs::remove_file(&path).expect("remove the fixture");

        match err {
            crate::Error::ShortRead { want, got, .. } => {
                assert_eq!((want, got), (64, 16), "asked 64, got the 16 there were");
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }
        assert_eq!(moved, 16, "the file device really did copy its prefix");
        assert_eq!(
            (reads, bytes),
            (1, 64),
            "charged the request, exactly as the in-memory device was"
        );
    }

    /// Resetting drops the mount's own reads, which is the whole reason
    /// it exists: an operation measured with them included is measured
    /// against a constant that has nothing to do with it.
    #[test]
    fn resetting_starts_the_measurement_where_the_work_does() {
        let dev = device(4096);
        let mut buf = [0u8; 64];
        dev.read_at(0, &mut buf).expect("the mount's own reads");
        dev.reset();
        assert_eq!((dev.reads(), dev.bytes()), (0, 0));

        dev.read_at(64, &mut buf).expect("the work being measured");
        assert_eq!((dev.reads(), dev.bytes()), (1, 64));
    }
}
