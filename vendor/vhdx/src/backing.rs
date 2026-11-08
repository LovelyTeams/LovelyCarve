//! Pluggable, positioned backing store for one VHDX container.
//!
//! The reader historically held the **entire image** in a `Vec<u8>` — a 2 TB
//! VHDX meant a 2 TB heap. [`Backing`] generalises that to four positioned-read
//! backings WITHOUT a boxed trait, so the hot read path stays vtable-free (a
//! `match` the compiler can inline, not a dynamic dispatch):
//!
//! - [`Backing::File`] — a loose container file (the bounded path), read with
//!   the OS positioned-read primitive: only the BAT-selected blocks ever touch
//!   RAM, so peak memory no longer scales with image size.
//! - [`Backing::Sub`] — a contiguous sub-range of a larger file (a STORED, i.e.
//!   uncompressed, zip entry sits at a fixed offset inside the archive):
//!   `read_at(buf, off)` preads at `base + off`, clamped to `len`.
//! - [`Backing::Mem`] — an in-RAM buffer (the legacy `from_bytes` path, or a
//!   DEFLATED zip entry inflated to memory once): `read_at` copies from the
//!   slice.
//! - [`Backing::Reader`] — an arbitrary boxed [`ReadSeekSend`] reader (the
//!   forensic-vfs engine path): `read_at` locks a mutex, seeks to the requested
//!   offset, and reads — bridging the `Read + Seek` world to the positioned-read
//!   API. Keeps `forbid(unsafe)` (no mmap).
//!
//! All four expose the same cursor-free positioned-read API (`read_at` + `len`),
//! so the open/parse pass reads small structures from their known offsets and the
//! data path reads just the resolved block — never the whole file.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

/// A boxed `Read + Seek + Send` reader that can be used as a [`Backing::Reader`].
///
/// Any type implementing `Read + Seek + Send` satisfies this trait via the blanket
/// impl below. This is the trait [`crate::VhdxReader::open_reader`] accepts so the
/// forensic-vfs engine can hand a `SourceCursor` (which is `Read + Seek + Send`)
/// straight to the VHDX parser without forensic-vfs appearing in the production
/// dependency tree.
pub trait ReadSeekSend: Read + Seek + Send {}

impl<T: Read + Seek + Send> ReadSeekSend for T {}

/// Fill `buf` from `file` starting at `offset`, returning the bytes read (short
/// only at end of file).
///
/// Uses the OS positioned-read primitive — `pread(2)` on Unix, `seek_read`
/// (a `ReadFile` carrying its own `OVERLAPPED` offset) on Windows — so it takes
/// `&File` and never touches a shared cursor. That makes it safe to call
/// concurrently from many threads on one handle: each call carries its own
/// offset, so there is no read/seek race. Keeps `forbid(unsafe)` (no mmap).
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;

    let mut total = 0usize;
    while total < buf.len() {
        #[cfg(unix)]
        let res = file.read_at(&mut buf[total..], offset + total as u64);
        #[cfg(windows)]
        let res = file.seek_read(&mut buf[total..], offset + total as u64);
        #[cfg(not(any(unix, windows)))]
        let res: io::Result<usize> = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "positioned reads unsupported on this platform",
        ));
        match res {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// A positioned, cursor-free reader over one VHDX container's bytes.
///
/// `read_at(buf, offset)` fills `buf` from logical `offset` within the container
/// and returns the byte count (short only at end of container) — never touching
/// a shared cursor, so it is safe to call concurrently through `&self`.
pub enum Backing {
    /// A loose container file: positioned reads go straight to the OS handle.
    File(File),
    /// A contiguous sub-range `[base, base+len)` of a larger shared file.
    Sub {
        /// The backing file (shared; positioned reads carry their own offset).
        file: Arc<File>,
        /// Absolute file offset where this container's bytes begin.
        base: u64,
        /// Length of this container in bytes.
        len: u64,
    },
    /// An in-RAM container (e.g. the legacy `from_bytes` path or an inflated
    /// zip entry).
    Mem(Arc<[u8]>),
    /// An arbitrary seekable reader (e.g. a forensic-vfs `SourceCursor`).
    ///
    /// The inner reader is `Read + Seek + Send` but not cursor-free: `read_at`
    /// locks the mutex, seeks to the requested offset, then reads. Because the
    /// lock is held only for the duration of one `read_at` call, this is safe
    /// under `&self` — the mutex serialises concurrent accesses.
    Reader {
        /// The seekable reader, guarded by a mutex to satisfy `&self` [`Backing::read_at`].
        inner: Mutex<Box<dyn ReadSeekSend>>,
        /// Total byte length of the container, measured at construction.
        len: u64,
    },
}

impl Backing {
    /// Construct a [`Backing::Sub`] over `[base, base+len)` of `file`.
    #[must_use]
    pub fn sub(file: Arc<File>, base: u64, len: u64) -> Self {
        Backing::Sub { file, base, len }
    }

    /// Construct an in-RAM [`Backing::Mem`] from owned bytes.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Backing::Mem(bytes.into())
    }

    /// Total length of this container in bytes.
    #[must_use]
    pub fn len(&self) -> u64 {
        match self {
            Backing::File(f) => f.metadata().map_or(0, |m| m.len()),
            Backing::Mem(b) => b.len() as u64,
            Backing::Sub { len, .. } | Backing::Reader { len, .. } => *len,
        }
    }

    /// Whether the container is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill `buf` from logical `offset` within this container, returning the
    /// bytes read (short only at the container's end). Cursor-free and
    /// thread-safe.
    ///
    /// # Errors
    /// Propagates the underlying I/O error for [`Backing::File`] /
    /// [`Backing::Sub`]; [`Backing::Mem`] never fails.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        match self {
            Backing::File(f) => pread(f, buf, offset),
            Backing::Sub { file, base, len } => {
                // Clamp the request to this container's window so a Sub never
                // reads past its end into a neighbouring entry. A read starting
                // beyond the window yields 0 (clean EOF), mirroring File/Mem.
                let avail = len.saturating_sub(offset);
                if avail == 0 {
                    return Ok(0);
                }
                let want = (buf.len() as u64).min(avail) as usize;
                pread(file, &mut buf[..want], base + offset)
            }
            Backing::Mem(bytes) => {
                let off = offset.min(bytes.len() as u64) as usize;
                let src = &bytes[off..];
                let n = src.len().min(buf.len());
                buf[..n].copy_from_slice(&src[..n]);
                Ok(n)
            }
            Backing::Reader { inner, len } => {
                // Clamp to container bounds first — a read starting at or past
                // EOF returns 0 cleanly, mirroring the other variants.
                let avail = len.saturating_sub(offset);
                if avail == 0 {
                    return Ok(0);
                }
                let want = (buf.len() as u64).min(avail) as usize;
                // Acquire the lock. A poisoned mutex means the reader is in an
                // inconsistent state; surface that as an I/O error rather than
                // panicking, so the caller can decide how to handle it.
                let mut guard = inner
                    .lock()
                    .map_err(|_| io::Error::other("backing reader mutex poisoned"))?;
                guard.seek(SeekFrom::Start(offset))?;
                // Read in a loop to fill the buffer (mirroring pread semantics:
                // short reads are only expected at EOF).
                let dst = &mut buf[..want];
                let mut total = 0;
                while total < dst.len() {
                    match guard.read(&mut dst[total..])? {
                        0 => break, // EOF reached before filling the buffer.
                        n => total += n,
                    }
                }
                Ok(total)
            }
        }
    }

    /// Read exactly `len` bytes starting at `offset` into a fresh `Vec`.
    ///
    /// Used by the open/parse pass to pull a small, known-size structure
    /// (a header slot, the region table, the metadata region, the BAT) into a
    /// bounded buffer. The returned `Vec` is short only when the container ends
    /// before `offset + len` (so the caller still range-checks the parse).
    ///
    /// # Errors
    /// Propagates the underlying positioned-read error.
    pub fn read_exact_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let n = self.read_at(&mut buf, offset)?;
        buf.truncate(n);
        Ok(buf)
    }
}

