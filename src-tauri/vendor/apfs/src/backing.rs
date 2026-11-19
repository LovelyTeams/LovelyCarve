//! Bound all object and data reads to container geometry, even on a larger source.
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

const BLOCK_CACHE_CAP: usize = 2048;
const MAX_CACHED_READ_LEN: usize = 65_536;

pub(crate) struct ContainerReader<R> {
    source: R,
    length: u64,
    position: u64,
    cache: HashMap<(u64, usize), Vec<u8>>,
}

impl<R> ContainerReader<R> {
    pub(crate) fn new(source: R, length: u64) -> Self {
        Self {
            source,
            length,
            position: 0,
            cache: HashMap::new(),
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
        if wanted <= MAX_CACHED_READ_LEN {
            if let Some(cached) = self.cache.get(&(self.position, wanted)) {
                buffer[..cached.len()].copy_from_slice(cached);
                self.position += cached.len() as u64;
                return Ok(cached.len());
            }
        }
        self.source.seek(SeekFrom::Start(self.position))?;
        let count = self.source.read(&mut buffer[..wanted])?;
        if count > 0 && count == wanted && wanted <= MAX_CACHED_READ_LEN {
            if self.cache.len() >= BLOCK_CACHE_CAP {
                self.cache.clear();
            }
            self.cache
                .insert((self.position, wanted), buffer[..count].to_vec());
        }
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
