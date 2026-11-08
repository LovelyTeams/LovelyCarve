//! Slice adapters — view a byte sub-range of any `BlockRead` as its own
//! device. Useful any time you want to feed a fragment of a larger
//! device to a consumer that expects a whole block source — partition
//! probes, image-file extents, mmap-style views, fuzzer harnesses.
//!
//! Three variants:
//!
//! - [`SliceReader`] borrows the parent, lifetime-tied. Cheaper when the
//!   parent outlives the slice and you can express that statically.
//! - [`OwnedSlice`] holds an `Arc` to the parent. Use when the parent's
//!   lifetime can't be expressed in a borrow (FFI handles, slice handed
//!   across thread boundaries, etc.).
//! - [`OwnedRwSlice`] holds an `Arc<dyn BlockDevice>` and propagates
//!   writes to the parent.
//!
//! The first two are strictly read-only: the default `Err(ReadOnly)`
//! write path from [`BlockDevice`] applies.
//!
//! # Which error an out-of-range request gets
//!
//! All three share one range check — `SliceGeometry::rebase` — and it
//! answers in two different currencies depending on the direction of the
//! request:
//!
//! | request outside `[0, length)` | error |
//! |---|---|
//! | read  | [`Error::ShortRead`] with `got: 0` |
//! | write | [`Error::OutOfBounds`] |
//!
//! The asymmetry is deliberate. A slice exists to be substitutable for a
//! real device of size `length`, and a real device — [`FileDevice`] —
//! answers a read that begins at or past its end with exactly
//! `ShortRead { offset, want, got: 0 }`. A slice that answered
//! `OutOfBounds` would be distinguishable from the thing it stands in
//! for, and every caller that already handles end-of-device would need a
//! second arm to cope with slices. Writes have no partial-write variant
//! to stay consistent with, and a caller that overran a write needs the
//! device size in order to clamp and retry — which is what
//! [`Error::OutOfBounds`] carries and [`Error::ShortRead`] does not.
//!
//! The match is on the variant, not on `got`. A slice refuses an
//! out-of-range read before it touches the parent, so it reports `got: 0`
//! and leaves the buffer untouched — including for a read that begins
//! inside the slice and runs off its end, where [`FileDevice`] would have
//! copied the readable prefix and reported its length. `got` counts bytes
//! actually delivered, and a slice delivers none.
//!
//! This governs the slice's own range only. A request that *is* inside
//! `[0, length)` is forwarded to the parent, and whatever the parent says
//! about it — including [`Error::OutOfBounds`] from a container reader
//! that knows its virtual size — comes back unchanged.
//!
//! # A slice cannot report more device than its parent holds
//!
//! All three constructors ask the parent its size and CLAMP `length` to
//! what is actually there — see [`window_on_parent`] for the rule and
//! why it clamps rather than refuses. So `size_bytes()` is the truth
//! about how much is readable, not a number the caller asserted.
//!
//! That is load-bearing rather than tidy. `size_bytes()` is "used for
//! bounds checks" ([`BlockRead::size_bytes`]), and a driver mounted on a
//! slice sizes its own structures from it. A slice that claimed a
//! megabyte over a hundred-byte parent kept both promises of the section
//! above and broke them in the same breath: a read inside the *declared*
//! length but past the *real* end was forwarded, and the parent — being
//! a real device — answered with a `ShortRead` carrying its own absolute
//! offset and a non-zero `got`, having already copied the readable prefix
//! into the caller's buffer. Substitutable for a real device of size
//! `length` is exactly what that is not.
//!
//! A slice whose `start` is at or past the parent's end has nothing
//! behind it at all. The constructors are infallible and give it
//! `length = 0`, which behaves as a zero-byte device does: every read is
//! `ShortRead { got: 0 }` and every write is `OutOfBounds { size: 0 }`.
//! The C ABI, which can afford to be fallible, refuses it instead —
//! `fs_core_device_slice_ro`/`_rw` return NULL with a message.
//!
//! [`FileDevice`]: crate::FileDevice

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::sync::Arc;

