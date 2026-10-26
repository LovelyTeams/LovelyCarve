//! `std::io::Read` + `std::io::Seek` adapter over any [`BlockRead`].
//!
//! Every consumer that wants to stream a positioned-read device
//! top-to-bottom (hashing, copying, burning, feeding into a parser that
//! expects `Read`) otherwise reinvents the same cursor wrapper: track the
//! current offset, call `read_at`, advance, clamp at `size_bytes`, signal
//! EOF. [`BlockReadStreamer`] is that wrapper, once.
//!
//! Generic over `T: BlockRead` so the parent can be owned, `Arc`-held, or
//! borrowed:
//!
//! ```ignore
//! use fs_core::{BlockReadStreamer, FileDevice};
//! use std::io::Read;
//!
//! let dev = FileDevice::open("disk.img")?;
//! let mut stream = BlockReadStreamer::new(dev);
//! let mut hasher = sha2::Sha256::new();
//! std::io::copy(&mut stream, &mut hasher)?;
//! ```
//!
//! Short-read semantics match `std::io::Read`: a read past `size_bytes()`
//! is clamped, not an error; a read at or past the end returns `Ok(0)`.

use crate::block::BlockRead;
use crate::error::Error;
use std::io::{self, Read, Seek, SeekFrom};

/// Sequential `Read` + `Seek` adapter over any `BlockRead`.
///
/// Holds the parent by value. Use:
/// - `BlockReadStreamer<MyDevice>` to own a concrete device.
/// - `BlockReadStreamer<std::sync::Arc<dyn BlockRead>>` for shared ownership.
/// - `BlockReadStreamer<&dyn BlockRead>` (or `&MyDevice`) to borrow.
pub struct BlockReadStreamer<T: BlockRead> {
    inner: T,
    pos: u64,
}

impl<T: BlockRead> BlockReadStreamer<T> {
    /// New streamer starting at offset 0.
    pub fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }

    /// New streamer starting at the given byte offset. The position is
    /// not bounded against `size_bytes()` — a position past the end is
    /// legal (just reads will return `Ok(0)`), matching `std::io::Seek`.
    pub fn with_position(inner: T, pos: u64) -> Self {
        Self { inner, pos }
    }

    /// Current byte offset within the device.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Borrow the wrapped device.
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Consume the streamer and return the wrapped device.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: BlockRead> Read for BlockReadStreamer<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.size_bytes();
        if self.pos >= size {
            return Ok(0);
        }
        let remaining = size - self.pos;
        let n = std::cmp::min(buf.len() as u64, remaining) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.inner
            .read_at(self.pos, &mut buf[..n])
            .map_err(fs_core_error_to_io)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<T: BlockRead> Seek for BlockReadStreamer<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        // `SeekFrom::End`/`Current` with a negative offset that would
        // place the cursor before byte 0 returns an InvalidInput error,
        // mirroring `std::io::Cursor`. Seeking *past* the end is allowed
        // — the next read returns `Ok(0)`.
        let new_pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(n) => offset_from(self.inner.size_bytes(), n)?,
            SeekFrom::Current(n) => offset_from(self.pos, n)?,
        };
        self.pos = new_pos;
        Ok(new_pos)
    }
}

fn offset_from(base: u64, delta: i64) -> io::Result<u64> {
    if delta >= 0 {
        base.checked_add(delta as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflows u64"))
    } else {
        let abs = delta.unsigned_abs();
        base.checked_sub(abs).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek would place cursor before byte 0",
            )
        })
    }
}

