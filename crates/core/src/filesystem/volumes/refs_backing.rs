//! Exact partition-relative byte adapters for the pure Rust ReFS reader.
use crate::evidence::crypto::bitlocker::UnlockedVolume;
use crate::evidence::image::DiskImage;
use refs_reader::ReadAt;
use std::{io, sync::Arc};

pub(super) struct ImageBacking {
    pub image: Arc<DiskImage>,
    pub base_offset: u64,
    pub length_bytes: u64,
}

impl ReadAt for ImageBacking {
    fn len(&self) -> u64 {
        self.length_bytes
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        check_range(offset, buffer.len(), self.length_bytes)?;
        let absolute = self
            .base_offset
            .checked_add(offset)
            .ok_or_else(|| io::Error::other("ReFS image offset overflow"))?;
        let count = self
            .image
            .read_into(absolute, buffer)
            .map_err(io::Error::other)?;
        if count != buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short ReFS evidence read",
            ));
        }
        Ok(())
    }
}

pub struct BitlockerBacking {
    pub unlocked: Arc<UnlockedVolume>,
    pub length_bytes: u64,
}

impl ReadAt for BitlockerBacking {
    fn len(&self) -> u64 {
        self.length_bytes
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        check_range(offset, buffer.len(), self.length_bytes)?;
        let count = self
            .unlocked
            .read_at(offset, buffer)
            .map_err(io::Error::other)?;
        if count != buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short decrypted ReFS read",
            ));
        }
        Ok(())
    }
}

fn check_range(offset: u64, length: usize, volume_size: u64) -> io::Result<()> {
    if offset
        .checked_add(length as u64)
        .is_none_or(|end| end > volume_size)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ReFS read exceeds partition",
        ));
    }
    Ok(())
}
