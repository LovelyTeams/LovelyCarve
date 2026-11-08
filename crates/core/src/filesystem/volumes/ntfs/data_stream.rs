//! Logical NTFS streams with one bounded decompressed unit per reader.
use super::*;
use anyhow::{ensure, Context};
use ntfs::{attribute_value::NtfsAttributeValue, NtfsAttribute};

pub(super) struct DataStream<'n, 'f> {
    value: NtfsAttributeValue<'n, 'f>,
    length: u64,
    initialized: u64,
    position: u64,
    cluster_size: usize,
    unit_size: Option<usize>,
    cached_unit: Option<u64>,
    decoded: Vec<u8>,
}

#[cfg(test)]
#[path = "data_stream_tests.rs"]
mod tests;

impl<'n, 'f> DataStream<'n, 'f> {
    pub(super) fn new<T: Read + Seek>(
        attribute: &NtfsAttribute<'n, 'f>,
        fs: &mut T,
        cluster_size: u32,
        record_number: u64,
    ) -> Result<Self> {
        reject_unreadable_attribute(attribute.flags(), record_number)?;
        ensure!(
            attribute.raw_flags() & 0xff <= 1,
            "unsupported NTFS compression format {}",
            attribute.raw_flags() & 0xff
        );
        let length = attribute.value_length();
        let storage = attribute.storage_info()?;
        let initialized = storage.map_or(length, |info| info.initialized_size);
        ensure!(
            initialized <= length,
            "NTFS initialized data exceeds logical size"
        );
        let unit_size = if attribute.flags().contains(NtfsAttributeFlags::COMPRESSED)
            && !attribute.is_resident()
        {
            let info = storage.context("missing NTFS compression geometry")?;
            let exponent = if info.compression_unit_exponent == 0 {
                4
            } else {
                info.compression_unit_exponent
            };
            let clusters = 1u32
                .checked_shl(u32::from(exponent))
                .context("NTFS compression exponent overflow")?;
            let size = cluster_size
                .checked_mul(clusters)
                .context("NTFS compression unit overflow")?;
            ensure!(
                (512..=4096).contains(&cluster_size) && cluster_size.is_power_of_two(),
                "unsupported NTFS compressed cluster size"
            );
            ensure!(size <= 1024 * 1024, "NTFS compression unit exceeds 1 MiB");
            ensure!(
                info.allocated_size % u64::from(cluster_size) == 0,
                "unaligned NTFS compressed allocation"
            );
            Some(size as usize)
        } else {
            None
        };
        let value = if unit_size.is_some() {
            attribute.raw_value(fs)?
        } else {
            attribute.value(fs)?
        };
        Ok(Self {
            value,
            length,
            initialized,
            position: 0,
            cluster_size: cluster_size as usize,
            unit_size,
            cached_unit: None,
            decoded: Vec::new(),
        })
    }

    pub(super) fn len(&self) -> u64 {
        self.length
    }

    pub(super) fn seek<T: Read + Seek>(&mut self, fs: &mut T, position: SeekFrom) -> Result<u64> {
        let SeekFrom::Start(offset) = position else {
            anyhow::bail!("logical NTFS stream requires an absolute seek");
        };
        self.position = offset;
        if self.unit_size.is_none() {
            self.value.seek(fs, position)?;
        }
        Ok(offset)
    }

    pub(super) fn read<T: Read + Seek>(&mut self, fs: &mut T, buffer: &mut [u8]) -> Result<usize> {
        let wanted = self
            .length
            .saturating_sub(self.position)
            .min(buffer.len() as u64) as usize;
        let mut filled = 0;
        while filled < wanted {
            if self.position >= self.initialized {
                buffer[filled..wanted].fill(0);
                self.position += (wanted - filled) as u64;
                break;
            }
            let initialized =
                (self.initialized - self.position).min((wanted - filled) as u64) as usize;
            let read = if let Some(unit_size) = self.unit_size {
                let unit = self.position / unit_size as u64;
                if self.cached_unit != Some(unit) {
                    self.load_unit(fs, unit, unit_size)?;
                }
                let offset = (self.position % unit_size as u64) as usize;
                let read = initialized.min(unit_size - offset);
                buffer[filled..filled + read].copy_from_slice(&self.decoded[offset..offset + read]);
                read
            } else {
                let read = self
                    .value
                    .read(fs, &mut buffer[filled..filled + initialized])?;
                ensure!(
                    read != 0,
                    "truncated NTFS data stream at byte {}",
                    self.position
                );
                read
            };
            filled += read;
            self.position += read as u64;
        }
        Ok(wanted)
    }

    fn load_unit<T: Read + Seek>(&mut self, fs: &mut T, unit: u64, unit_size: usize) -> Result<()> {
        let start = unit
            .checked_mul(unit_size as u64)
            .context("NTFS unit offset overflow")?;
        self.value.seek(fs, SeekFrom::Start(start))?;
        let mapped = self.value.len().saturating_sub(start).min(unit_size as u64) as usize;
        ensure!(mapped != 0, "missing NTFS compression unit {unit}");
        let mut stored = Vec::with_capacity(unit_size);
        let mut cluster = vec![0; self.cluster_size];
        let mut sparse = false;
        for _ in 0..mapped / self.cluster_size {
            // A raw value advances to the next run on its next read, so read
            // one byte before querying the backing position at a run boundary.
            ensure!(
                self.value.read(fs, &mut cluster[..1])? == 1,
                "truncated NTFS compression unit {unit}"
            );
            let allocated = self.value.data_position().value().is_some();
            ensure!(
                !sparse || !allocated,
                "allocated NTFS clusters after sparse unit padding"
            );
            let mut filled = 1;
            while filled < cluster.len() {
                let read = self.value.read(fs, &mut cluster[filled..])?;
                ensure!(read != 0, "truncated NTFS compression unit {unit}");
                filled += read;
            }
            if allocated {
                stored.extend_from_slice(&cluster);
            } else {
                sparse = true;
            }
        }
        let required = self.initialized.saturating_sub(start).min(unit_size as u64) as usize;
        let mut decoded = if stored.is_empty() {
            vec![0; unit_size]
        } else if sparse {
            let decoded = super::lznt1::decompress(&stored, unit_size)
                .map_err(|error| anyhow!("NTFS LZNT1 compression unit {unit}: {error:#}"))?;
            ensure!(
                decoded.len() >= required || stored.iter().all(|byte| *byte == 0),
                "short decoded NTFS compression unit {unit}"
            );
            decoded
        } else {
            stored
        };
        ensure!(
            decoded.len() >= required || sparse,
            "short raw NTFS compression unit {unit}"
        );
        decoded.resize(unit_size, 0);
        self.decoded = decoded;
        self.cached_unit = Some(unit);
        Ok(())
    }
}