/// How much of the window `[start, start + length)` is actually on a
/// parent of `parent_size` bytes, or `None` when the window begins at or
/// past the parent's end and there is nothing on it to slice.
///
/// A slice's geometry comes from a partition table, and a partition
/// table comes off the disk, so a window that claims to start or end
/// past the device is an ordinary thing to be handed -- and not only
/// from a hostile image. A `dd` of the first N gigabytes of a disk, or a
/// table left stale after the volume was shrunk, both produce a last
/// partition that runs off the end.
///
/// So the length is CLAMPED rather than the slice refused. Refusing it
/// takes away the one thing someone with a truncated image wants, which
/// is to read what is still there. What must not happen is the slice
/// reporting more device than exists, because the driver stacked on it
/// sizes its own structures from that answer.
///
/// Clamping also closes the arithmetic hole in the slices' shared
/// rebasing step as a side effect, and closes it at the root.
/// `start + offset + len`
/// can only leave a `u64` if `start + length` does, and `length` is now
/// at most `parent_size - start`.
pub fn window_on_parent(parent_size: u64, start: u64, length: u64) -> Option<u64> {
    if start >= parent_size {
        return None;
    }
    Some(length.min(parent_size - start))
}

/// Where a slice sits on its parent, and the one bounds rule the three
/// slice types share.
///
/// The public slice types differ only in how they hold the parent and
/// whether writes propagate. The geometry, the range check and the choice
/// of error are identical across all of them, so they live here — one
/// definition to read, one place to change.
#[derive(Clone, Copy)]
struct SliceGeometry {
    start: u64,
    length: u64,
}

impl SliceGeometry {
    /// `length` is CLAMPED to what `parent_size` can back — see
    /// [`window_on_parent`]. A window beginning at or past the parent's
    /// end becomes a zero-length slice, which is what it is.
    ///
    /// Taking the parent's size here rather than on each read is
    /// deliberate: [`BlockRead::size_bytes`] must not change for the
    /// life of a device, so one call at construction is the whole
    /// answer, and `length` is then a fact instead of a claim.
    fn new(parent_size: u64, start: u64, length: u64) -> Self {
        Self {
            start,
            length: window_on_parent(parent_size, start, length).unwrap_or(0),
        }
    }

    /// Parent offset corresponding to `offset`, or `None` when
    /// `[offset, offset + len)` is not wholly inside `[0, length)`.
    ///
    /// This asks the parent nothing — the parent was asked once, in
    /// [`SliceGeometry::new`], and `length` is its answer. The comment
    /// here used to claim a second check "when the rebased offset would
    /// not fit on the parent at all", which no code performed and no
    /// parent was in scope to perform: it was a `u64` overflow guard
    /// being described as a bounds check against the device.
    ///
    /// Both additions stay checked, `start + offset` included. That one
    /// used to be deliberate, on the argument that a slice built with a
    /// nonsense `start` would "overflow here rather than quietly
    /// reading some other part of the parent" -- which holds only while
    /// `overflow-checks` is on, and it is off in the release profile
    /// these crates ship. In release the addition wrapped, and the wrap
    /// did precisely the thing the argument said it avoided: a slice
    /// starting at 2^63 and 5000 bytes long returned `Ok` and the
    /// parent's bytes from offset 5000.
    ///
    /// Both are now unreachable through the public API, because the
    /// clamp makes `start + length <= parent_size` and every accepted
    /// `offset + len` is at most `length`. They are kept as the last
    /// line of defence for a parent that violates the `size_bytes`
    /// stability contract and shrinks under a live slice; the guard that
    /// is actually load-bearing, and tested, is the clamp.
    fn rebase(&self, offset: u64, len: u64) -> Option<u64> {
        let end = offset.checked_add(len)?;
        if end > self.length {
            return None;
        }
        self.start.checked_add(offset)
    }

