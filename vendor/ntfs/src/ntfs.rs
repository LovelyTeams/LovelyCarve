// Copyright 2021-2026 Colin Finck <colin@reactos.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::mem;

use zerocopy::FromBytes;

use crate::boot_sector::BootSector;
use crate::error::{NtfsError, Result};
use crate::file::{KnownNtfsFileRecordNumber, NtfsFile};
use crate::io::{Read, Seek};
use crate::structured_values::{NtfsVolumeInformation, NtfsVolumeName};
use crate::types::NtfsPosition;
use crate::upcase_table::UpcaseTable;

/// Root structure describing an NTFS filesystem.
#[derive(Debug)]
pub struct Ntfs {
    /// The size of a single cluster, in bytes. This is usually 4096.
    cluster_size: u32,
    /// The size of a single sector, in bytes. This is usually 512.
    sector_size: u16,
    /// Size of the filesystem, in bytes.
    size: u64,
    /// Absolute position of the Master File Table (MFT), in bytes.
    mft_position: NtfsPosition,
    /// Size of a single File Record, in bytes.
    file_record_size: u32,
    /// Serial number of the NTFS volume.
    serial_number: u64,
    /// Table of Unicode uppercase characters (only required for case-insensitive comparisons).
    upcase_table: Option<UpcaseTable>,
    #[cfg(feature = "std")]
    pub(crate) mft_map: std::sync::OnceLock<crate::mft::MftMap>,
}

impl Ntfs {
    /// Creates a new [`Ntfs`] object from a reader and validates its boot sector information.
    ///
    /// The reader must cover the entire NTFS partition, not more and not less.
    /// It will be rewinded to the beginning before reading anything.
    pub fn new<T>(fs: &mut T) -> Result<Self>
    where
        T: Read + Seek,
    {
        // Read and validate the boot sector.
        fs.rewind()?;
        let mut boot_sector_bytes = [0u8; mem::size_of::<BootSector>()];
        fs.read_exact(&mut boot_sector_bytes)?;
        let boot_sector = BootSector::ref_from_bytes(&boot_sector_bytes).unwrap();
        boot_sector.validate()?;

        let bpb = boot_sector.bpb();
        let cluster_size = bpb.cluster_size()?;
        let sector_size = bpb.sector_size()?;
        let total_sectors = bpb.total_sectors();
        let size = total_sectors
            .checked_mul(sector_size as u64)
            .ok_or(NtfsError::TotalSectorsTooBig { total_sectors })?;
        let mft_position = NtfsPosition::none();
        let file_record_size = bpb.file_record_size()?;
        let serial_number = bpb.serial_number();
        let upcase_table = None;

        let mut ntfs = Self {
            cluster_size,
            sector_size,
            size,
            mft_position,
            file_record_size,
            serial_number,
            upcase_table,
            #[cfg(feature = "std")]
            mft_map: std::sync::OnceLock::new(),
        };
        ntfs.mft_position = bpb.mft_lcn()?.position(&ntfs)?;

        Ok(ntfs)
    }

    /// Returns the size of a single cluster, in bytes.
    pub fn cluster_size(&self) -> u32 {
        self.cluster_size
    }

