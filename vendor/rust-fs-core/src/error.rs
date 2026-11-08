//! Unified error type. Each driver still keeps its own rich error type for
//! internal use; conversions to/from this one happen at the trait boundary.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure (open, seek, read, write).
    Io(io::Error),
    /// A read that could not be satisfied in full: the source ran out of
    /// data before `want` bytes had been transferred.
    ///
    /// `got` counts the bytes actually placed in the caller's buffer, not
    /// the bytes that were available. [`FileDevice`] copies what it can
    /// and reports that count, so a read straddling EOF comes back with
    /// the readable prefix in `buf` and `got` equal to its length. The
    /// slice adapters in [`crate::slice`] refuse an out-of-range read
    /// before touching the parent, so they leave `buf` untouched and
    /// always report `got: 0` — including for a read that begins inside
    /// the slice and runs off its end, where a [`FileDevice`] of the same
    /// size would have reported a non-zero prefix. So **`got: 0` always
    /// means nothing was transferred, and does not on its own tell you
    /// whether anything was available**: from a [`FileDevice`] at EOF it
    /// happens to mean both, from a slice it means only the former.
    ///
    /// It is not the only error an over-read can produce, because most of
    /// this crate's devices do not own the bytes they serve:
    ///
    /// - [`crate::CachingDevice`], [`crate::ReadOnlyDevice`] and the
    ///   slice adapters forward an in-range read to their parent and
    ///   return the parent's error unchanged. Over a parent that reports
    ///   over-reads as [`Error::OutOfBounds`] — the `img-*` container
    ///   readers do — that is what the wrapper reports too.
    /// - [`crate::CallbackDevice`] surfaces a failing host callback as
    ///   [`Error::Io`]. The callback ABI is an errno-space code carrying
    ///   no byte count, so there is nothing to put in `got`.
    ///
    /// [`FileDevice`]: crate::FileDevice
    ShortRead {
        offset: u64,
        want: usize,
        got: usize,
    },
    /// `write_at` invoked on a device opened read-only.
    ReadOnly,
    /// A request refused before any transfer because its range is not
    /// wholly inside the device's declared size. Nothing was read or
    /// written; `size` is the device size, so the caller can clamp.
    ///
    /// **This crate constructs it in exactly two places, both writes:**
    /// [`crate::OwnedRwSlice`]'s `write_at`, for a write outside the
    /// slice, and [`FileDevice`]'s `write_at`, for a write outside the
    /// size that device took at construction. No read path here builds
    /// it, so matching it to catch an over-read of a [`FileDevice`], or
    /// a read past a slice's own end, is an arm that will never be
    /// taken — those report [`Error::ShortRead`] with `got: 0`.
    ///
    /// The count is pinned by `tests/outofbounds_constructors.rs`
    /// against the source, because this paragraph said "exactly one
    /// place" for as long as it took someone to add the second.
    ///
    /// **It still reaches reads, from elsewhere.** A container that knows
    /// its virtual size before touching the backing store rejects an
    /// over-read up front rather than discovering EOF, and the
    /// `img-qcow2`, `img-vhd`, `img-vhdx` and `img-vmdk` readers all
    /// return `OutOfBounds` from `BlockRead::read_at` on that path. This
    /// crate's wrappers — [`crate::CachingDevice`],
    /// [`crate::ReadOnlyDevice`], the slice adapters — forward it
    /// unchanged from such a parent.
    ///
    /// So code reading through a `dyn BlockRead` of unknown provenance
    /// has to handle this as well as [`Error::ShortRead`] — and cannot
    /// treat the pair as exhaustive, because a
    /// [`crate::CallbackDevice`] reports its host's refusal as
    /// [`Error::Io`] however the host arrived at it. Only code that knows
    /// its device bottoms out in a [`FileDevice`] can rely on `ShortRead`
    /// alone.
    ///
    /// [`FileDevice`]: crate::FileDevice
    OutOfBounds { offset: u64, len: u64, size: u64 },
    /// Driver-specific error lifted to the trait boundary. Each driver's
    /// internal error type implements `Into<Error>` via this variant.
    Custom(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::ShortRead { offset, want, got } => {
                write!(f, "short read at {offset}: wanted {want} got {got}")
            }
            Error::ReadOnly => write!(f, "device is read-only"),
            Error::OutOfBounds { offset, len, size } => {
                write!(f, "{offset}+{len} past device size {size}")
            }
            Error::Custom(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
