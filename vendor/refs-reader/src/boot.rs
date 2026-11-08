use crate::bytes::{slice, u32_at, u64_at};
use crate::{corrupt, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootSector {
    pub major: u8,
    pub minor: u8,
    pub sector_size: u32,
    pub cluster_size: u32,
    pub volume_size: u64,
    pub serial_number: u64,
}

impl BootSector {
    pub fn parse(bytes: &[u8], source_length: u64) -> Result<Self> {
        slice(bytes, 0, 512)?;
        if slice(bytes, 3, 4)? != b"ReFS" || slice(bytes, 16, 4)? != b"FSRS" {
            return Err(corrupt("invalid ReFS boot signatures"));
        }
        let major = bytes[40];
        if !(1..=3).contains(&major) {
            return Err(corrupt(format!("unsupported ReFS major version {major}")));
        }
        let sector_size = u32_at(bytes, 32)?;
        let sectors_per_cluster = u32_at(bytes, 36)?;
        let cluster_size = sector_size
            .checked_mul(sectors_per_cluster)
            .ok_or_else(|| corrupt("cluster size overflow"))?;
        if !sector_size.is_power_of_two()
            || !(512..=4096).contains(&sector_size)
            || !cluster_size.is_power_of_two()
            || !(4096..=65536).contains(&cluster_size)
        {
            return Err(corrupt("unsupported ReFS sector / cluster geometry"));
        }
        let volume_size = u64_at(bytes, 24)?
            .checked_mul(u64::from(sector_size))
            .ok_or_else(|| corrupt("volume size overflow"))?;
        if volume_size == 0 || volume_size > source_length {
            return Err(corrupt("ReFS declared size exceeds source or is zero"));
        }
        Ok(Self {
            major,
            minor: bytes[41],
            sector_size,
            cluster_size,
            volume_size,
            serial_number: u64_at(bytes, 56)?,
        })
    }

    pub(crate) fn modern(&self) -> bool {
        self.major >= 2
    }
    pub(crate) fn block_unit(&self) -> u64 {
        if self.modern() {
            u64::from(self.cluster_size)
        } else {
            16384
        }
    }
    pub(crate) fn node_size(&self) -> usize {
        if self.modern() {
            (self.cluster_size as usize).max(16384)
        } else if self.cluster_size == 4096 {
            12288
        } else {
            16384
        }
    }
    pub(crate) fn header_size(&self) -> usize {
        if self.modern() {
            80
        } else {
            48
        }
    }
}
