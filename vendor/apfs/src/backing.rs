//! Bound all object and data reads to container geometry, even on a larger source.
use std::io::{self, Read, Seek, SeekFrom};

pub(crate) struct ContainerReader<R> {
    source: R,
    length: u64,
    position: u64,
}

impl<R> ContainerReader<R> {
    pub(crate) fn new(source: R, length: u64) -> Self {
        Self {
            source,
            length,
            position: 0,
        }
    }
}

impl<R: Read + Seek> Read for ContainerReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let wanted = self
            .length
            .saturating_sub(self.position)
            .min(buffer.len() as u64) as usize;
        if wanted == 0 {
            return Ok(0);
        }
        self.source.seek(SeekFrom::Start(self.position))?;
        let count = self.source.read(&mut buffer[..wanted])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for ContainerReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        self.position = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "APFS container seek out of range",
            )
        })?;
        Ok(self.position)
    }
}
