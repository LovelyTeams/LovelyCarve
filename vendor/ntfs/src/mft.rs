// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bootstrap the MFT run map without recursively resolving MFT attribute lists.
use crate::attribute_value::NtfsAttributeValue;
use crate::io::{Read, Seek, SeekFrom};
use crate::structured_values::NtfsAttributeList;
use crate::{Ntfs, NtfsAttribute, NtfsAttributeType, NtfsError, NtfsFile, Result};
use alloc::{vec, vec::Vec};
use core::num::NonZeroU64;

#[derive(Debug)]
pub(crate) struct MftMap {
    length: u64,
    runs: Vec<(u64, u64, u64)>,
}

impl MftMap {
    fn append<T: Read + Seek>(
        &mut self,
        ntfs: &Ntfs,
        fs: &mut T,
        attribute: &NtfsAttribute,
        expected_vcn: u64,
    ) -> Result<()> {
        let cluster = u64::from(ntfs.cluster_size());
        let mut logical =
            expected_vcn
                .checked_mul(cluster)
                .ok_or(NtfsError::InvalidFileRecordNumber {
                    file_record_number: 0,
                })?;
        if logical != self.runs.last().map_or(0, |run| run.0 + run.2) {
            return Err(NtfsError::InvalidMftMap {
                reason: "discontinuous MFT extents",
            });
        }
        // This value exposes only this record's runs; using attribute lists here would recurse.
        let NtfsAttributeValue::NonResident(value) = attribute.raw_value(fs)? else {
            return Err(NtfsError::InvalidMftMap {
                reason: "resident MFT data",
            });
        };
        for run in value.data_runs() {
            let run = run?;
            let physical = run
                .data_position()
                .value()
                .ok_or(NtfsError::InvalidMftMap {
                    reason: "sparse MFT data",
                })?
                .get();
            let size = run.allocated_size();
            logical = logical.checked_add(size).ok_or(NtfsError::InvalidMftMap {
                reason: "MFT span overflow",
            })?;
            self.runs.push((logical - size, physical, size));
        }
        Ok(())
    }

    fn can_read(&self, number: u64, size: u32) -> bool {
        number
            .checked_add(1)
            .and_then(|end| end.checked_mul(u64::from(size)))
            .is_some_and(|end| {
                end <= self.length && end <= self.runs.last().map_or(0, |run| run.0 + run.2)
            })
    }

    pub(crate) fn read<'n, T: Read + Seek>(
        &self,
        ntfs: &'n Ntfs,
        fs: &mut T,
        number: u64,
    ) -> Result<NtfsFile<'n>> {
        if !self.can_read(number, ntfs.file_record_size()) {
            return Err(NtfsError::InvalidFileRecordNumber {
                file_record_number: number,
            });
        }
        let mut logical = number * u64::from(ntfs.file_record_size());
        let mut data = vec![0; ntfs.file_record_size() as usize];
        let mut filled = 0;
        let mut first = None;
        for &(start, physical, size) in &self.runs {
            if logical < start || logical >= start + size {
                continue;
            }
            let inside = logical - start;
            let position = physical
                .checked_add(inside)
                .ok_or(NtfsError::InvalidMftMap {
                    reason: "MFT offset overflow",
                })?;
            first.get_or_insert(position);
            let count = (size - inside).min((data.len() - filled) as u64) as usize;
            fs.seek(SeekFrom::Start(position))?;
            fs.read_exact(&mut data[filled..filled + count])?;
            filled += count;
            logical += count as u64;
            if filled == data.len() {
                break;
            }
        }
        let position =
            first
                .and_then(NonZeroU64::new)
                .ok_or(NtfsError::InvalidFileRecordNumber {
                    file_record_number: number,
                })?;
        if filled != data.len() {
            return Err(NtfsError::InvalidMftMap {
                reason: "unmapped MFT record",
            });
        }
        NtfsFile::from_record_data(ntfs, data, position, number)
    }
}

pub(crate) fn build<T: Read + Seek>(ntfs: &Ntfs, fs: &mut T) -> Result<MftMap> {
    let position = ntfs
        .mft_position()
        .value()
        .ok_or(NtfsError::InvalidFileRecordNumber {
            file_record_number: 0,
        })?;
    let base = NtfsFile::new(ntfs, fs, position, 0)?;
    let data = base.find_resident_attribute(NtfsAttributeType::Data, Some(""), None)?;
    let mut map = MftMap {
        length: data.value_length(),
        runs: Vec::new(),
    };
    map.append(ntfs, fs, &data, 0)?;
    let mut pending = Vec::new();
    for attribute in base.attributes_raw() {
        let attribute = attribute?;
        if attribute.raw_type() != NtfsAttributeType::AttributeList as u32 {
            continue;
        }
        if attribute.value_length() > 16 * 1024 * 1024 {
            return Err(NtfsError::InvalidMftMap {
                reason: "MFT attribute list exceeds 16 MiB",
            });
        }
        let list: NtfsAttributeList = attribute.structured_value(fs)?;
        let mut entries = list.entries();
        while let Some(entry) = entries.next(fs) {
            let entry = entry?;
            if entry.raw_type() == NtfsAttributeType::Data as u32
                && entry.name().is_empty()
                && entry.lowest_vcn().value() > 0
            {
                pending.push(entry);
            }
        }
    }
    pending.sort_by_key(|entry| entry.lowest_vcn().value());
    for entry in pending {
        let number = entry.base_file_reference().file_record_number();
        // Each continuation record must be reachable through the already established prefix.
        let file = map.read(ntfs, fs, number)?;
        let sequence = entry.base_file_reference().sequence_number();
        if sequence != 0 && sequence != file.sequence_number() {
            return Err(NtfsError::InvalidMftMap {
                reason: "stale MFT extension reference",
            });
        }
        let attribute = entry.to_attribute(&file)?;
        if attribute.name()? != "" {
            return Err(NtfsError::InvalidMftMap {
                reason: "named MFT continuation",
            });
        }
        map.append(ntfs, fs, &attribute, entry.lowest_vcn().value() as u64)?;
    }
    if map.length == 0 {
        map.length = map.runs.last().map_or(0, |run| run.0 + run.2);
    }
    if map.runs.last().map_or(0, |run| run.0 + run.2) < map.length {
        return Err(NtfsError::InvalidMftMap {
            reason: "incomplete MFT run map",
        });
    }
    Ok(map)
}