    /// Returns the [`NtfsFile`] for the given NTFS File Record Number.
    ///
    /// The first few NTFS files have fixed indexes and contain filesystem
    /// management information (see the [`KnownNtfsFileRecordNumber`] enum).
    pub fn file<'n, T>(&'n self, fs: &mut T, file_record_number: u64) -> Result<NtfsFile<'n>>
    where
        T: Read + Seek,
    {
        #[cfg(feature = "std")]
        {
            if let Some(map) = self.mft_map.get() {
                return map.read(self, fs, file_record_number);
            }
            let map = crate::mft::build(self, fs)?;
            match self.mft_map.set(map) {
                Ok(()) => self
                    .mft_map
                    .get()
                    .ok_or(NtfsError::InvalidMftMap {
                        reason: "MFT cache unavailable",
                    })?
                    .read(self, fs, file_record_number),
                Err(map) => map.read(self, fs, file_record_number),
            }
        }
        #[cfg(not(feature = "std"))]
        crate::mft::build(self, fs)?.read(self, fs, file_record_number)
    }

    /// Returns the size of a File Record of this NTFS filesystem, in bytes.
    pub fn file_record_size(&self) -> u32 {
        self.file_record_size
    }

    /// Returns the absolute byte position of the Master File Table (MFT).
    ///
    /// This [`NtfsPosition`] is guaranteed to be nonzero.
    pub fn mft_position(&self) -> NtfsPosition {
        self.mft_position
    }

    /// Reads the $UpCase file from the filesystem and stores it in this [`Ntfs`] object.
    ///
    /// This function only needs to be called if case-insensitive comparisons are later performed
    /// (i.e. finding files).
    pub fn read_upcase_table<T>(&mut self, fs: &mut T) -> Result<()>
    where
        T: Read + Seek,
    {
        let upcase_table = UpcaseTable::read(self, fs)?;
        self.upcase_table = Some(upcase_table);
        Ok(())
    }

    /// Whether the volume case-folding table is available.
    pub fn has_upcase_table(&self) -> bool {
        self.upcase_table.is_some()
    }

    /// Returns the root directory of this NTFS volume as an [`NtfsFile`].
    pub fn root_directory<'n, T>(&'n self, fs: &mut T) -> Result<NtfsFile<'n>>
    where
        T: Read + Seek,
    {
        self.file(fs, KnownNtfsFileRecordNumber::RootDirectory as u64)
    }

    /// Returns the size of a single sector in bytes.
    pub fn sector_size(&self) -> u16 {
        self.sector_size
    }

    /// Returns the 64-bit serial number of this NTFS volume.
    pub fn serial_number(&self) -> u64 {
        self.serial_number
    }

    /// Returns the partition size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the stored [`UpcaseTable`].
    ///
    /// # Panics
    ///
    /// Panics if [`read_upcase_table`][Ntfs::read_upcase_table] had not been called.
    pub(crate) fn upcase_table(&self) -> &UpcaseTable {
        self.upcase_table
            .as_ref()
            .expect("You need to call read_upcase_table first")
    }

    /// Returns an [`NtfsVolumeInformation`] containing general information about
    /// the volume, like the NTFS version.
    pub fn volume_info<T>(&self, fs: &mut T) -> Result<NtfsVolumeInformation>
    where
        T: Read + Seek,
    {
        let volume_file = self.file(fs, KnownNtfsFileRecordNumber::Volume as u64)?;
        volume_file.find_resident_attribute_structured_value::<NtfsVolumeInformation>(None)
    }

    /// Returns an [`NtfsVolumeName`] to read the volume name (also called volume label)
    /// of this NTFS volume.
    ///
    /// Note that a volume may also have no label, which is why the return value is further
    /// encapsulated in an `Option`.
    pub fn volume_name<T>(&self, fs: &mut T) -> Option<Result<NtfsVolumeName>>
    where
        T: Read + Seek,
    {
        let volume_file = iter_try!(self.file(fs, KnownNtfsFileRecordNumber::Volume as u64));

        match volume_file.find_resident_attribute_structured_value::<NtfsVolumeName>(None) {
            Ok(volume_name) => Some(Ok(volume_name)),
            Err(NtfsError::AttributeNotFound { .. }) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basics() {
        let mut testfs1 = crate::helpers::tests::testfs1();
        let ntfs = Ntfs::new(&mut testfs1).unwrap();
        assert_eq!(ntfs.cluster_size(), 512);
        assert_eq!(ntfs.sector_size(), 512);
        assert_eq!(ntfs.size(), 2096640);
    }

    #[test]
    fn test_volume_info() {
        let mut testfs1 = crate::helpers::tests::testfs1();
        let ntfs = Ntfs::new(&mut testfs1).unwrap();
        let volume_info = ntfs.volume_info(&mut testfs1).unwrap();
        assert_eq!(volume_info.major_version(), 3);
        assert_eq!(volume_info.minor_version(), 1);
    }

    #[test]
    fn test_volume_name() {
        let mut testfs1 = crate::helpers::tests::testfs1();
        let ntfs = Ntfs::new(&mut testfs1).unwrap();
        let volume_name = ntfs.volume_name(&mut testfs1).unwrap().unwrap();
        assert_eq!(volume_name.name_length(), 14);
        assert_eq!(volume_name.name(), "mylabel");
    }
}
