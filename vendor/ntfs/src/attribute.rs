// Copyright 2021-2026 Colin Finck <colin@reactos.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::vec::Vec;
use core::iter::FusedIterator;
use core::ops::Range;
use core::{fmt, mem};

use crate::io::{Read, Seek};
use bitflags::bitflags;
use enumn::N;
use memoffset::offset_of;
use nt_string::u16strle::U16StrLe;
use strum_macros::Display;

use crate::attribute_value::{
    NtfsAttributeListNonResidentAttributeValue, NtfsAttributeValue, NtfsNonResidentAttributeValue,
    NtfsResidentAttributeValue,
};
use crate::error::{NtfsError, Result};
use crate::file::NtfsFile;
use crate::structured_values::{
    NtfsAttributeList, NtfsAttributeListEntries, NtfsStructuredValue,
    NtfsStructuredValueFromResidentAttributeValue,
};
use crate::types::{NtfsPosition, Vcn};

/// Size of all [`NtfsAttributeHeader`] fields.
const ATTRIBUTE_HEADER_SIZE: usize = 16;

/// On-disk structure of the generic header of an NTFS Attribute.
#[repr(C, packed)]
struct NtfsAttributeHeader {
    /// Type of the attribute, known types are in [`NtfsAttributeType`].
    ty: u32,
    /// Length of the resident part of this attribute, in bytes.
    length: u32,
    /// 0 if this attribute has a resident value, 1 if this attribute has a non-resident value.
    is_non_resident: u8,
    /// Length of the name, in UTF-16 code points (every code point is 2 bytes).
    name_length: u8,
    /// Offset to the beginning of the name, in bytes from the beginning of this header.
    name_offset: u16,
    /// Flags of the attribute, known flags are in [`NtfsAttributeFlags`].
    flags: u16,
    /// Identifier of this attribute that is unique within the [`NtfsFile`].
    instance: u16,
}

bitflags! {
    /// Flags returned by [`NtfsAttribute::flags`].
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct NtfsAttributeFlags: u16 {
        /// The attribute value is compressed.
        const COMPRESSED = 0x0001;
        /// The attribute value is encrypted.
        const ENCRYPTED = 0x4000;
        /// The attribute value is stored sparsely.
        const SPARSE = 0x8000;
    }
}