    /// Bounds-check a read and rebase it onto the parent.
    ///
    /// Out of range is [`Error::ShortRead`] with `got: 0` — the same
    /// answer a real device of size `length` gives for a read beginning
    /// at or past its end. See the module docs for why.
    fn rebase_read(&self, offset: u64, len: usize) -> Result<u64> {
        self.rebase(offset, len as u64).ok_or(Error::ShortRead {
            offset,
            want: len,
            got: 0,
        })
    }

    /// Bounds-check a write and rebase it onto the parent.
    ///
    /// Out of range is [`Error::OutOfBounds`]: nothing was written, and
    /// the caller is handed the slice's size so it can clamp and retry.
    fn rebase_write(&self, offset: u64, len: usize) -> Result<u64> {
        self.rebase(offset, len as u64).ok_or(Error::OutOfBounds {
            offset,
            len: len as u64,
            size: self.length,
        })
    }
}

/// Borrowed slice of a parent `BlockRead`.
///
/// `read_at(0, …)` reads `start` of the parent. Reads outside
/// `[0, length)` return [`Error::ShortRead`] with `got: 0`.
pub struct SliceReader<'a> {
    parent: &'a (dyn BlockRead + 'a),
    geom: SliceGeometry,
}

impl<'a> SliceReader<'a> {
    /// `length` is clamped to what the parent can back, so
    /// `size_bytes()` never exceeds it. See [`window_on_parent`].
    pub fn new(parent: &'a (dyn BlockRead + 'a), start: u64, length: u64) -> Self {
        let geom = SliceGeometry::new(parent.size_bytes(), start, length);
        Self { parent, geom }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl<'a> BlockRead for SliceReader<'a> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

/// Slices are read-only by default — even where the parent is writable,
/// slicing is almost always paired with a read-only inspection or
/// dispatch workflow.
impl<'a> BlockDevice for SliceReader<'a> {}

/// Owned slice over an `Arc<dyn BlockRead>`. Use when the parent's
/// lifetime can't be expressed in a borrow — e.g. when the slice is
/// handed across an FFI boundary or stored in a long-lived struct.
///
/// Reads outside `[0, length)` return [`Error::ShortRead`] with `got: 0`.
pub struct OwnedSlice {
    parent: Arc<dyn BlockRead>,
    geom: SliceGeometry,
}

impl OwnedSlice {
    /// `length` is clamped to what the parent can back, so
    /// `size_bytes()` never exceeds it. See [`window_on_parent`].
    pub fn new(parent: Arc<dyn BlockRead>, start: u64, length: u64) -> Self {
        let geom = SliceGeometry::new(parent.size_bytes(), start, length);
        Self { parent, geom }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl BlockRead for OwnedSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

/// Same rationale as `SliceReader`: read-only by default.
impl BlockDevice for OwnedSlice {}

/// Owned, read-WRITE slice over an `Arc<dyn BlockDevice>`. Use when the
/// parent is writable and the slice should propagate writes (e.g. an
/// individual partition handed to a filesystem driver).
///
/// Reads outside `[0, length)` return [`Error::ShortRead`] with `got: 0`;
/// writes outside it return [`Error::OutOfBounds`]. The two directions
/// differ on purpose — see the module docs.
pub struct OwnedRwSlice {
    parent: Arc<dyn BlockDevice>,
    geom: SliceGeometry,
}

impl OwnedRwSlice {
    /// `length` is clamped to what the parent can back, so
    /// `size_bytes()` never exceeds it and a write past the parent's end
    /// is [`Error::OutOfBounds`] from this slice rather than whatever
    /// the parent makes of an address beyond itself. See
    /// [`window_on_parent`].
    pub fn new(parent: Arc<dyn BlockDevice>, start: u64, length: u64) -> Self {
        let geom = SliceGeometry::new(parent.size_bytes(), start, length);
        Self { parent, geom }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl BlockRead for OwnedRwSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

impl BlockDevice for OwnedRwSlice {
    /// Range first, writability second: a write that is both out of range
    /// and aimed at a read-only parent reports [`Error::OutOfBounds`],
    /// not [`Error::ReadOnly`]. The range is a property of this slice and
    /// is knowable without asking the parent anything, so it is the more
    /// specific of the two answers.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let at = self.geom.rebase_write(offset, buf.len())?;
        if !self.parent.is_writable() {
            return Err(Error::ReadOnly);
        }
        self.parent.write_at(at, buf)
    }

    fn flush(&self) -> Result<()> {
        self.parent.flush()
    }

    fn is_writable(&self) -> bool {
        self.parent.is_writable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_device::{Bytes, RwBytes};
    use std::sync::Mutex;

    #[test]
    fn slice_reader_rebases_offsets() {
        let mut v = vec![0u8; 4096];
        v[2000..2004].copy_from_slice(&[0xAB, 0xCD, 0xEF, 0x01]);
        let dev = Bytes(Mutex::new(v));

        let slice = SliceReader::new(&dev, 2000, 4);
        assert_eq!(slice.size_bytes(), 4);
        assert_eq!(slice.start(), 2000);
        assert_eq!(slice.length(), 4);

        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xAB, 0xCD, 0xEF, 0x01]);
    }

    /// A slice's geometry comes from a partition table, and a partition
    /// table comes off the disk. A start and a length that add up past
    /// 2^64 are an ordinary thing to be handed.
    ///
    /// In a release build the rebasing addition wrapped, so a read at an
    /// offset inside the slice's declared length landed somewhere else
    /// on the parent entirely -- and came back `Ok`, with those bytes,
    /// as though they were the slice's own.
    ///
    /// The `checked_add` in `rebase` is no longer what stops this: the
    /// constructor's clamp gets there first, and a start of 2^63 over a
    /// 64 KiB parent now leaves nothing to read at all. Both assertions
    /// are kept -- the size, which is the guard now in force, and the
    /// bytes, which are what went wrong.
    #[test]
    fn a_slice_whose_start_plus_offset_leaves_the_parent_reads_nothing() {
        let mut v = vec![0u8; 64 * 1024];
        v[5000..5008].copy_from_slice(b"SECRET!!");
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(v)));

        // A GPT entry of starting_lba = 2^54 and ending_lba = 2^55 + 99
        // produces exactly this.
        let slice = OwnedSlice::new(dev, 1 << 63, (1 << 63) + 51200);
        assert_eq!(
            slice.size_bytes(),
            0,
            "the window begins past the parent, so none of it is there"
        );
        let mut buf = [0u8; 8];
        let inside_the_declared_length = (1u64 << 63) + 5000;

        let outcome = slice.read_at(inside_the_declared_length, &mut buf);
        assert!(
            outcome.is_err(),
            "the read succeeded and returned {:?}, which is the parent's \
             bytes from offset 5000",
            std::str::from_utf8(&buf)
        );
        assert_ne!(&buf, b"SECRET!!");
    }

    #[test]
    fn slice_reader_rejects_out_of_bounds() {
        let dev = Bytes(Mutex::new(vec![0u8; 4096]));
        let slice = SliceReader::new(&dev, 0, 16);
        let mut buf = [0u8; 8];
        match slice.read_at(12, &mut buf) {
            Err(Error::ShortRead { .. }) => {}
            other => panic!("expected ShortRead, got {other:?}"),
        }
    }

    #[test]
    fn owned_slice_works_through_arc() {
        let mut v = vec![0u8; 4096];
        v[100..104].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(v)));

        let slice = OwnedSlice::new(dev, 100, 4);
        assert_eq!(slice.size_bytes(), 4);
        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn slices_reject_writes_via_blockdevice_default() {
        let dev = Bytes(Mutex::new(vec![0u8; 16]));
        let slice = SliceReader::new(&dev, 0, 8);
        let err = BlockDevice::write_at(&slice, 0, &[1u8; 4]).unwrap_err();
        assert!(matches!(err, Error::ReadOnly));
    }

    #[test]
    fn owned_slice_accessors_report_geometry() {
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(vec![0u8; 4096])));
        let slice = OwnedSlice::new(dev, 512, 256);
        assert_eq!(slice.start(), 512);
        assert_eq!(slice.length(), 256);
        assert_eq!(slice.size_bytes(), 256);
    }

    #[test]
    fn owned_rw_slice_accessors_report_geometry() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 16, 32);
        assert_eq!(slice.start(), 16);
        assert_eq!(slice.length(), 32);
        assert_eq!(slice.size_bytes(), 32);
        assert!(slice.is_writable());
    }

    #[test]
    fn owned_rw_slice_rebases_reads_and_writes() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev.clone(), 16, 32);

        // Write through the slice lands at parent offset 16.
        slice.write_at(0, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xDE, 0xAD, 0xBE, 0xEF]);

        // Confirm rebasing against the parent directly.
        let mut pbuf = [0u8; 4];
        dev.read_at(16, &mut pbuf).unwrap();
        assert_eq!(pbuf, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn owned_rw_slice_rejects_out_of_bounds_write() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 0, 8);
        match slice.write_at(6, &[0u8; 4]) {
            Err(Error::OutOfBounds { .. }) => {}
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    /// The bounds rule is direction-dependent by design: one slice, one
    /// out-of-range span, two different errors. Pinned here so the
    /// asymmetry cannot be "tidied up" into consistency without someone
    /// deciding to — the reasoning is in the module docs.
    #[test]
    fn same_out_of_range_span_is_short_read_for_a_read_and_out_of_bounds_for_a_write() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 16, 8);

        let mut buf = [0u8; 4];
        match slice.read_at(6, &mut buf) {
            Err(Error::ShortRead { offset, want, got }) => {
                assert_eq!((offset, want, got), (6, 4, 0));
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }

        match slice.write_at(6, &[0u8; 4]) {
            Err(Error::OutOfBounds { offset, len, size }) => {
                assert_eq!((offset, len, size), (6, 4, 8));
            }
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    /// The rule itself, at its edges. Clamp where the window merely runs
    /// off the end; `None` only where it begins at or past the end and
    /// there is nothing to slice.
    #[test]
    fn window_on_parent_clamps_the_length_and_refuses_only_a_start_past_the_end() {
        // Wholly inside: untouched.
        assert_eq!(window_on_parent(1024, 0, 1024), Some(1024));
        assert_eq!(window_on_parent(1024, 512, 512), Some(512));
        assert_eq!(window_on_parent(1024, 1023, 1), Some(1));

        // Running off the end: as much of it as is there. A `dd` of the
        // first part of a disk, or a table left stale after a shrink.
        assert_eq!(window_on_parent(1024, 512, 513), Some(512));
        assert_eq!(window_on_parent(1024, 0, u64::MAX), Some(1024));

        // Beginning at or past the end: nothing on the parent to slice.
        assert_eq!(window_on_parent(1024, 1024, 1), None);
        assert_eq!(window_on_parent(1024, 4096, 1), None);
        assert_eq!(window_on_parent(0, 0, 8), None);

        // The pair a GPT entry of starting_lba 2^54 and ending_lba
        // 2^55 + 99 produces: the sum leaves a u64 entirely, and the
        // start alone is already past the parent.
        assert_eq!(
            window_on_parent(64 * 1024, 1 << 63, (1 << 63) + 51200),
            None
        );

        // The arithmetic case from the top of the address space. The
        // clamp is what keeps `start + offset + len` inside a u64: 4
        // bytes rebase to exactly `u64::MAX`, and the requested 8 would
        // not have.
        assert_eq!(window_on_parent(u64::MAX, u64::MAX - 4, 8), Some(4));
    }

    /// `size_bytes()` is what bounds checks are done against, so a
    /// borrowed slice must not report a window its parent cannot back.
    ///
    /// The read assertion is on the ShortRead's FIELDS, not on
    /// `is_err()`. An unclamped slice rebases 45 to 95 and the parent
    /// refuses that too -- the error arrives either way. What tells the
    /// two apart is whose bounds were consulted: the slice's own offset
    /// and `got: 0`, or the parent's absolute 95 and the five bytes it
    /// could have delivered.
    #[test]
    fn a_borrowed_slice_cannot_claim_more_than_its_parent_holds() {
        let dev = Bytes::new(vec![0xEE; 100]);
        let slice = SliceReader::new(&dev, 50, 100);

        assert_eq!(slice.size_bytes(), 50);
        assert_eq!(slice.length(), 50);
        assert_eq!(
            slice.start(),
            50,
            "the start is not clamped, only the length"
        );

        let mut buf = [0u8; 8];
        match slice.read_at(45, &mut buf) {
            Err(Error::ShortRead { offset, want, got }) => {
                assert_eq!((offset, want, got), (45, 8, 0));
            }
            other => panic!("expected the slice's own ShortRead, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 8], "a refused read leaves the buffer alone");
    }

    /// The same for the `Arc` variant, which is the one the C ABI and
    /// the partition walkers reach.
    #[test]
    fn an_owned_slice_cannot_claim_more_than_its_parent_holds() {
        let mut v = vec![0u8; 100];
        v[50..58].copy_from_slice(b"LASTHALF");
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes::new(v));
        let slice = OwnedSlice::new(dev, 50, 100);

        assert_eq!(slice.size_bytes(), 50);
        assert_eq!(slice.length(), 50);

        // What is there still reads.
        let mut buf = [0u8; 8];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"LASTHALF");

        match slice.read_at(45, &mut buf) {
            Err(Error::ShortRead { offset, want, got }) => {
                assert_eq!((offset, want, got), (45, 8, 0));
            }
            other => panic!("expected the slice's own ShortRead, got {other:?}"),
        }
    }

    /// The write direction has its own currency, and an over-long slice
    /// lost it: the write was inside the declared length, so it was
    /// forwarded, and the caller got whatever the parent makes of an
    /// address beyond itself -- a `ShortRead` from a WRITE, in the case
    /// of the in-memory double, and a silently extended file in the case
    /// of `FileDevice`, which seeks and writes without a bounds check.
    #[test]
    fn an_rw_slice_refuses_a_write_past_its_parents_end_as_out_of_bounds() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes::new(vec![0u8; 64]));
        let slice = OwnedRwSlice::new(dev.clone(), 32, 4096);

        assert_eq!(slice.size_bytes(), 32);

        // Inside the clamped window still writes through.
        slice.write_at(0, &[0xAB; 4]).unwrap();
        let mut pbuf = [0u8; 4];
        dev.read_at(32, &mut pbuf).unwrap();
        assert_eq!(pbuf, [0xAB; 4]);

        match slice.write_at(32, &[0xCD; 8]) {
            Err(Error::OutOfBounds { offset, len, size }) => {
                assert_eq!((offset, len, size), (32, 8, 32));
            }
            other => panic!("expected the slice's own OutOfBounds, got {other:?}"),
        }
        assert_eq!(dev.size_bytes(), 64, "the parent did not grow");
    }

    /// A window that begins at or past the parent's end is a zero-byte
    /// device, not a window onto somewhere else.
    #[test]
    fn a_slice_starting_past_its_parents_end_is_empty() {
        let mut v = vec![0u8; 100];
        v[0..8].copy_from_slice(b"NOTYOURS");
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes::new(v));
        let slice = OwnedSlice::new(dev, 200, 64);

        assert_eq!(slice.size_bytes(), 0);

        let mut buf = [0u8; 8];
        match slice.read_at(0, &mut buf) {
            Err(Error::ShortRead { offset, want, got }) => {
                assert_eq!((offset, want, got), (0, 8, 0));
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 8]);
    }

    #[test]
    fn owned_rw_slice_flush_delegates_to_parent() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 8])));
        let slice = OwnedRwSlice::new(dev, 0, 8);
        // Default `flush` on RwBytes is a no-op success; the slice forwards it.
        slice.flush().unwrap();
    }
}
