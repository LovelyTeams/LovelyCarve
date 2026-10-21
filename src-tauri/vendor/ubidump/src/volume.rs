use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};

/// A bounded UBI source. Implementers must return exactly the requested bytes,
/// or an error, without reopening paths or suppressing source read failures.
pub trait ReadAt {
    fn size(&self) -> u64;
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>>;
}

pub(crate) fn read_exact(source: &dyn ReadAt, offset: u64, length: usize) -> Result<Vec<u8>> {
    ensure!(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= source.size()),
        "UBI read outside source"
    );
    let bytes = source.read_at(offset, length)?;
    ensure!(
        bytes.len() == length,
        "UBI source short read at {offset:#x}"
    );
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub offset: u64,
    pub sequence: u64,
    pub data_size: u32,
    pub used_ebs: u32,
}

/// One volume's logical-to-physical map; offsets remain relative to the UBI
/// source, never to a temporary extracted file. Missing dynamic LEBs are erased.
#[derive(Debug, Clone)]
pub struct UbiVolume {
    pub id: u32,
    pub name: String,
    pub length: u64,
    pub leb_size: u32,
    pub(crate) static_volume: bool,
    pub(crate) blocks: BTreeMap<u32, Block>,
}

impl UbiVolume {
    pub fn read_at(&self, source: &dyn ReadAt, offset: u64, length: usize) -> Result<Vec<u8>> {
        let length = (length as u64).min(self.length.saturating_sub(offset)) as usize;
        let mut output = vec![0xff; length];
        let mut done = 0;
        while done < length {
            let logical = offset
                .checked_add(done as u64)
                .context("UBI offset overflow")?;
            let number = u32::try_from(logical / u64::from(self.leb_size))?;
            let within = logical % u64::from(self.leb_size);
            let count = (length - done).min((u64::from(self.leb_size) - within) as usize);
            if let Some(block) = self.blocks.get(&number) {
                ensure!(
                    !self.static_volume || within + count as u64 <= u64::from(block.data_size),
                    "UBI static data is truncated"
                );
                let bytes = read_exact(
                    source,
                    block
                        .offset
                        .checked_add(within)
                        .context("UBI physical offset overflow")?,
                    count,
                )?;
                output[done..done + count].copy_from_slice(&bytes);
            } else if self.static_volume {
                bail!("UBI static volume {} is missing LEB {number}", self.id);
            }
            done += count;
        }
        Ok(output)
    }
}

/// Seekable view over a logical volume, suitable for nested filesystem readers.
pub struct UbiReader<R> {
    pub source: R,
    pub volume: Arc<UbiVolume>,
    position: u64,
}

impl<R> UbiReader<R> {
    pub fn new(source: R, volume: Arc<UbiVolume>) -> Self {
        Self {
            source,
            volume,
            position: 0,
        }
    }
}

impl<R: ReadAt> Read for UbiReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes = self
            .volume
            .read_at(&self.source, self.position, buffer.len())
            .map_err(io::Error::other)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }
}

impl<R> Seek for UbiReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.volume.length) + i128::from(delta),
        };
        self.position = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "UBI seek outside address space",
            )
        })?;
        Ok(self.position)
    }
}
