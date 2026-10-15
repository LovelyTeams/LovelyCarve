use crate::{invalid, GzipDecoder, Member, Options};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Seekable logical GZIP data. Opening scans and verifies every member with
/// bounded working memory, keeping only member metadata. The backing source
/// must remain unchanged throughout this handle's lifetime.
pub struct GzipFile<R> {
    decoder: GzipDecoder<R>,
    members: Vec<Member>,
    size: u64,
    position: u64,
    pending_skip: u64,
    failed: bool,
}

impl GzipFile<File> {
    /// Open a host file read-only, with default resource limits.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_reader(File::open(path)?)
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: Options) -> io::Result<Self> {
        Self::from_reader_with_options(File::open(path)?, options)
    }
}

impl<R: Read + Seek> GzipFile<R> {
    pub fn from_reader(reader: R) -> io::Result<Self> {
        Self::from_reader_with_options(reader, Options::default())
    }

    /// Source offsets are relative to byte zero of `reader`, which is rewound.
    pub fn from_reader_with_options(mut reader: R, options: Options) -> io::Result<Self> {
        options.check_cancelled()?;
        reader.seek(SeekFrom::Start(0))?;
        let mut decoder = GzipDecoder::with_options(reader, options);
        decoder.finish()?;
        let size = decoder.uncompressed_size();
        let members = std::mem::take(&mut decoder.members);
        decoder.restart(0, 0)?;
        Ok(Self {
            decoder,
            members,
            size,
            position: 0,
            pending_skip: 0,
            failed: false,
        })
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn member(&self, index: usize) -> Option<&Member> {
        self.members.get(index)
    }

    pub fn uncompressed_size(&self) -> u64 {
        self.size
    }

    pub fn offset(&self) -> u64 {
        self.position
    }

    /// True after a read error; this handle then rejects further reads/seeks.
    pub fn has_failed(&self) -> bool {
        self.failed
    }

    /// Like libgzipf read-at-offset, updates the logical cursor after reading.
    pub fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        self.seek(SeekFrom::Start(offset))?;
        self.read(output)
    }
    fn read_inner(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.decoder.input.options.check_cancelled()?;
        if self.failed || self.decoder.has_failed() {
            return Err(invalid(
                "GZIP file cannot continue after an earlier read error",
            ));
        }
        if self.position >= self.size {
            return Ok(0);
        }
        let mut discard = [0; 64 * 1024];
        while self.pending_skip != 0 {
            let count = self.pending_skip.min(discard.len() as u64) as usize;
            let read = self.decoder.read(&mut discard[..count])?;
            if read == 0 {
                return Err(invalid(
                    "GZIP source changed or became truncated during seek",
                ));
            }
            self.pending_skip -= read as u64;
        }
        let count = (self.size - self.position).min(output.len() as u64) as usize;
        let read = self.decoder.read(&mut output[..count])?;
        if read == 0 {
            return Err(invalid(
                "GZIP source changed or became truncated during read",
            ));
        }
        self.position += read as u64;
        if self.position == self.size {
            // Also validate empty trailing members and catch source growth.
            let mut extra = [0];
            if self.decoder.read(&mut extra)? != 0 {
                return Err(invalid("GZIP source grew after opening"));
            }
        }
        Ok(read)
    }
}

impl<R: Read + Seek> Read for GzipFile<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let result = self.read_inner(output);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl<R: Read + Seek> Seek for GzipFile<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };
        let target = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "GZIP seek offset is negative or exceeds u64",
            )
        })?;
        self.decoder.input.options.check_cancelled()?;
        if self.failed || self.decoder.has_failed() {
            return Err(invalid("GZIP file cannot seek after an earlier error"));
        }
        if target == self.position {
            return Ok(target);
        }
        if target < self.size {
            // Skip zero-length members at boundaries, retaining them in metadata.
            let index = self.members.partition_point(|member| {
                member.uncompressed_offset + member.uncompressed_size <= target
            });
            let member = self
                .members
                .get(index)
                .ok_or_else(|| invalid("GZIP member index is incomplete"))?;
            self.decoder
                .restart(member.header_offset, member.uncompressed_offset)?;
            self.pending_skip = target - member.uncompressed_offset;
        } else {
            self.pending_skip = 0;
        }
        self.position = target;
        Ok(target)
    }
}
