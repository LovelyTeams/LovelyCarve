use std::io::{self, Read, Seek, SeekFrom};

use positioned_io2::ReadAt;

use crate::{VdiDisk, checked_seek_position};

pub struct Slice<'a> {
    inner: &'a VdiDisk,
    range: std::ops::Range<u64>,
    position: u64,
}

impl<'a> Slice<'a> {
    pub fn new(inner: &'a VdiDisk, range: std::ops::Range<u64>) -> io::Result<Self> {
        validate_range(inner, &range)?;
        Ok(Self {
            inner,
            range,
            position: 0,
        })
    }

    pub fn len(&self) -> u64 {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
}

impl Read for Slice<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = ReadAt::read_at(self, self.position, buf)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for Slice<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = checked_seek_position(self.position, self.len(), position)?;
        Ok(self.position)
    }
}

impl ReadAt for Slice<'_> {
    fn read_at(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        read_slice_at(self.inner, &self.range, position, buf)
    }
}

pub struct OwnedSlice {
    inner: VdiDisk,
    range: std::ops::Range<u64>,
    position: u64,
}

impl OwnedSlice {
    pub fn new(inner: VdiDisk, range: std::ops::Range<u64>) -> io::Result<Self> {
        validate_range(&inner, &range)?;
        Ok(Self {
            inner,
            range,
            position: 0,
        })
    }

    pub fn into_inner(self) -> VdiDisk {
        self.inner
    }

    pub fn len(&self) -> u64 {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
}

impl Read for OwnedSlice {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = ReadAt::read_at(self, self.position, buf)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for OwnedSlice {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = checked_seek_position(self.position, self.len(), position)?;
        Ok(self.position)
    }
}

impl ReadAt for OwnedSlice {
    fn read_at(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        read_slice_at(&self.inner, &self.range, position, buf)
    }
}

fn validate_range(inner: &VdiDisk, range: &std::ops::Range<u64>) -> io::Result<()> {
    if range.start > range.end || range.end > inner.disk_size() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "VDI slice range is out of bounds",
        ));
    }
    Ok(())
}

fn read_slice_at(
    inner: &VdiDisk,
    range: &std::ops::Range<u64>,
    position: u64,
    buf: &mut [u8],
) -> io::Result<usize> {
    let length = range.end - range.start;
    if position >= length || buf.is_empty() {
        return Ok(0);
    }
    let available = usize::try_from(length - position).unwrap_or(usize::MAX);
    let read_len = buf.len().min(available);
    inner.read_at(range.start + position, &mut buf[..read_len])
}
