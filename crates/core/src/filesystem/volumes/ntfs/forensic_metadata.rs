//! Public read-only metadata access, preserving both SI/FN times and raw attributes.
use super::*;
use anyhow::{ensure, Context};

#[derive(Debug, Serialize)]
pub struct NtfsVolumeMetadata {
    pub sector_size: u16,
    pub cluster_size: u32,
    pub record_size: u32,
    pub size: u64,
    pub serial_number: u64,
    pub name: Option<String>,
    pub version: (u8, u8),
    pub flags: u16,
    pub mft_records: u64,
    pub upcase_available: bool,
}

#[derive(Debug, Serialize)]
pub struct NtfsAttributeMetadata {
    pub record_number: u64,
    pub instance: u16,
    pub attribute_type: u32,
    pub name: String,
    pub name_utf16le: String,
    pub flags: u16,
    pub resident: bool,
    pub logical_size: u64,
    pub initialized_size: u64,
    pub allocated_size: u64,
    pub lowest_vcn: u64,
    pub compression_unit_exponent: u16,
    pub extents: Vec<NtfsStreamExtent>,
}

/// Volume-relative physical storage, including sparse holes and encoded compressed spans.
#[derive(Debug, Serialize)]
pub struct NtfsStreamExtent {
    pub logical_offset: u64,
    pub storage_length: u64,
    pub volume_offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct NtfsFilenameMetadata {
    pub name: String,
    pub name_utf16le: String,
    pub namespace: u8,
    pub parent_record: u64,
    pub parent_sequence: u16,
    pub timestamps: Mactb,
    /// Original 100 ns FILETIME integers, in created/modified/changed/accessed order.
    pub filetimes: [u64; 4],
    pub allocated_size: u64,
    pub logical_size: u64,
    pub file_flags: u32,
}

#[derive(Debug, Serialize)]
pub struct NtfsRecordMetadata {
    pub record_number: u64,
    pub sequence: u16,
    pub allocated: bool,
    pub directory: bool,
    pub base_record: u64,
    pub base_sequence: u16,
    pub hard_link_count: u16,
    pub logfile_sequence_number: u64,
    pub standard_filetimes: Option<[u64; 4]>,
    pub file_flags: Option<u32>,
    pub owner_id: Option<u32>,
    pub security_id: Option<u32>,
    pub usn: Option<u64>,
    pub quota_charged: Option<u64>,
    pub filenames: Vec<NtfsFilenameMetadata>,
    pub attributes: Vec<NtfsAttributeMetadata>,
    pub reparse_tag: Option<u32>,
    pub link_target: Option<String>,
    pub reparse_print_name: Option<String>,
    pub reparse_data_hex: Option<String>,
    pub symlink_flags: Option<u32>,
}

impl NtfsVolume<'_> {
    pub fn volume_metadata(&self) -> Result<NtfsVolumeMetadata> {
        let mut fs = self.reader();
        let info = self.ntfs.volume_info(&mut fs)?;
        Ok(NtfsVolumeMetadata {
            sector_size: self.ntfs.sector_size(),
            cluster_size: self.ntfs.cluster_size(),
            record_size: self.ntfs.file_record_size(),
            size: self.ntfs.size(),
            serial_number: self.ntfs.serial_number(),
            name: self
                .ntfs
                .volume_name(&mut fs)
                .transpose()?
                .map(|name| name.name().to_string_lossy()),
            version: (info.major_version(), info.minor_version()),
            flags: info.flags().bits(),
            mft_records: self.mft_total_records()?,
            upcase_available: self.ntfs.has_upcase_table(),
        })
    }

