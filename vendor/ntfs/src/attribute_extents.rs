// SPDX-License-Identifier: MIT OR Apache-2.0
//! Physical storage maps for ordinary and connected NTFS attributes.
use super::*;

/// A physical mapping span. Compressed spans describe encoded storage, not decoded bytes.
#[derive(Clone, Debug)]
pub struct NtfsDataExtent {
    pub logical_offset: u64,
    pub allocated_size: u64,
    pub data_position: NtfsPosition,
}

impl NtfsAttribute<'_, '_> {
    /// All allocated and sparse extents, including connected physical attributes.
    pub fn data_extents<T: Read + Seek>(&self, fs: &mut T) -> Result<Vec<NtfsDataExtent>> {
        let mut extents = Vec::new();
        if self.is_resident() {
            return Ok(extents);
        }
        if let Some(list) = self.list_entries {
            let mut entries = list.clone();
            while let Some(entry) = entries.next(fs) {
                let entry = entry?;
                if entry.raw_type() != self.raw_type() || entry.name() != self.name()? {
                    break;
                }
                let file = entry.to_file(self.file.ntfs(), fs)?;
                let attribute = entry.to_attribute(&file)?;
                attribute.append_extents(fs, &mut extents)?;
            }
        } else {
            self.append_extents(fs, &mut extents)?;
        }
        Ok(extents)
    }

    fn append_extents<T: Read + Seek>(
        &self,
        fs: &mut T,
        extents: &mut Vec<NtfsDataExtent>,
    ) -> Result<()> {
        let NtfsAttributeValue::NonResident(value) = self.raw_value(fs)? else {
            return Err(NtfsError::InvalidAttributeChain {
                reason: "resident continuation extent",
            });
        };
        let mut logical_offset = self
            .lowest_vcn()
            .checked_mul(u64::from(self.file.ntfs().cluster_size()))
            .ok_or(NtfsError::InvalidAttributeChain {
                reason: "VCN overflow",
            })?;
        if extents
            .last()
            .is_some_and(|last| last.logical_offset + last.allocated_size != logical_offset)
        {
            return Err(NtfsError::InvalidAttributeChain {
                reason: "extent gap or overlap",
            });
        }
        for run in value.data_runs() {
            let run = run?;
            if extents.len() >= 1_000_000 {
                return Err(NtfsError::InvalidAttributeChain {
                    reason: "too many data extents",
                });
            }
            extents.push(NtfsDataExtent {
                logical_offset,
                allocated_size: run.allocated_size(),
                data_position: run.data_position(),
            });
            logical_offset = logical_offset.checked_add(run.allocated_size()).ok_or(
                NtfsError::InvalidAttributeChain {
                    reason: "extent span overflow",
                },
            )?;
        }
        Ok(())
    }
}