fn fs_core_error_to_io(e: Error) -> io::Error {
    match e {
        Error::Io(io) => io,
        Error::ShortRead { offset, want, got } => io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("short read at {offset}: wanted {want} got {got}"),
        ),
        Error::OutOfBounds { offset, len, size } => io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("{offset}+{len} past device size {size}"),
        ),
        Error::ReadOnly => io::Error::new(io::ErrorKind::PermissionDenied, "device is read-only"),
        Error::Custom(s) => io::Error::other(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as FsResult;
    use crate::test_device::Bytes;
    use std::sync::{Arc, Mutex};

    /// `BlockRead` that always errors with `Custom` — used to verify
    /// error propagation through the streamer.
    struct AlwaysFails;
    impl BlockRead for AlwaysFails {
        fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<()> {
            Err(Error::Custom("simulated failure".into()))
        }
        fn size_bytes(&self) -> u64 {
            1024
        }
    }

    fn fixture() -> Bytes {
        let mut v = vec![0u8; 32];
        for (i, b) in v.iter_mut().enumerate() {
            *b = i as u8;
        }
        Bytes(Mutex::new(v))
    }

    #[test]
    fn read_to_end_returns_full_contents() {
        let mut s = BlockReadStreamer::new(fixture());
        let mut out = Vec::new();
        let n = s.read_to_end(&mut out).unwrap();
        assert_eq!(n, 32);
        assert_eq!(out.len(), 32);
        assert_eq!(out[0], 0);
        assert_eq!(out[31], 31);
    }

    #[test]
    fn partial_end_read_is_clamped_not_errored() {
        let mut s = BlockReadStreamer::with_position(fixture(), 30);
        let mut buf = [0u8; 16];
        let n = s.read(&mut buf).unwrap();
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], &[30, 31]);
        assert_eq!(s.position(), 32);
    }

    #[test]
    fn read_at_eof_returns_zero() {
        let mut s = BlockReadStreamer::with_position(fixture(), 32);
        let mut buf = [0u8; 8];
        assert_eq!(s.read(&mut buf).unwrap(), 0);
        // Still zero on subsequent reads.
        assert_eq!(s.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn read_past_eof_position_returns_zero() {
        let mut s = BlockReadStreamer::with_position(fixture(), 9_999);
        let mut buf = [0u8; 8];
        assert_eq!(s.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn zero_length_buf_returns_zero() {
        let mut s = BlockReadStreamer::new(fixture());
        let mut buf: [u8; 0] = [];
        assert_eq!(s.read(&mut buf).unwrap(), 0);
        assert_eq!(s.position(), 0);
    }

    #[test]
    fn position_advances_after_read() {
        let mut s = BlockReadStreamer::new(fixture());
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).unwrap();
        assert_eq!(s.position(), 4);
        assert_eq!(buf, [0, 1, 2, 3]);

        s.read_exact(&mut buf).unwrap();
        assert_eq!(s.position(), 8);
        assert_eq!(buf, [4, 5, 6, 7]);
    }

    #[test]
    fn seek_start_jumps_absolute() {
        let mut s = BlockReadStreamer::new(fixture());
        let p = s.seek(SeekFrom::Start(10)).unwrap();
        assert_eq!(p, 10);
        assert_eq!(s.position(), 10);
        let mut buf = [0u8; 2];
        s.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [10, 11]);
    }

    #[test]
    fn seek_end_jumps_relative_to_size() {
        let mut s = BlockReadStreamer::new(fixture());
        let p = s.seek(SeekFrom::End(-4)).unwrap();
        assert_eq!(p, 28);
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [28, 29, 30, 31]);
    }

    #[test]
    fn seek_current_jumps_relative_to_cursor() {
        let mut s = BlockReadStreamer::with_position(fixture(), 10);
        let p = s.seek(SeekFrom::Current(5)).unwrap();
        assert_eq!(p, 15);
        let p = s.seek(SeekFrom::Current(-3)).unwrap();
        assert_eq!(p, 12);
    }

    #[test]
    fn seek_past_end_is_allowed_then_read_returns_zero() {
        let mut s = BlockReadStreamer::new(fixture());
        assert_eq!(s.seek(SeekFrom::Start(1_000_000)).unwrap(), 1_000_000);
        let mut buf = [0u8; 4];
        assert_eq!(s.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn seek_before_zero_is_invalid_input() {
        let mut s = BlockReadStreamer::new(fixture());
        let err = s.seek(SeekFrom::Current(-1)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        let err = s.seek(SeekFrom::End(-99_999)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn works_through_arc_dyn_blockread() {
        let dev: Arc<dyn BlockRead> = Arc::new(fixture());
        let mut s = BlockReadStreamer::new(dev);
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn works_through_borrowed_reference() {
        let dev = fixture();
        {
            let mut s = BlockReadStreamer::new(&dev as &dyn BlockRead);
            let mut buf = [0u8; 8];
            s.read_exact(&mut buf).unwrap();
            assert_eq!(buf, [0, 1, 2, 3, 4, 5, 6, 7]);
        }
        // `dev` is still usable after the streamer goes out of scope.
        assert_eq!(dev.size_bytes(), 32);
    }

    #[test]
    fn into_inner_returns_wrapped_device() {
        let s = BlockReadStreamer::new(fixture());
        let inner = s.into_inner();
        assert_eq!(inner.size_bytes(), 32);
    }

    #[test]
    fn get_ref_exposes_inner_without_consuming() {
        let s = BlockReadStreamer::new(fixture());
        assert_eq!(s.get_ref().size_bytes(), 32);
        // Streamer still usable.
        assert_eq!(s.position(), 0);
    }

    #[test]
    fn error_from_inner_propagates_as_io_error() {
        let mut s = BlockReadStreamer::new(AlwaysFails);
        let mut buf = [0u8; 8];
        let err = s.read(&mut buf).unwrap_err();
        // `Custom` is mapped via `io::Error::other`, whose kind is `Other`.
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert!(err.to_string().contains("simulated failure"));
    }

    #[test]
    fn offset_from_handles_add_sub_and_overflow() {
        assert_eq!(offset_from(10, 5).unwrap(), 15);
        assert_eq!(offset_from(10, -4).unwrap(), 6);
        assert_eq!(offset_from(10, 0).unwrap(), 10);

        // Positive delta that overflows u64.
        let err = offset_from(u64::MAX, 1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // Negative delta past zero.
        let err = offset_from(3, -4).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn error_mapping_covers_every_variant() {
        // Io is forwarded with its original kind preserved.
        let io_err = fs_core_error_to_io(Error::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "missing",
        )));
        assert_eq!(io_err.kind(), io::ErrorKind::NotFound);

        let sr = fs_core_error_to_io(Error::ShortRead {
            offset: 4,
            want: 8,
            got: 2,
        });
        assert_eq!(sr.kind(), io::ErrorKind::UnexpectedEof);
        assert!(sr.to_string().contains("short read"));

        let oob = fs_core_error_to_io(Error::OutOfBounds {
            offset: 16,
            len: 4,
            size: 8,
        });
        assert_eq!(oob.kind(), io::ErrorKind::UnexpectedEof);

        let ro = fs_core_error_to_io(Error::ReadOnly);
        assert_eq!(ro.kind(), io::ErrorKind::PermissionDenied);

        let custom = fs_core_error_to_io(Error::Custom("boom".into()));
        assert_eq!(custom.kind(), io::ErrorKind::Other);
        assert!(custom.to_string().contains("boom"));
    }
}