    /// Metadata does not sample file content, so encrypted/offline files remain inspectable.
    pub fn record_metadata(&self, record_number: u64) -> Result<NtfsRecordMetadata> {
        let mut fs = self.reader();
        let file = self.ntfs.file(&mut fs, record_number)?;
        let info = match file.info() {
            Ok(info) => Some(info),
            Err(ntfs::NtfsError::AttributeNotFound { .. }) => None,
            Err(err) => return Err(err.into()),
        };
        let mut attributes = Vec::new();
        let mut filenames = Vec::new();
        let mut iterator = file.attributes();
        while let Some(item) = iterator.next(&mut fs) {
            let item = item?;
            let attribute = item.to_attribute()?;
            let name = attribute.name()?;
            let storage = attribute.storage_info()?;
            attributes.push(NtfsAttributeMetadata {
                record_number: attribute.file_record_number(),
                instance: attribute.instance(),
                attribute_type: attribute.raw_type(),
                name: name.to_string_lossy(),
                name_utf16le: hex::encode(name.0),
                flags: attribute.raw_flags(),
                resident: attribute.is_resident(),
                logical_size: attribute.value_length(),
                initialized_size: storage
                    .map_or(attribute.value_length(), |info| info.initialized_size),
                allocated_size: storage
                    .map_or(attribute.value_length(), |info| info.allocated_size),
                lowest_vcn: attribute.lowest_vcn(),
                compression_unit_exponent: storage.map_or(0, |info| info.compression_unit_exponent),
                extents: attribute
                    .data_extents(&mut fs)?
                    .into_iter()
                    .map(|extent| NtfsStreamExtent {
                        logical_offset: extent.logical_offset,
                        storage_length: extent.allocated_size,
                        volume_offset: extent.data_position.value().map(|position| position.get()),
                    })
                    .collect(),
            });
            if attribute.raw_type() == NtfsAttributeType::FileName as u32 {
                let name: NtfsFileName = attribute.structured_value(&mut fs)?;
                filenames.push(NtfsFilenameMetadata {
                    name: name.name().to_string_lossy(),
                    name_utf16le: hex::encode(name.name().0),
                    namespace: name.namespace() as u8,
                    parent_record: name.parent_directory_reference().file_record_number(),
                    parent_sequence: name.parent_directory_reference().sequence_number(),
                    timestamps: file_name_timestamps(&name),
                    filetimes: [
                        name.creation_time().nt_timestamp(),
                        name.modification_time().nt_timestamp(),
                        name.mft_record_modification_time().nt_timestamp(),
                        name.access_time().nt_timestamp(),
                    ],
                    allocated_size: name.allocated_size(),
                    logical_size: name.data_size(),
                    file_flags: name.file_attributes().bits(),
                });
            }
        }
        let point = super::reparse::read_reparse(&file, &mut fs)?;
        Ok(NtfsRecordMetadata {
            record_number,
            sequence: file.sequence_number(),
            allocated: file.flags().contains(NtfsFileFlags::IN_USE),
            directory: file.is_directory(),
            base_record: file.base_file_reference().file_record_number(),
            base_sequence: file.base_file_reference().sequence_number(),
            hard_link_count: file.hard_link_count(),
            logfile_sequence_number: file.logfile_sequence_number(),
            standard_filetimes: info.as_ref().map(|info| {
                [
                    info.creation_time().nt_timestamp(),
                    info.modification_time().nt_timestamp(),
                    info.mft_record_modification_time().nt_timestamp(),
                    info.access_time().nt_timestamp(),
                ]
            }),
            file_flags: info.as_ref().map(|info| info.file_attributes().bits()),
            owner_id: info.as_ref().and_then(NtfsStandardInformation::owner_id),
            security_id: info.as_ref().and_then(NtfsStandardInformation::security_id),
            usn: info.as_ref().and_then(NtfsStandardInformation::usn),
            quota_charged: info
                .as_ref()
                .and_then(NtfsStandardInformation::quota_charged),
            filenames,
            attributes,
            reparse_tag: point.as_ref().map(|point| point.tag),
            link_target: point
                .as_ref()
                .map(|point| point.link_target())
                .transpose()?
                .flatten(),
            reparse_print_name: point
                .as_ref()
                .map(|point| point.print_name())
                .transpose()?
                .flatten(),
            reparse_data_hex: point.as_ref().map(|point| hex::encode(&point.data)),
            symlink_flags: point
                .as_ref()
                .map(|point| point.symlink_flags())
                .transpose()?
                .flatten(),
        })
    }

    /// Read a physical attribute header's value by its record and local instance.
    /// This preserves encoded WOF/LZNT1 data for forensic inspection; file reads use logical decoding.
    pub fn read_attribute_value(
        &self,
        record_number: u64,
        instance: u16,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let mut fs = self.reader();
        let file = self.ntfs.file(&mut fs, record_number)?;
        let attribute = file
            .attributes_raw()
            .find_map(|result| match result {
                Ok(attribute) if attribute.instance() == instance => Some(Ok(attribute)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .context("NTFS physical attribute not found")??;
        ensure!(
            attribute.value_length() <= max_bytes as u64,
            "NTFS attribute exceeds caller budget"
        );
        let mut value = attribute.value(&mut fs)?;
        let mut bytes = vec![0; attribute.value_length() as usize];
        value.read_exact(&mut fs, &mut bytes)?;
        Ok(bytes)
    }
}
