//! Standalone exported $MFT inspection, without inventing unavailable volume geometry.
use super::*;
use anyhow::{ensure, Context};
use ntfs_core::{apply_fixup, parse_attributes, AttributeBody, MftRecordHeader};

pub struct NtfsMftMetadataFile {
    file: std::fs::File,
    record_size: usize,
    records: u64,
}

#[derive(Debug, Serialize)]
pub struct NtfsMftVolumeMetadata {
    pub name: Option<String>,
    pub version: Option<(u8, u8)>,
    pub flags: Option<u16>,
}

impl NtfsMftMetadataFile {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        let mut header = [0; 48];
        file.read_exact(&mut header)?;
        let header = MftRecordHeader::parse(&header)?;
        let size = header.allocated_size as usize;
        ensure!(
            (512..=65536).contains(&size) && size.is_power_of_two(),
            "invalid standalone MFT record size"
        );
        ensure!(
            length % size as u64 == 0,
            "standalone MFT has a partial trailing record"
        );
        Ok(Self {
            file,
            record_size: size,
            records: length / size as u64,
        })
    }

    pub fn record_count(&self) -> u64 {
        self.records
    }

    /// Resident volume labels/version from record 3; boot geometry is absent.
    pub fn volume_metadata(&mut self) -> Result<NtfsMftVolumeMetadata> {
        let (bytes, header) = self.read_record(3)?;
        let mut result = NtfsMftVolumeMetadata {
            name: None,
            version: None,
            flags: None,
        };
        for attribute in parse_attributes(
            &bytes[..header.used_size as usize],
            usize::from(header.first_attribute_offset),
        )? {
            let Some(content) = attribute.resident_content(&bytes) else {
                continue;
            };
            match attribute.type_code {
                0x60 => {
                    ensure!(content.len() % 2 == 0, "unaligned MFT volume name");
                    let words: Vec<_> = content
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect();
                    result.name = Some(String::from_utf16_lossy(&words));
                }
                0x70 => {
                    ensure!(content.len() >= 12, "truncated MFT volume information");
                    result.version = Some((content[8], content[9]));
                    result.flags = Some(u16::from_le_bytes([content[10], content[11]]));
                }
                _ => {}
            }
        }
        Ok(result)
    }

    fn read_record(&mut self, number: u64) -> Result<(Vec<u8>, MftRecordHeader)> {
        ensure!(number < self.records, "standalone MFT record out of bounds");
        let mut bytes = vec![0; self.record_size];
        self.file
            .seek(SeekFrom::Start(number * self.record_size as u64))?;
        self.file.read_exact(&mut bytes)?;
        let header = MftRecordHeader::parse(&bytes)?;
        ensure!(!header.is_corrupt(), "MFT record is marked BAAD");
        ensure!(
            header.allocated_size as usize == self.record_size
                && header.used_size <= header.allocated_size
                && usize::from(header.first_attribute_offset) >= 48,
            "invalid standalone MFT record geometry"
        );
        apply_fixup(&mut bytes, 512)?;
        Ok((bytes, header))
    }

    /// Inspect one physical slot. Extension slots remain separately addressable.
    /// Extent byte offsets require the original volume and are deliberately absent.
    pub fn record_metadata(&mut self, number: u64) -> Result<NtfsRecordMetadata> {
        let (bytes, header) = self.read_record(number)?;
        let mut result = NtfsRecordMetadata {
            record_number: number,
            sequence: header.sequence_number,
            allocated: header.is_in_use(),
            directory: header.is_directory(),
            base_record: header.base_record & 0xffff_ffff_ffff,
            base_sequence: (header.base_record >> 48) as u16,
            hard_link_count: header.hard_link_count,
            logfile_sequence_number: header.lsn,
            standard_filetimes: None,
            file_flags: None,
            owner_id: None,
            security_id: None,
            usn: None,
            quota_charged: None,
            filenames: Vec::new(),
            attributes: Vec::new(),
            reparse_tag: None,
            link_target: None,
            reparse_print_name: None,
            reparse_data_hex: None,
            symlink_flags: None,
        };
        for attribute in parse_attributes(
            &bytes[..header.used_size as usize],
            usize::from(header.first_attribute_offset),
        )? {
            let (logical_size, initialized_size, allocated_size, lowest_vcn, exponent) =
                match attribute.body {
                    AttributeBody::Resident { content_length, .. } => (
                        u64::from(content_length),
                        u64::from(content_length),
                        u64::from(content_length),
                        0,
                        0,
                    ),
                    AttributeBody::NonResident {
                        real_size,
                        initialized_size,
                        allocated_size,
                        start_vcn,
                        compression_unit,
                        ..
                    } => (
                        real_size,
                        initialized_size,
                        allocated_size,
                        start_vcn,
                        compression_unit,
                    ),
                };
            let header_bytes =
                &bytes[attribute.offset..attribute.offset + attribute.length as usize];
            let name_offset = usize::from(u16::from_le_bytes([header_bytes[10], header_bytes[11]]));
            let name_size = usize::from(header_bytes[9]) * 2;
            let raw_name = if name_size == 0 {
                &[][..]
            } else {
                header_bytes
                    .get(name_offset..name_offset + name_size)
                    .context("invalid MFT attribute name")?
            };
            result.attributes.push(NtfsAttributeMetadata {
                record_number: number,
                instance: attribute.attribute_id,
                attribute_type: attribute.type_code,
                name: attribute.name.clone().unwrap_or_default(),
                name_utf16le: hex::encode(raw_name),
                flags: attribute.flags,
                resident: !attribute.non_resident,
                logical_size,
                initialized_size,
                allocated_size,
                lowest_vcn,
                compression_unit_exponent: exponent,
                extents: Vec::new(),
            });
            let Some(content) = attribute.resident_content(&bytes) else {
                continue;
            };
            match attribute.type_code {
                0x10 => {
                    let info = ntfs_core::StandardInformation::parse(content)?;
                    result.standard_filetimes = Some([
                        info.created.0,
                        info.modified.0,
                        info.mft_modified.0,
                        info.accessed.0,
                    ]);
                    result.file_flags = Some(info.file_attributes);
                    result.security_id = info.security_id;
                    result.usn = info.usn;
                    if content.len() >= 72 {
                        result.owner_id = Some(u32::from_le_bytes(content[48..52].try_into()?));
                        result.quota_charged =
                            Some(u64::from_le_bytes(content[56..64].try_into()?));
                    }
                }
                0x30 => {
                    let name = ntfs_core::FileName::parse(content)?;
                    let created = ntfs_time_to_rfc3339(name.created.0.into());
                    result.filenames.push(NtfsFilenameMetadata {
                        name: name.name,
                        name_utf16le: hex::encode(&content[66..66 + usize::from(content[64]) * 2]),
                        namespace: name.namespace,
                        parent_record: name.parent.record_number,
                        parent_sequence: name.parent.sequence,
                        timestamps: Mactb {
                            modified: ntfs_time_to_rfc3339(name.modified.0.into()),
                            accessed: ntfs_time_to_rfc3339(name.accessed.0.into()),
                            created: created.clone(),
                            mft_changed: ntfs_time_to_rfc3339(name.mft_modified.0.into()),
                            birth: created,
                        },
                        filetimes: [
                            name.created.0,
                            name.modified.0,
                            name.mft_modified.0,
                            name.accessed.0,
                        ],
                        allocated_size: name.allocated_size,
                        logical_size: name.real_size,
                        file_flags: name.flags,
                    });
                }
                0xc0 => {
                    let point = super::reparse::ReparsePoint::parse(content)?;
                    result.reparse_tag = Some(point.tag);
                    result.link_target = point.link_target()?;
                    result.reparse_print_name = point.print_name()?;
                    result.reparse_data_hex = Some(hex::encode(&point.data));
                    result.symlink_flags = point.symlink_flags()?;
                }
                _ => {}
            }
        }
        Ok(result)
    }

    /// Raw resident values are present in an exported MFT; external cluster bytes are unavailable.
    pub fn read_resident_attribute(
        &mut self,
        number: u64,
        instance: u16,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let (bytes, header) = self.read_record(number)?;
        let attributes = parse_attributes(
            &bytes[..header.used_size as usize],
            usize::from(header.first_attribute_offset),
        )?;
        let attribute = attributes
            .iter()
            .find(|attribute| attribute.attribute_id == instance)
            .context("standalone MFT attribute not found")?;
        let content = attribute
            .resident_content(&bytes)
            .context("nonresident MFT attribute requires original volume cluster data")?;
        ensure!(
            content.len() <= max_bytes,
            "MFT resident attribute exceeds caller budget"
        );
        Ok(content.to_vec())
    }
}