impl std::fmt::Debug for Backing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backing::File(_) => f.debug_struct("File").field("len", &self.len()).finish(),
            Backing::Sub { base, len, .. } => f
                .debug_struct("Sub")
                .field("base", base)
                .field("len", len)
                .finish(),
            Backing::Mem(b) => f.debug_struct("Mem").field("len", &b.len()).finish(),
            Backing::Reader { len, .. } => f.debug_struct("Reader").field("len", len).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{Backing, ReadSeekSend};

    /// Blanket impl: [`std::io::Cursor`]`<Vec<u8>>` satisfies [`ReadSeekSend`].
    fn cursor_backing(data: Vec<u8>) -> Backing {
        let len = data.len() as u64;
        let inner = std::sync::Mutex::new(Box::new(Cursor::new(data)) as Box<dyn ReadSeekSend>);
        Backing::Reader { inner, len }
    }

    #[test]
    fn reader_backing_len_matches_construction() {
        let b = cursor_backing(vec![1u8, 2, 3, 4, 5]);
        assert_eq!(b.len(), 5);
        assert!(!b.is_empty());
    }

    #[test]
    fn reader_backing_read_at_fills_buf() {
        let b = cursor_backing(vec![10, 20, 30, 40, 50]);
        let mut buf = [0u8; 3];
        let n = b.read_at(&mut buf, 1).unwrap();
        assert_eq!(n, 3);
        assert_eq!(buf, [20, 30, 40]);
    }

    #[test]
    fn reader_backing_read_at_clamps_at_eof() {
        let b = cursor_backing(vec![10, 20, 30]);
        let mut buf = [0u8; 10];
        let n = b.read_at(&mut buf, 1).unwrap();
        assert_eq!(n, 2, "read past end should be clamped");
        assert_eq!(&buf[..2], &[20, 30]);
    }

    #[test]
    fn reader_backing_read_at_past_eof_returns_zero() {
        let b = cursor_backing(vec![1, 2, 3]);
        let mut buf = [0xFFu8; 4];
        let n = b.read_at(&mut buf, 100).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn reader_backing_debug_does_not_panic() {
        let b = cursor_backing(vec![0u8; 8]);
        let s = format!("{b:?}");
        assert!(s.contains("Reader"), "Debug output should name the variant");
    }

    #[test]
    fn reader_backing_read_exact_at_works() {
        let b = cursor_backing(vec![0, 1, 2, 3, 4, 5, 6, 7]);
        let v = b.read_exact_at(2, 4).unwrap();
        assert_eq!(v, vec![2, 3, 4, 5]);
    }

    #[test]
    fn empty_reader_backing_is_empty() {
        let b = cursor_backing(vec![]);
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }
}
