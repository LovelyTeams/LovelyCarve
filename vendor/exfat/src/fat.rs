use crate::disk::DiskPartition;
use crate::param::Params;
use byteorder::{ByteOrder, LE};
use core::fmt::Debug;
use thiserror::Error;

pub(crate) struct Fat {
    entries: Vec<u32>,
}

impl Fat {
    pub fn load<P: DiskPartition>(
        params: &Params,
        partition: &P,
        index: usize,
    ) -> Result<Self, LoadError<P>> {
        // Get FAT region offset.
        let sector = match params.fat_length.checked_mul(index as u64) {
            Some(v) => match params.fat_offset.checked_add(v) {
                Some(v) => v,
                None => return Err(LoadError::InvalidFatOffset),
            },
            None => return Err(LoadError::InvalidFatLength),
        };

        let offset = match sector.checked_mul(params.bytes_per_sector) {
            Some(v) => v,
            None => return Err(LoadError::InvalidFatOffset),
        };

        // Load entries.
        let count = params
            .cluster_count
            .checked_add(2)
            .ok_or(LoadError::InvalidFatLength)?;
        if (count as u64) * 4 > params.fat_length.saturating_mul(params.bytes_per_sector) {
            return Err(LoadError::InvalidFatLength);
        }
        let mut data = vec![0u8; count * 4];

        if let Err(e) = partition.read_exact(offset, &mut data) {
            return Err(LoadError::ReadFailed(offset, e));
        }

        // Convert each entry from little endian to native endian.
        let mut entries = vec![0u32; count];

        LE::read_u32_into(&data, &mut entries);

        Ok(Self { entries })
    }

    pub fn cluster_chain(&self, first: usize) -> Result<Vec<usize>, crate::cluster::NewError> {
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut cluster = first;
        loop {
            if cluster < 2 || cluster >= self.entries.len() || !visited.insert(cluster) {
                return Err(crate::cluster::NewError::InvalidClusterChain);
            }
            let next = self.entries[cluster];
            if next == 0 || next == 1 || next == 0xfffffff7 {
                return Err(crate::cluster::NewError::InvalidClusterChain);
            }
            chain.push(cluster);
            if next >= 0xfffffff8 {
                return Ok(chain);
            }
            cluster = next as usize;
        }
    }
}

/// Represents an error for [`Fat::load()`].
#[derive(Error)]
pub enum LoadError<P: DiskPartition> {
    #[error("invalid FatLength")]
    InvalidFatLength,

    #[error("invalid FatOffset")]
    InvalidFatOffset,

    #[error("cannot read the data at {0:#x}")]
    ReadFailed(u64, #[source] P::Err),
}

impl<P: DiskPartition> Debug for LoadError<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFatLength => write!(f, "InvalidFatLength"),
            Self::InvalidFatOffset => write!(f, "InvalidFatOffset"),
            Self::ReadFailed(arg0, arg1) => {
                f.debug_tuple("ReadFailed").field(arg0).field(arg1).finish()
            }
        }
    }
}
