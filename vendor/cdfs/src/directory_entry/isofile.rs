// SPDX-License-Identifier: (MIT OR Apache-2.0)

#[allow(unused)]
use log::{debug, error, info, trace, warn};

use std::{
    cmp::min,
    convert::TryFrom,
    fmt,
    io::{self, Read, Seek, SeekFrom, Write},
    str::FromStr,
};

use super::{DirectoryEntryHeader, ExtraAttributes, ExtraMeta};
use crate::{BlockBuffer, BlockBufferCtor, FileRef, ISO9660Reader, Result, BLOCK_SIZE};

/// [`DirectoryEntry`](crate::DirectoryEntry) for regular files.
///
/// # See Also
///
/// ISO-9660 / ECMA-119 § 9
#[derive(Clone)]
pub struct ISOFile<T: ISO9660Reader> {
    pub(crate) header: DirectoryEntryHeader,

    /// The filename encoded with UTF-8.  Note that most often filenames will not be UTF-8 encoded in the ISO disc image.
    pub identifier: String,

    /// File version; ranges from 1 to 32767
    pub version: u16,

    pub(super) ext: ExtraMeta,

    file: FileRef<T>,
}

impl<T: ISO9660Reader> ExtraAttributes for ISOFile<T> {
    fn ext(&self) -> &ExtraMeta {
        &self.ext
    }

    fn header(&self) -> &DirectoryEntryHeader {
        &self.header
    }
}

impl<T: ISO9660Reader> fmt::Debug for ISOFile<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("ISOFile")
            .field("header", &self.header)
            .field("identifier", &self.identifier)
            .field("version", &self.version)
            .field("ext", &self.ext)
            .finish()
    }
}

impl<T: ISO9660Reader> ISOFile<T> {
    pub(crate) fn new(
        header: DirectoryEntryHeader,
        ext: ExtraMeta,
        identifier: String,
        file: FileRef<T>,
    ) -> Result<Self> {
        let mut identifier = match ext.alt_name.as_ref() {
            Some(alt_name) => alt_name.clone(),
            None => identifier,
        };

        // Files (not directories) in ISO 9660 have a version number, which is
        // provided at the end of the identifier, seperated by ';'.
        // If not, assume 1.
        let version = match identifier.rfind(';') {
            Some(idx) => {
                let version = u16::from_str(&identifier[idx + 1..])?;
                identifier.truncate(idx);
                version
            }
            None => 1,
        };

        // Files without an extension have a '.' at the end
        if identifier.ends_with('.') {
            identifier.pop();
        }

        Ok(ISOFile {
            header,
            identifier,
            version,
            ext,
            file,
        })
    }

    /// Returns the size of the file in bytes.
    pub fn size(&self) -> u32 {
        self.header.extent_length
    }

    /// Returns an [`ISOFileReader`] for this file.
    pub fn read(&self) -> ISOFileReader<T> {
        ISOFileReader {
            buf: BlockBuffer::new(),
            buf_lba: None,
            seek: 0,
            start_lba: self.header.extent_loc,
            size: self.size() as usize,
            file: self.file.clone(),
        }
    }
}

/// A struct providing read-only access to a file on the filesystem.
pub struct ISOFileReader<T: ISO9660Reader> {
    buf: BlockBuffer,
    buf_lba: Option<u64>,
    seek: usize,
    start_lba: u32,
    size: usize,
    file: FileRef<T>,
}

impl<T: ISO9660Reader> Read for ISOFileReader<T> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let blksize = usize::from(BLOCK_SIZE);
        let mut seek = self.seek;
        while !buf.is_empty() && seek < self.size {
            let lba = u64::from(self.start_lba) + u64::try_from(seek / blksize).unwrap();
            if self.buf_lba != Some(lba) {
                self.file.read_at(&mut self.buf, lba)?;
                self.buf_lba = Some(lba);
            }

            let start = seek % blksize;
            let end = min(self.size - (seek / blksize) * blksize, blksize);
            seek += buf.write(&self.buf[start..end]).unwrap();
        }

        let bytes = seek - self.seek;
        self.seek = seek;
        Ok(bytes)
    }
}

impl<T: ISO9660Reader> Seek for ISOFileReader<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let seek = match pos {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::End(pos) => self.size as i64 + pos,
            SeekFrom::Current(pos) => self.seek as i64 + pos,
        };

        if seek < 0 {
            Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid seek"))
        } else {
            self.seek = seek as usize;
            Ok(seek as u64)
        }
    }
}
