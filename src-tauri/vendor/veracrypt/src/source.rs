//! Positional reads from the bytes that hold a volume.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

/// The bytes a volume lives in: a container file, a partition, or the whole
/// drive of a system-encrypted volume.
///
/// The reader fetches ciphertext at absolute offsets, possibly from several
/// threads at once, so implementations take `&self` and are shared. A read
/// fills the whole buffer or fails: a short read is an error, never padding.
pub trait ReadAt: Send + Sync {
    /// Total bytes the source holds.
    fn size(&self) -> u64;

    /// Fill `buf` from `offset`.
    ///
    /// # Errors
    /// I/O failures, and [`io::ErrorKind::UnexpectedEof`] when the range
    /// leaves the source.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

fn eof() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "read past the end of the source",
    )
}

impl ReadAt for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| eof())?;
        let end = start.checked_add(buf.len()).ok_or_else(eof)?;
        buf.copy_from_slice(self.get(start..end).ok_or_else(eof)?);
        Ok(())
    }
}

impl ReadAt for Vec<u8> {
    fn size(&self) -> u64 {
        self.as_slice().size()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.as_slice().read_exact_at(offset, buf)
    }
}

impl<T: ReadAt + ?Sized> ReadAt for &T {
    fn size(&self) -> u64 {
        (**self).size()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        (**self).read_exact_at(offset, buf)
    }
}

impl<T: ReadAt + ?Sized> ReadAt for Arc<T> {
    fn size(&self) -> u64 {
        (**self).size()
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        (**self).read_exact_at(offset, buf)
    }
}

/// A host file read at explicit offsets.
pub struct FileSource {
    file: std::fs::File,
    size: u64,
}

impl FileSource {
    /// Wrap an open file.
    ///
    /// # Errors
    /// The file's metadata cannot be read.
    pub fn new(file: std::fs::File) -> io::Result<Self> {
        let size = file.metadata()?.len();
        Ok(Self { file, size })
    }

    /// Open the file at `path`.
    ///
    /// # Errors
    /// I/O errors.
    pub fn open(path: impl AsRef<std::path::Path>) -> io::Result<Self> {
        Self::new(std::fs::File::open(path)?)
    }
}

impl ReadAt for FileSource {
    fn size(&self) -> u64 {
        self.size
    }

    #[cfg(unix)]
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::os::unix::fs::FileExt;
        self.file.read_exact_at(buf, offset)
    }

    #[cfg(windows)]
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        let mut filled = 0;
        while filled < buf.len() {
            match self
                .file
                .seek_read(&mut buf[filled..], offset + filled as u64)
            {
                Ok(0) => return Err(eof()),
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Any `Read + Seek` source, serialized behind a lock.
pub struct SeekSource<R> {
    inner: Mutex<R>,
    size: u64,
}

impl<R: Read + Seek + Send> SeekSource<R> {
    /// Wrap `inner`, measuring its size.
    ///
    /// # Errors
    /// Seeking to the end fails.
    pub fn new(mut inner: R) -> io::Result<Self> {
        let size = inner.seek(SeekFrom::End(0))?;
        Ok(Self {
            inner: Mutex::new(inner),
            size,
        })
    }
}

impl<R: Read + Seek + Send> ReadAt for SeekSource<R> {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("source lock poisoned"))?;
        inner.seek(SeekFrom::Start(offset))?;
        inner.read_exact(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_and_seek_sources_read_exactly_or_fail() {
        let bytes: Vec<u8> = (0..100).collect();
        let mut buf = [0u8; 10];
        bytes.read_exact_at(90, &mut buf).unwrap();
        assert_eq!(buf[0], 90);
        assert!(bytes.read_exact_at(91, &mut buf).is_err());
        assert!(bytes.read_exact_at(u64::MAX, &mut buf).is_err());
        let seek = SeekSource::new(io::Cursor::new(bytes)).unwrap();
        assert_eq!(seek.size(), 100);
        seek.read_exact_at(5, &mut buf).unwrap();
        assert_eq!(buf[0], 5);
        assert!(seek.read_exact_at(95, &mut buf).is_err());
    }
}
