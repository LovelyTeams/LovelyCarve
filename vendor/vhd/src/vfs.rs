//! `forensic-vfs` integration: a decoded VHD as an [`ImageSource`].
//!
//! A decoded VHD is a read-only, randomly-addressable byte stream — the
//! `ImageSource` contract. [`VhdReader`] maps virtual sectors to on-disk data
//! through a `Read + Seek` cursor (the read advances an internal position, so it
//! needs `&mut self`). It is therefore wrapped here: [`VhdSource`] holds the
//! reader behind a poison-recovering `Mutex` and serves `read_at` by seeking then
//! reading under the lock — the same technique the sibling VHDX/VMDK adapters
//! use. Behind the `vfs` feature.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Mutex, PoisonError};

use forensic_vfs::{ImageSource, VfsError, VfsResult};

use crate::VhdReader;

/// A decoded [`VhdReader`] presented as a read-only [`ImageSource`].
///
/// Construction records the virtual disk size once; `read_at` locks the reader,
/// seeks, and fills the buffer. Because a VHD read advances an internal cursor
/// (`&mut self`), reads **serialize through the mutex** — correct and
/// `Send + Sync`, at the cost of no intra-source read parallelism. The lock is
/// poison-recovering, so one panicking reader does not wedge the source.
pub struct VhdSource {
    inner: Mutex<VhdReader>,
    len: u64,
}

impl VhdSource {
    /// Wrap an open [`VhdReader`], recording its virtual disk size as the source
    /// length.
    pub fn new(reader: VhdReader) -> Self {
        let len = reader.virtual_disk_size();
        Self {
            inner: Mutex::new(reader),
            len,
        }
    }
}

impl ImageSource for VhdSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let io_err = |op: &'static str| move |source: std::io::Error| VfsError::Io { op, source };
        let avail = self.len.saturating_sub(offset);
        if avail == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(avail) as usize;
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .seek(SeekFrom::Start(offset))
            .map_err(io_err("vhd::seek"))?;
        let mut total = 0;
        while total < want {
            let Some(slot) = buf.get_mut(total..want) else {
                break; // cov:unreachable: total < want <= buf.len()
            };
            match guard.read(slot).map_err(io_err("vhd::read"))? {
                0 => break,
                n => total += n,
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::sync::Arc;

    use forensic_vfs::ImageSource;

    use super::VhdSource;
    use crate::VhdReader;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ntfs_fixed.vhd");

    /// Open a real committed VHD and drive it through the `ImageSource` API,
    /// cross-checking the positioned read against the reader's own `Read` path
    /// (the oracle). Skips cleanly if the fixture is absent.
    #[test]
    fn vhd_reader_is_an_image_source() {
        let Ok(bytes) = std::fs::read(FIXTURE) else {
            eprintln!("skipping: fixture {FIXTURE} not present");
            return;
        };

        // Oracle: the reader's own Read path for the first sector.
        let mut direct =
            VhdReader::open_reader(Box::new(Cursor::new(bytes.clone()))).expect("open vhd");
        let expected_len = direct.virtual_disk_size();
        let read_len = expected_len.min(512) as usize;
        direct.seek(SeekFrom::Start(0)).expect("seek 0");
        let mut expected = vec![0u8; read_len];
        direct.read_exact(&mut expected).expect("direct read");

        // The load-bearing claim: a VhdReader composes as a dyn ImageSource.
        let reader = VhdReader::open_reader(Box::new(Cursor::new(bytes))).expect("open vhd");
        let src: Arc<dyn ImageSource> = Arc::new(VhdSource::new(reader));
        assert_eq!(src.len(), expected_len);
        assert!(!src.is_empty());

        // Positioned read matches the direct read, byte for byte.
        let mut buf = vec![0u8; read_len];
        let n = src.read_at(0, &mut buf).expect("read_at");
        assert_eq!(n, read_len);
        assert_eq!(buf, expected);

        // A read starting at EOF yields 0 (ImageSource short-read contract).
        let mut eof = [0u8; 16];
        assert_eq!(src.read_at(expected_len, &mut eof).expect("eof read"), 0);
    }
}
