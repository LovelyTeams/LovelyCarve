//! Single and split RAW images without native libraries or FFI.
//!
//! RAW has no header, segment count or stored checksum. Discovery validates
//! observable numbering gaps; callers needing completeness must provide an
//! expected length or an independently known explicit segment list.
mod discovery;
mod metadata;
mod reader;
mod writer;

pub use discovery::{discover, Discovery, MAX_SEGMENTS};
pub use metadata::{MediaMetadata, MetadataValues};
pub use reader::{RawCursor, RawImage, Segment};
pub use writer::RawWriter;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cloneable cancellation handle; no runtime or polling thread is required.
#[derive(Clone, Debug, Default)]
pub struct AbortHandle(Arc<AtomicBool>);

impl AbortHandle {
    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub(crate) fn check(&self) -> io::Result<()> {
        if self.is_aborted() {
            Err(io::Error::other("RAW operation aborted"))
        } else {
            Ok(())
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests;
