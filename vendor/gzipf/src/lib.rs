//! Read-only RFC 1952 GZIP support, using libyal/libgzipf's public API as a
//! capability reference. No C implementation or ABI is included.
//!
//! [`GzipDecoder`] streams concatenated members. Drain it to EOF to validate
//! every trailer and reject trailing junk; bytes returned before EOF are not
//! yet a guarantee of whole-file integrity. [`GzipFile`] validates the whole
//! source on open and provides [`std::io::Read`] / [`std::io::Seek`]. Seeking
//! restarts DEFLATE at the containing member, with O(member prefix) work.

#![doc = include_str!("../README.md")]

mod decoder;
mod file;
mod header;
mod input;
mod member;

pub use decoder::GzipDecoder;
pub use file::GzipFile;
pub use member::Member;

use std::io;
use std::sync::{atomic::AtomicBool, Arc};

/// Rust crate version, distinct from the libgzipf reference revision.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Bounds apply to the entire stream, including empty members and metadata.
/// Increase these explicitly for larger evidence; ISIZE is never an allocation
/// hint. Cancellation is checked between bounded I/O/decompression operations.
#[derive(Clone, Debug)]
pub struct Options {
    pub max_header_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_members: usize,
    pub max_uncompressed_bytes: u64,
    pub max_compressed_bytes: u64,
    pub(crate) cancellation: Option<Arc<AtomicBool>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_header_bytes: 1024 * 1024,
            max_metadata_bytes: 64 * 1024 * 1024,
            max_members: 65_536,
            max_uncompressed_bytes: 1 << 40,
            max_compressed_bytes: 1 << 40,
            cancellation: None,
        }
    }
}

impl Options {
    /// Attach a caller-owned cancellation flag. `true` terminates reading.
    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub(crate) fn check_cancelled(&self) -> io::Result<()> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            // Interrupted would be retried forever by read_to_end / io::copy.
            return Err(io::Error::other("GZIP operation cancelled"));
        }
        Ok(())
    }
}

/// Probe the two-byte GZIP signature only, without claiming validity.
pub fn check_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// Probe two bytes at the reader's current offset; advances that reader.
/// Short inputs return `UnexpectedEof`, I/O errors remain errors.
pub fn check_reader_signature(reader: &mut impl io::Read) -> io::Result<bool> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(check_signature(&bytes))
}

/// Probe a host file read-only. Native Rust paths cover Windows Unicode names.
pub fn check_file_signature(path: impl AsRef<std::path::Path>) -> io::Result<bool> {
    check_reader_signature(&mut std::fs::File::open(path)?)
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests;
