use crate::{invalid, Options};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

pub(crate) struct Input<R> {
    pub reader: BufReader<R>,
    pub offset: u64,
    pub options: Options,
}

impl<R: Read> Input<R> {
    pub fn new(reader: R, options: Options) -> Self {
        Self {
            reader: BufReader::with_capacity(64 * 1024, reader),
            offset: 0,
            options,
        }
    }

    pub fn consume(&mut self, count: usize) -> io::Result<()> {
        let next = self
            .offset
            .checked_add(count as u64)
            .ok_or_else(|| invalid("GZIP source offset overflow"))?;
        if next > self.options.max_compressed_bytes {
            return Err(invalid("GZIP compressed byte limit exceeded"));
        }
        self.reader.consume(count);
        self.offset = next;
        Ok(())
    }

    pub fn exact(&mut self, mut output: &mut [u8]) -> io::Result<()> {
        while !output.is_empty() {
            self.options.check_cancelled()?;
            let available = match self.reader.fill_buf() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            let count = available.len().min(output.len());
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("truncated GZIP structure at source offset {}", self.offset),
                ));
            }
            output[..count].copy_from_slice(&available[..count]);
            self.consume(count)?;
            output = &mut output[count..];
        }
        Ok(())
    }

    pub fn at_end(&mut self) -> io::Result<bool> {
        loop {
            self.options.check_cancelled()?;
            match self.reader.fill_buf() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return Ok(result?.is_empty()),
            }
        }
    }
}

impl<R: Read + Seek> Input<R> {
    pub fn seek(&mut self, offset: u64) -> io::Result<()> {
        self.options.check_cancelled()?;
        self.reader.seek(SeekFrom::Start(offset))?;
        self.offset = offset;
        Ok(())
    }
}