impl fmt::Display for NtfsAttributeFlags {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// On-disk structure of the extra header of an NTFS Attribute that has a resident value.
#[repr(C, packed)]
struct NtfsResidentAttributeHeader {
    attribute_header: NtfsAttributeHeader,
    /// Length of the value, in bytes.
    value_length: u32,
    /// Offset to the beginning of the value, in bytes from the beginning of the [`NtfsAttributeHeader`].
    value_offset: u16,
    /// 1 if this attribute (with resident value) is referenced in an index.
    indexed_flag: u8,
}

/// On-disk structure of the extra header of an NTFS Attribute that has a non-resident value.
#[repr(C, packed)]
struct NtfsNonResidentAttributeHeader {
    attribute_header: NtfsAttributeHeader,
    /// Lower boundary of Virtual Cluster Numbers (VCNs) referenced by this attribute.
    /// This becomes relevant when file data is split over multiple attributes.
    /// Otherwise, it's zero.
    lowest_vcn: Vcn,
    /// Upper boundary of Virtual Cluster Numbers (VCNs) referenced by this attribute.
    /// This becomes relevant when file data is split over multiple attributes.
    /// Otherwise, it's zero (or even -1 for zero-length files according to NTFS-3G).
    highest_vcn: Vcn,
    /// Offset to the beginning of the value data runs.
    data_runs_offset: u16,
    /// Binary exponent denoting the number of clusters in a compression unit.
    /// A typical value is 4, meaning that 2^4 = 16 clusters are part of a compression unit.
    /// A value of zero means no compression (but that should better be determined via
    /// [`NtfsAttributeFlags`]).
    compression_unit_exponent: u16,
    reserved: [u8; 4],
    /// Allocated space for the attribute value, in bytes. This is always a multiple of the cluster size.
    /// For compressed files, this is always a multiple of the compression unit size.
    allocated_size: u64,
    /// Size of the attribute value, in bytes.
    /// This can be larger than `allocated_size` if the value is compressed or stored sparsely.
    data_size: u64,
    /// Size of the initialized part of the attribute value, in bytes.
    /// This is usually the same as `data_size`.
    initialized_size: u64,
}

/// Storage geometry of a non-resident attribute, before filesystem decoding.
#[derive(Clone, Copy, Debug)]
pub struct NtfsNonResidentStorageInfo {
    /// Virtual allocated span, including sparse compression-unit padding.
    pub allocated_size: u64,
    /// Logical bytes that have been initialized; the remainder reads as zero.
    pub initialized_size: u64,
    /// Binary exponent of the compression-unit cluster count.
    pub compression_unit_exponent: u16,
}

#[path = "attribute_extents.rs"]
mod extents;
pub use extents::NtfsDataExtent;

/// All known NTFS Attribute types.
///
/// Reference: <https://flatcap.github.io/linux-ntfs/ntfs/attributes/index.html>
#[derive(Clone, Copy, Debug, Display, Eq, N, PartialEq)]
#[repr(u32)]
pub enum NtfsAttributeType {
    /// $STANDARD_INFORMATION, see [`NtfsStandardInformation`].
    ///
    /// [`NtfsStandardInformation`]: crate::structured_values::NtfsStandardInformation
    StandardInformation = 0x10,
    /// $ATTRIBUTE_LIST, see [`NtfsAttributeList`].
    ///
    /// [`NtfsAttributeList`]: crate::structured_values::NtfsAttributeList
    AttributeList = 0x20,
    /// $FILE_NAME, see [`NtfsFileName`].
    ///
    /// [`NtfsFileName`]: crate::structured_values::NtfsFileName
    FileName = 0x30,
    /// $OBJECT_ID, see [`NtfsObjectId`].
    ///
    /// [`NtfsObjectId`]: crate::structured_values::NtfsObjectId
    ObjectId = 0x40,
    /// $SECURITY_DESCRIPTOR
    SecurityDescriptor = 0x50,
    /// $VOLUME_NAME, see [`NtfsVolumeName`].
    ///
    /// [`NtfsVolumeName`]: crate::structured_values::NtfsVolumeName
    VolumeName = 0x60,
    /// $VOLUME_INFORMATION, see [`NtfsVolumeInformation`].
    ///
    /// [`NtfsVolumeInformation`]: crate::structured_values::NtfsVolumeInformation
    VolumeInformation = 0x70,
    /// $DATA, see [`NtfsFile::data`].
    Data = 0x80,
    /// $INDEX_ROOT, see [`NtfsIndexRoot`].
    ///
    /// [`NtfsIndexRoot`]: crate::structured_values::NtfsIndexRoot
    IndexRoot = 0x90,
    /// $INDEX_ALLOCATION, see [`NtfsIndexAllocation`].
    ///
    /// [`NtfsIndexAllocation`]: crate::structured_values::NtfsIndexAllocation
    IndexAllocation = 0xA0,
    /// $BITMAP
    Bitmap = 0xB0,
    /// $REPARSE_POINT
    ReparsePoint = 0xC0,
    /// $EA_INFORMATION
    EAInformation = 0xD0,
    /// $EA
    EA = 0xE0,
    /// $PROPERTY_SET
    PropertySet = 0xF0,
    /// $LOGGED_UTILITY_STREAM
    LoggedUtilityStream = 0x100,
    /// Marks the end of the valid attributes.
    End = 0xFFFF_FFFF,
}

/// A single NTFS Attribute of an [`NtfsFile`].
///
/// Not to be confused with [`NtfsFileAttributeFlags`].
///
/// This structure is returned by the [`NtfsAttributesRaw`] iterator as well as [`NtfsAttributeItem::to_attribute`].
///
/// Reference: <https://flatcap.github.io/linux-ntfs/ntfs/concepts/attribute_header.html>
///
/// [`NtfsFileAttributeFlags`]: crate::structured_values::NtfsFileAttributeFlags
#[derive(Clone, Debug)]
pub struct NtfsAttribute<'n, 'f> {
    file: &'f NtfsFile<'n>,
    offset: usize,
    /// Has a value if this attribute's value may be split over multiple attributes.
    /// The connected attributes can be iterated using the encapsulated iterator.
    list_entries: Option<&'f NtfsAttributeListEntries<'n, 'f>>,
}

impl<'n, 'f> NtfsAttribute<'n, 'f> {
    pub(crate) fn new(
        file: &'f NtfsFile<'n>,
        offset: usize,
        list_entries: Option<&'f NtfsAttributeListEntries<'n, 'f>>,
    ) -> Result<Self> {
        let attribute = Self {
            file,
            offset,
            list_entries,
        };
        attribute.validate_attribute_length()?;

        Ok(attribute)
    }

