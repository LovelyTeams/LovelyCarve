use crate::disk::DiskPartition;
use crate::fat::Fat;
use crate::param::Params;
use std::cmp::min;
use thiserror::Error;

/// Struct to read all data in a cluster chain.
pub struct ClustersReader<D, P> {
    disk: D,
    params: P,
    chain: Vec<usize>,
    data_length: u64,
    offset: u64,
}

impl<D, P: AsRef<Params>> ClustersReader<D, P> {
    pub fn new(
        disk: D,
        params: P,
        fat: &Fat,
        first_cluster: usize,
        data_length: Option<u64>,
        no_fat_chain: Option<bool>,
    ) -> Result<Self, NewError> {
        if first_cluster < 2 {
            return Err(NewError::InvalidFirstCluster);
        }

        // Get cluster chain.
        let cluster_size = params.as_ref().cluster_size();
        let (chain, data_length) = if no_fat_chain.unwrap_or(false) {
            // If the NoFatChain bit is 1 then DataLength must not be zero.
            let data_length = match data_length {
                Some(v) if v > 0 => v,
                _ => return Err(NewError::InvalidDataLength),
            };

            // FIXME: Use div_ceil once https://github.com/rust-lang/rust/issues/88581 stabilized.
            let count = data_length.div_ceil(cluster_size);
            if count > params.as_ref().cluster_count as u64
                || first_cluster as u64 + count > params.as_ref().cluster_count as u64 + 2
            {
                return Err(NewError::InvalidClusterChain);
            }
            let chain: Vec<usize> = (first_cluster..(first_cluster + count as usize)).collect();

            (chain, data_length)
        } else {
            let chain = fat.cluster_chain(first_cluster)?;

            if chain.is_empty() {
                return Err(NewError::InvalidFirstCluster);
            }

            let data_length = match data_length {
                Some(v) => {
                    if v > cluster_size * chain.len() as u64 {
                        return Err(NewError::InvalidDataLength);
                    } else {
                        v
                    }
                }
                None => {
                    params.as_ref().bytes_per_sector
                        * (params.as_ref().sectors_per_cluster * chain.len() as u64)
                }
            };

            (chain, data_length)
        };

        Ok(Self {
            disk,
            params,
            chain,
            data_length,
            offset: 0,
        })
    }

    pub fn extents(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        let cluster_size = self.params.as_ref().cluster_size();
        self.chain
            .iter()
            .enumerate()
            .map(move |(index, cluster)| {
                let offset = self.params.as_ref().cluster_offset(*cluster).unwrap_or(0);
                let length = self
                    .data_length
                    .saturating_sub(index as u64 * cluster_size)
                    .min(cluster_size);
                (offset, length)
            })
            .filter(|(_, length)| *length != 0)
    }

    pub fn physical_offset(&self) -> Option<u64> {
        self.params
            .as_ref()
            .cluster_offset(self.cluster())
            .map(|base| base + self.offset % self.params.as_ref().cluster_size())
    }

    pub fn cluster(&self) -> usize {
        self.chain
            .get((self.offset / self.params.as_ref().cluster_size()) as usize)
            .copied()
            .unwrap_or(0)
    }
}

impl<D, P> ClustersReader<D, P> {
    pub fn data_length(&self) -> u64 {
        self.data_length
    }

    pub fn seek(&mut self, off: u64) -> bool {
        if off > self.data_length {
            return false;
        }

        self.offset = off;
        true
    }

    pub fn rewind(&mut self) {
        self.offset = 0;
    }

    pub fn stream_position(&self) -> u64 {
        self.offset
    }
}

impl<D: DiskPartition, P: AsRef<Params>> ClustersReader<D, P> {
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Error, ErrorKind};

        // Check if the actual read is required.
        if buf.is_empty() || self.offset == self.data_length {
            return Ok(0);
        }

        // Get remaining data in the current cluster.
        let params = self.params.as_ref();
        let cluster_size = params.cluster_size();
        let cluster_remaining = cluster_size - self.offset % cluster_size;
        let remaining = min(cluster_remaining, self.data_length - self.offset);

        // Get the offset in the partition.
        let cluster = self.chain[(self.offset / cluster_size) as usize];
        let offset = match params.cluster_offset(cluster) {
            Some(v) => v + self.offset % cluster_size,
            None => {
                return Err(Error::new(
                    ErrorKind::Other,
                    format!("cluster #{cluster} is not available"),
                ));
            }
        };

        // Read image.
        let amount = min(buf.len(), remaining as usize);

        if let Err(e) = self.disk.read_exact(offset, &mut buf[..amount]) {
            return Err(Error::new(ErrorKind::Other, Box::new(e)));
        }

        self.offset += amount as u64;

        Ok(amount)
    }

    pub fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<(), std::io::Error> {
        while !buf.is_empty() {
            let n = self.read(buf)?;

            if n == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }

            buf = &mut buf[n..];
        }

        Ok(())
    }
}

/// Represents an error for [`new()`][ClustersReader::new()].
#[derive(Debug, Error)]
pub enum NewError {
    #[error("invalid, out-of-bounds or cyclic cluster chain")]
    InvalidClusterChain,
    #[error("first cluster is not valid")]
    InvalidFirstCluster,

    #[error("data length is not valid")]
    InvalidDataLength,
}
