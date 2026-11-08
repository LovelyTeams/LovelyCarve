//! `forensic-vfs` integration: a decoded VMDK as an [`ImageSource`].
//!
//! A decoded VMDK is a read-only, randomly-addressable byte stream — the
//! `ImageSource` contract. [`VmdkReader`] resolves a virtual offset to physical
//! grains through a `Read + Seek` cursor (the translation advances an internal
//! position, so it needs `&mut self`). Unlike a `&self` positioned reader, it is
//! therefore wrapped here: [`VmdkSource`] holds the reader behind a
//! poison-recovering `Mutex` and serves `read_at` by seeking then reading under
//! the lock. Reads serialize through the mutex. Behind the `vfs` feature.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Mutex, PoisonError};

use forensic_vfs::{ImageSource, VfsError, VfsResult};

use crate::VmdkReader;

/// A decoded [`VmdkReader`] presented as a read-only [`ImageSource`].
///
/// Construction records the virtual disk size once; `read_at` locks the reader,
/// seeks, and fills the buffer. Because a VMDK read advances an internal cursor
/// (`&mut self`), reads **serialize through the mutex** — correct and
/// `Send + Sync`, at the cost of no intra-source read parallelism. The lock is
/// poison-recovering, so one panicking reader does not wedge the source.
pub struct VmdkSource<R: Read + Seek + Send> {
    inner: Mutex<VmdkReader<R>>,
    len: u64,
}

impl<R: Read + Seek + Send> VmdkSource<R> {
    /// Wrap an open [`VmdkReader`], recording its virtual disk size as the
    /// source length.
    pub fn new(reader: VmdkReader<R>) -> Self {
        let len = reader.virtual_disk_size();
        Self {
            inner: Mutex::new(reader),
            len,
        }
    }
}

impl<R: Read + Seek + Send + 'static> ImageSource for VmdkSource<R> {
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
            .map_err(io_err("vmdk::seek"))?;
        let mut total = 0;
        while total < want {
            match guard
                .read(&mut buf[total..want])
                .map_err(io_err("vmdk::read"))?
            {
                0 => break,
                n => total += n,
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use forensic_vfs::ImageSource;

    use super::VmdkSource;
    use crate::VmdkReader;

    /// A synthetic monolithicSparse VMDK whose single grain begins with a known
    /// marker, driven purely through the `ImageSource` API.
    #[test]
    fn vmdk_reader_is_an_image_source() {
        let marker = b"VMDK_VFS_MARKER_0123456789";
        let image = crate::testutil::test_sparse_vmdk(marker);
        let reader = VmdkReader::open(Cursor::new(image)).expect("open synthetic vmdk");
        let expected_len = reader.virtual_disk_size();

        // The load-bearing claim: a VmdkReader composes as a dyn ImageSource.
        let src: Arc<dyn ImageSource> = Arc::new(VmdkSource::new(reader));
        assert_eq!(src.len(), expected_len);
        assert!(!src.is_empty());

        // Positioned read of the first bytes returns the known marker.
        let mut buf = vec![0u8; marker.len()];
        let n = src.read_at(0, &mut buf).expect("read_at");
        assert_eq!(n, marker.len());
        assert_eq!(&buf, marker);

        // A read starting at EOF yields 0 (ImageSource short-read contract).
        let mut eof = [0u8; 16];
        assert_eq!(src.read_at(expected_len, &mut eof).expect("eof read"), 0);
    }
}