    /// Returns the length of this NTFS Attribute, in bytes.
    ///
    /// This denotes the length of the attribute structure on disk.
    /// Apart from various headers, this structure also includes the name and,
    /// for resident attributes, the actual value.
    pub fn attribute_length(&self) -> u32 {
        let start = self.offset + offset_of!(NtfsAttributeHeader, length);
        u32::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    pub(crate) fn ensure_ty(&self, expected: NtfsAttributeType) -> Result<()> {
        let ty = self.ty()?;
        if ty != expected {
            return Err(NtfsError::AttributeOfDifferentType {
                position: self.position(),
                expected,
                actual: ty,
            });
        }

        Ok(())
    }

    /// Returns flags set for this attribute as specified by [`NtfsAttributeFlags`].
    pub fn flags(&self) -> NtfsAttributeFlags {
        let start = self.offset + offset_of!(NtfsAttributeHeader, flags);
        NtfsAttributeFlags::from_bits_truncate(u16::from_le_bytes(
            *self.file.record_data()[start..].first_chunk().unwrap(),
        ))
    }

    /// Returns the identifier of this attribute that is unique within the [`NtfsFile`].
    pub fn instance(&self) -> u16 {
        let start = self.offset + offset_of!(NtfsAttributeHeader, instance);
        u16::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    /// First VCN of this physical non-resident extent; zero for resident data.
    pub fn lowest_vcn(&self) -> u64 {
        if self.is_resident() {
            return 0;
        }
        let start = self.offset + 16;
        let bytes = &self.file.record_data()[start..start + 8];
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }

    /// On-disk type number, including unrecognized attribute types.
    pub fn raw_type(&self) -> u32 {
        let bytes = &self.file.record_data()[self.offset..self.offset + 4];
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// All flag bits, including unknown compression formats.
    pub fn raw_flags(&self) -> u16 {
        let bytes = &self.file.record_data()[self.offset + 12..self.offset + 14];
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    /// Record that physically contains this attribute header.
    pub fn file_record_number(&self) -> u64 {
        self.file.file_record_number()
    }

    /// Returns `true` if this is a resident attribute, i.e. one where its value
    /// is part of the attribute structure.
    pub fn is_resident(&self) -> bool {
        let start = self.offset + offset_of!(NtfsAttributeHeader, is_non_resident);
        let is_non_resident = self.file.record_data()[start];
        is_non_resident == 0
    }

    /// Gets the name of this NTFS Attribute (if any) and returns it wrapped in a [`U16StrLe`].
    ///
    /// Note that most NTFS attributes have no name and are distinguished by their types.
    /// Use [`NtfsAttribute::ty`] to get the attribute type.
    pub fn name(&self) -> Result<U16StrLe<'f>> {
        if self.name_offset() == 0 || self.name_length() == 0 {
            return Ok(U16StrLe(&[]));
        }

        self.validate_name_sizes()?;

        let start = self.offset + self.name_offset() as usize;
        let end = start + self.name_length();
        let string = U16StrLe(&self.file.record_data()[start..end]);

        Ok(string)
    }

    fn name_offset(&self) -> u16 {
        let start = self.offset + offset_of!(NtfsAttributeHeader, name_offset);
        u16::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    /// Returns the length of the name of this NTFS Attribute, in bytes.
    ///
    /// An attribute name has a maximum length of 255 UTF-16 code points (510 bytes).
    /// It is always part of the attribute itself and hence also of the length
    /// returned by [`NtfsAttribute::attribute_length`].
    pub fn name_length(&self) -> usize {
        let start = self.offset + offset_of!(NtfsAttributeHeader, name_length);
        let name_length_in_characters = self.file.record_data()[start];
        name_length_in_characters as usize * mem::size_of::<u16>()
    }

    pub(crate) fn non_resident_value_data_and_position(&self) -> Result<(&'f [u8], NtfsPosition)> {
        debug_assert!(!self.is_resident());
        let start = self.offset + self.non_resident_value_data_runs_offset() as usize;
        let end = self.offset + self.attribute_length() as usize;
        let position = self.file.position() + start;
        let data = &self.file.record_data().get(start..end).ok_or(
            NtfsError::InvalidNonResidentValueDataRange {
                position,
                range: start..end,
                size: self.file.record_data().len(),
            },
        )?;
        Ok((data, position))
    }

    fn non_resident_value_data_size(&self) -> u64 {
        debug_assert!(!self.is_resident());
        let start = self.offset + offset_of!(NtfsNonResidentAttributeHeader, data_size);
        u64::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    fn non_resident_value_data_runs_offset(&self) -> u16 {
        debug_assert!(!self.is_resident());
        let start = self.offset + offset_of!(NtfsNonResidentAttributeHeader, data_runs_offset);
        u16::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the absolute position of this NTFS Attribute within the filesystem, in bytes.
    pub fn position(&self) -> NtfsPosition {
        self.file.position() + self.offset
    }

    /// Attempts to parse the value data as the given resident structured value type and returns that.
    ///
    /// This is a fast path for attributes that are always resident.
    /// It doesn't need a reference to the filesystem reader.
    ///
    /// This function first checks that the attribute is of the required type for that structured value
    /// and if it's a resident attribute.
    /// It returns with an error if that is not the case.
    /// It also returns an error for any parsing problem.
    pub fn resident_structured_value<S>(&self) -> Result<S>
    where
        S: NtfsStructuredValueFromResidentAttributeValue<'n, 'f>,
    {
        self.ensure_ty(S::TY)?;

        if !self.is_resident() {
            return Err(NtfsError::UnexpectedNonResidentAttribute {
                position: self.position(),
            });
        }

        let resident_value = self.resident_value()?;
        S::from_resident_attribute_value(resident_value)
    }

    pub(crate) fn resident_value(&self) -> Result<NtfsResidentAttributeValue<'f>> {
        debug_assert!(self.is_resident());
        self.validate_resident_value_sizes()?;

        let start = self.offset + self.resident_value_offset() as usize;
        let end = start + self.resident_value_length() as usize;
        let data = &self.file.record_data()[start..end];

        Ok(NtfsResidentAttributeValue::new(data, self.position()))
    }

    fn resident_value_length(&self) -> u32 {
        debug_assert!(self.is_resident());
        let start = self.offset + offset_of!(NtfsResidentAttributeHeader, value_length);
        u32::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    fn resident_value_offset(&self) -> u16 {
        debug_assert!(self.is_resident());
        let start = self.offset + offset_of!(NtfsResidentAttributeHeader, value_offset);
        u16::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap())
    }

    /// Attempts to parse the value data as the given structured value type and returns that.
    ///
    /// This function first checks that the attribute is of the required type for that structured value.
    /// It returns with an error if that is not the case.
    /// It also returns an error for any parsing problem.
    pub fn structured_value<T, S>(&self, fs: &mut T) -> Result<S>
    where
        T: Read + Seek,
        S: NtfsStructuredValue<'n, 'f>,
    {
        self.ensure_ty(S::TY)?;
        let value = self.value(fs)?;
        S::from_attribute_value(fs, value)
    }

    /// Returns the type of this NTFS Attribute, or [`NtfsError::UnsupportedAttributeType`]
    /// if it's an unknown type.
    pub fn ty(&self) -> Result<NtfsAttributeType> {
        let start = self.offset + offset_of!(NtfsAttributeHeader, ty);
        let ty = u32::from_le_bytes(*self.file.record_data()[start..].first_chunk().unwrap());

        NtfsAttributeType::n(ty).ok_or(NtfsError::UnsupportedAttributeType {
            position: self.position(),
            actual: ty,
        })
    }

    fn validate_attribute_length(&self) -> Result<()> {
        let start = self.offset;
        let end = self.file.record_data().len();
        let remaining_length = (start..end).len();

        if remaining_length < ATTRIBUTE_HEADER_SIZE {
            return Err(NtfsError::InvalidAttributeLength {
                position: self.position(),
                expected: ATTRIBUTE_HEADER_SIZE,
                actual: remaining_length,
            });
        }

        let attribute_length = self.attribute_length() as usize;
        let minimum = if self.is_resident() { 24 } else { 64 };
        if attribute_length < minimum {
            return Err(NtfsError::InvalidAttributeLength {
                position: self.position(),
                expected: minimum,
                actual: attribute_length,
            });
        }

        if attribute_length > remaining_length {
            return Err(NtfsError::InvalidAttributeLength {
                position: self.position(),
                expected: attribute_length,
                actual: remaining_length,
            });
        }

        Ok(())
    }

    fn validate_name_sizes(&self) -> Result<()> {
        let start = self.name_offset();
        if start as u32 >= self.attribute_length() {
            return Err(NtfsError::InvalidAttributeNameOffset {
                position: self.position(),
                expected: start,
                actual: self.attribute_length(),
            });
        }

        let end = start as usize + self.name_length();
        if end > self.attribute_length() as usize {
            return Err(NtfsError::InvalidAttributeNameLength {
                position: self.position(),
                expected: end,
                actual: self.attribute_length(),
            });
        }

        Ok(())
    }

    fn validate_resident_value_sizes(&self) -> Result<()> {
        debug_assert!(self.is_resident());

        let position = self.position();
        let attribute_length = self.attribute_length();

        let start = self.resident_value_offset();
        if start as u32 > attribute_length {
            return Err(NtfsError::InvalidResidentAttributeValueOffset {
                position,
                expected: start,
                actual: attribute_length,
            });
        }

        let length = self.resident_value_length();

        let end = u32::from(start).checked_add(length).ok_or(
            NtfsError::InvalidResidentAttributeValueLength {
                position,
                length,
                offset: start,
                actual: attribute_length,
            },
        )?;
        if end > attribute_length {
            return Err(NtfsError::InvalidResidentAttributeValueLength {
                position,
                length,
                offset: start,
                actual: attribute_length,
            });
        }

        Ok(())
    }

    /// Returns an [`NtfsAttributeValue`] structure to read the value of this NTFS Attribute.
    pub fn value<T>(&self, fs: &mut T) -> Result<NtfsAttributeValue<'n, 'f>>
    where
        T: Read + Seek,
    {
        self.value_with_length(fs, self.value_length())
    }

    /// Returns non-resident storage fields, or `None` for resident values.
    /// Checks the complete header before accessing these additional fields.
    pub fn storage_info(&self) -> Result<Option<NtfsNonResidentStorageInfo>> {
        if self.is_resident() {
            return Ok(None);
        }
        let expected = mem::size_of::<NtfsNonResidentAttributeHeader>();
        let actual = self.attribute_length() as usize;
        if actual < expected {
            return Err(NtfsError::InvalidAttributeLength {
                position: self.position(),
                expected,
                actual,
            });
        }
        let header = &self.file.record_data()[self.offset..self.offset + expected];
        Ok(Some(NtfsNonResidentStorageInfo {
            allocated_size: u64::from_le_bytes(core::array::from_fn(|index| header[40 + index])),
            initialized_size: u64::from_le_bytes(core::array::from_fn(|index| header[56 + index])),
            compression_unit_exponent: u16::from_le_bytes([header[34], header[35]]),
        }))
    }

    /// Reads mapped storage up to its virtual allocated span, including sparse
    /// padding. Unlike `value`, this does not truncate compressed storage at
    /// the decoded file length. Connected attributes remain joined normally.
    pub fn raw_value<T>(&self, fs: &mut T) -> Result<NtfsAttributeValue<'n, 'f>>
    where
        T: Read + Seek,
    {
        let length = self
            .storage_info()?
            .map_or(self.value_length(), |info| info.allocated_size);
        self.value_with_length(fs, length)
    }

    fn value_with_length<T>(&self, fs: &mut T, data_size: u64) -> Result<NtfsAttributeValue<'n, 'f>>
    where
        T: Read + Seek,
    {
        if let Some(list_entries) = self.list_entries {
            // The first attribute reports the entire data size for all connected attributes
            // (remaining ones are set to zero).
            // Fortunately, we are the first attribute :)

            let value = NtfsAttributeListNonResidentAttributeValue::new(
                self.file.ntfs(),
                fs,
                list_entries.clone(),
                self.name()?.0.to_vec(),
                self.raw_type(),
                data_size,
            )?;
            Ok(NtfsAttributeValue::AttributeListNonResident(value))
        } else if self.is_resident() {
            let value = self.resident_value()?;
            Ok(NtfsAttributeValue::Resident(value))
        } else {
            let (data, position) = self.non_resident_value_data_and_position()?;
            let value =
                NtfsNonResidentAttributeValue::new(self.file.ntfs(), data, position, data_size)?;
            Ok(NtfsAttributeValue::NonResident(value))
        }
    }

    /// Returns the length of the value data of this NTFS Attribute, in bytes.
    pub fn value_length(&self) -> u64 {
        if self.is_resident() {
            self.resident_value_length() as u64
        } else {
            self.non_resident_value_data_size()
        }
    }
}

/// Iterator over
///   all attributes of an [`NtfsFile`],
///   returning an [`NtfsAttributeItem`] for each entry.
///
/// This iterator is returned from the [`NtfsFile::attributes`] function.
/// It provides a flattened "data-centric" view of the attributes and abstracts away the filesystem details
/// to deal with many or large attributes (Attribute Lists and connected attributes).
///
/// Check [`NtfsAttributesRaw`] if you want to iterate over the plain attributes on the filesystem.
/// See [`NtfsAttributesAttached`] for an iterator that implements [`Iterator`] and [`FusedIterator`].
#[derive(Clone, Debug)]
pub struct NtfsAttributes<'n, 'f> {
    raw_iter: NtfsAttributesRaw<'n, 'f>,
    list_entries: Option<NtfsAttributeListEntries<'n, 'f>>,
    list_skip_info: Option<(Vec<u8>, u32)>,
    listed_base_instances: Vec<u16>,
    type_filter: Option<NtfsAttributeType>,
}

impl<'n, 'f> NtfsAttributes<'n, 'f> {
    pub(crate) fn new(file: &'f NtfsFile<'n>) -> Self {
        Self {
            raw_iter: NtfsAttributesRaw::new(file),
            list_entries: None,
            list_skip_info: None,
            listed_base_instances: Vec::new(),
            type_filter: None,
        }
    }

    /// Restricts this fresh iterator to one attribute type before resolving
    /// attribute-list references. Unrelated references are never opened;
    /// matching references and connected extents retain all validation.
    pub fn of_type(mut self, ty: NtfsAttributeType) -> Self {
        self.type_filter = Some(ty);
        self
    }

    /// Returns a variant of this iterator that implements [`Iterator`] and [`FusedIterator`]
    /// by mutably borrowing the filesystem reader.
    pub fn attach<'a, T>(self, fs: &'a mut T) -> NtfsAttributesAttached<'n, 'f, 'a, T>
    where
        T: Read + Seek,
    {
        NtfsAttributesAttached::new(fs, self)
    }

    /// See [`Iterator::next`].
    pub fn next<T>(&mut self, fs: &mut T) -> Option<Result<NtfsAttributeItem<'n, 'f>>>
    where
        T: Read + Seek,
    {
        loop {
            if let Some(attribute_list_entries) = &mut self.list_entries {
                loop {
                    // If the next Attribute List entry turns out to be a non-resident attribute, that attribute's
                    // value may be split over multiple (adjacent) attributes.
                    // To view this value as a single one, we need an `AttributeListConnectedEntries` iterator
                    // and that iterator needs `NtfsAttributeListEntries` where the next call to `next` yields
                    // the first connected attribute.
                    // Therefore, we need to clone `attribute_list_entries` before every call.
                    let attribute_list_entries_clone = attribute_list_entries.clone();

                    let entry = match attribute_list_entries.next(fs) {
                        Some(Ok(entry)) => entry,
                        Some(Err(e)) => return Some(Err(e)),
                        None => break,
                    };
                    let entry_instance = entry.instance();
                    let entry_record_number = entry.base_file_reference().file_record_number();
                    let entry_ty = entry.raw_type();
                    if self.type_filter.is_some_and(|ty| entry_ty != ty as u32) {
                        continue;
                    }

                    // The first extent may live in the base record. It must
                    // retain the list reader too, otherwise its continuation
                    // (often sparse compression padding) is never joined.
                    if entry_record_number == self.raw_iter.file.file_record_number() {
                        self.listed_base_instances.push(entry_instance);
                    }

                    // Ignore all Attribute List entries that are connected attributes of a previous one.
                    if let Some((skip_name, skip_ty)) = &self.list_skip_info {
                        if entry.name().0 == skip_name
                            && entry_ty == *skip_ty
                            && entry.lowest_vcn().value() > 0
                        {
                            continue;
                        }
                    }

                    // We found an attribute that we want to return.
                    self.list_skip_info = None;

                    let ntfs = self.raw_iter.file.ntfs();
                    let entry_file = iter_try!(entry.to_file(ntfs, fs));
                    let entry_attribute = iter_try!(entry.to_attribute(&entry_file));
                    let attribute_offset = entry_attribute.offset();

                    let mut list_entries = None;
                    if !entry_attribute.is_resident() {
                        list_entries = Some(attribute_list_entries_clone);
                        self.list_skip_info = Some((entry.name().0.to_vec(), entry_ty));
                    }

                    let item = NtfsAttributeItem {
                        attribute_file: self.raw_iter.file,
                        attribute_value_file: Some(entry_file),
                        attribute_offset,
                        list_entries,
                    };
                    return Some(Ok(item));
                }
            }

            let attribute = iter_try!(self.raw_iter.next()?);
            if self.listed_base_instances.contains(&attribute.instance()) {
                continue;
            }
            if let Ok(NtfsAttributeType::AttributeList) = attribute.ty() {
                let attribute_list =
                    iter_try!(attribute.structured_value::<T, NtfsAttributeList>(fs));
                self.list_entries = Some(attribute_list.entries());
            } else {
                if self
                    .type_filter
                    .is_some_and(|ty| attribute.raw_type() != ty as u32)
                {
                    continue;
                }
                let item = NtfsAttributeItem {
                    attribute_file: self.raw_iter.file,
                    attribute_value_file: None,
                    attribute_offset: attribute.offset(),
                    list_entries: None,
                };
                return Some(Ok(item));
            }
        }
    }
}

/// Iterator over
///   all attributes of an [`NtfsFile`],
///   returning an [`NtfsAttributeItem`] for each entry,
///   implementing [`Iterator`] and [`FusedIterator`].
///
/// This iterator is returned from the [`NtfsAttributes::attach`] function.
/// Conceptually the same as [`NtfsAttributes`], but mutably borrows the filesystem
/// to implement aforementioned traits.
#[derive(Debug)]
pub struct NtfsAttributesAttached<'n, 'f, 'a, T: Read + Seek> {
    fs: &'a mut T,
    attributes: NtfsAttributes<'n, 'f>,
}

impl<'n, 'f, 'a, T> NtfsAttributesAttached<'n, 'f, 'a, T>
where
    T: Read + Seek,
{
    fn new(fs: &'a mut T, attributes: NtfsAttributes<'n, 'f>) -> Self {
        Self { fs, attributes }
    }

    /// Consumes this iterator and returns the inner [`NtfsAttributes`].
    pub fn detach(self) -> NtfsAttributes<'n, 'f> {
        self.attributes
    }
}

impl<'n, 'f, 'a, T> Iterator for NtfsAttributesAttached<'n, 'f, 'a, T>
where
    T: Read + Seek,
{
    type Item = Result<NtfsAttributeItem<'n, 'f>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.attributes.next(self.fs)
    }
}

impl<'n, 'f, 'a, T> FusedIterator for NtfsAttributesAttached<'n, 'f, 'a, T> where T: Read + Seek {}

/// Item returned by the [`NtfsAttributes`] iterator.
///
/// [`NtfsAttributes`] provides a flattened view over the attributes by traversing Attribute Lists.
/// Attribute Lists may contain entries with references to other [`NtfsFile`]s.
/// Therefore, the attribute's information may either be stored in the original [`NtfsFile`] or in another
/// [`NtfsFile`] that has been read just for this attribute.
///
/// [`NtfsAttributeItem`] abstracts over both cases by providing a reference to the original [`NtfsFile`],
/// and optionally holding another [`NtfsFile`] if the attribute is actually stored there.
#[derive(Clone, Debug)]
pub struct NtfsAttributeItem<'n, 'f> {
    attribute_file: &'f NtfsFile<'n>,
    attribute_value_file: Option<NtfsFile<'n>>,
    attribute_offset: usize,
    list_entries: Option<NtfsAttributeListEntries<'n, 'f>>,
}

impl<'n, 'f> NtfsAttributeItem<'n, 'f> {
    /// Returns the actual [`NtfsAttribute`] structure for this NTFS Attribute.
    pub fn to_attribute<'i>(&'i self) -> Result<NtfsAttribute<'n, 'i>> {
        if let Some(file) = &self.attribute_value_file {
            NtfsAttribute::new(file, self.attribute_offset, self.list_entries.as_ref())
        } else {
            NtfsAttribute::new(
                self.attribute_file,
                self.attribute_offset,
                self.list_entries.as_ref(),
            )
        }
    }
}

/// Iterator over
///   all top-level attributes of an [`NtfsFile`],
///   returning an [`NtfsAttribute`] for each entry,
///   implementing [`Iterator`] and [`FusedIterator`].
///
/// This iterator is returned from the [`NtfsFile::attributes_raw`] function.
/// Contrary to [`NtfsAttributes`], it does not traverse $ATTRIBUTE_LIST attributes and returns them
/// as raw [`NtfsAttribute`]s.
/// Check that structure if you want an iterator providing a flattened "data-centric" view over
/// the attributes by traversing Attribute Lists automatically.
#[derive(Clone, Debug)]
pub struct NtfsAttributesRaw<'n, 'f> {
    file: &'f NtfsFile<'n>,
    items_range: Range<usize>,
}

impl<'n, 'f> NtfsAttributesRaw<'n, 'f> {
    pub(crate) fn new(file: &'f NtfsFile<'n>) -> Self {
        let start = file.first_attribute_offset() as usize;
        let end = file.data_size() as usize;
        let items_range = start..end;

        Self { file, items_range }
    }
}

impl<'n, 'f> Iterator for NtfsAttributesRaw<'n, 'f> {
    type Item = Result<NtfsAttribute<'n, 'f>>;

