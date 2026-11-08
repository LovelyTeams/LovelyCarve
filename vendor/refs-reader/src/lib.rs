//! Read-only ReFS disk structures, derived from Erik Larsson's refsprogs.
//!
//! The reader receives partition-relative bytes, so callers can use image
//! containers and decrypted volumes without materializing intermediate images.
//! See ORIGIN.md for the source revision and license of the port.
#![forbid(unsafe_code)]

mod attributes;
mod boot;
mod bytes;
mod data;
mod directory_cache;
mod entry;
mod extents;
mod metadata;
mod reparse;
mod shared_extents;
mod streams;
mod table;
mod volume;

#[cfg(test)]
mod tests;

pub use boot::BootSector;
pub use entry::FileEntry;
pub use extents::Extent;
pub use metadata::{Checkpoint, NodeReference};
pub use streams::StreamEntry;
pub use volume::{FileReader, Volume};

use std::io;

pub type Result<T> = io::Result<T>;

/// Supplies exact, partition-relative reads from immutable evidence bytes.
/// Implementers must reject overflow, out-of-volume requests and short reads;
/// they must not silently pad unavailable source bytes with zeros.
pub trait ReadAt: Send + Sync {
    fn len(&self) -> u64;
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<()>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn corrupt(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// A raw filename record and its on-disk value, before attribute decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryRecord {
    pub name: String,
    pub entry_type: u16,
    pub value: Vec<u8>,
    pub inode: Option<u64>,
    /// Filename-record FILETIMEs (created, accessed, modified, changed) when
    /// file data is resolved through a separate hardlink backing record.
    pub filename_timestamps: Option<[u64; 4]>,
}