    fn next(&mut self) -> Option<Self::Item> {
        // This may be an entire attribute or just the 4-byte end marker.
        // Check if this marks the end of the attribute list.
        let start = self.items_range.start;
        let end = start + mem::size_of::<u32>();
        let ty_slice = self.file.record_data().get(start..end)?;

        let ty = u32::from_le_bytes(*ty_slice.first_chunk().unwrap());
        if ty == NtfsAttributeType::End as u32 {
            return None;
        }

        // It's a real attribute.
        let attribute = match NtfsAttribute::new(self.file, self.items_range.start, None) {
            Ok(attribute) => attribute,
            Err(e) => {
                // A malformed attribute header means the rest of the chain can
                // no longer be trusted — and the length we'd advance past it is
                // exactly what failed to validate, so we have no safe stride.
                // Terminate the iterator (return the error once, then `None`) so
                // a caller that ignores the error and calls `next` again can't
                // spin forever re-reading the same bad offset.
                self.items_range.start = self.items_range.end;
                return Some(Err(e));
            }
        };
        self.items_range.start += attribute.attribute_length() as usize;

        Some(Ok(attribute))
    }
}

impl<'n, 'f> FusedIterator for NtfsAttributesRaw<'n, 'f> {}

#[cfg(test)]
mod tests {
    use crate::indexes::NtfsFileNameIndex;
    use crate::ntfs::Ntfs;
    use crate::traits::NtfsReadSeek;

    #[test]
    fn test_empty_data_attribute() {
        let mut testfs1 = crate::helpers::tests::testfs1();
        let mut ntfs = Ntfs::new(&mut testfs1).unwrap();
        ntfs.read_upcase_table(&mut testfs1).unwrap();
        let root_dir = ntfs.root_directory(&mut testfs1).unwrap();

        // Find the "empty-file".
        let root_dir_index = root_dir.directory_index(&mut testfs1).unwrap();
        let mut root_dir_finder = root_dir_index.finder();
        let entry =
            NtfsFileNameIndex::find(&mut root_dir_finder, &ntfs, &mut testfs1, "empty-file")
                .unwrap()
                .unwrap();
        let empty_file = entry.to_file(&ntfs, &mut testfs1).unwrap();

        let data_attribute_item = empty_file.data(&mut testfs1, "").unwrap().unwrap();
        let data_attribute = data_attribute_item.to_attribute().unwrap();
        assert_eq!(data_attribute.value_length(), 0);

        let mut data_attribute_value = data_attribute.value(&mut testfs1).unwrap();
        assert!(data_attribute_value.is_empty());

        let mut buf = [0u8; 5];
        let bytes_read = data_attribute_value.read(&mut testfs1, &mut buf).unwrap();
        assert_eq!(bytes_read, 0);
    }
}
