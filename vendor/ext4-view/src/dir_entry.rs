// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::error::{CorruptKind, Ext4Error};
use crate::file_type::FileType;
use crate::format::{BytesDisplay, format_bytes_debug};
use crate::inode::{Inode, InodeIndex};
use crate::metadata::Metadata;
use crate::path::{Path, PathBuf};
use crate::util::{read_u16le, read_u32le};
use alloc::rc::Rc;
use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::num::NonZero;
use core::str::Utf8Error;

/// Error returned when [`DirEntryName`] construction fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DirEntryNameError {
    /// Name is empty.
    Empty,

    /// Name is longer than [`DirEntryName::MAX_LEN`].
    TooLong,

    /// Name contains a null byte.
    ContainsNull,

    /// Name contains a path separator.
    ContainsSeparator,
}

impl Display for DirEntryNameError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "direntry name is empty"),
            Self::TooLong => {
                write!(f, "directory entry name is longer than 255 bytes")
            }
            Self::ContainsNull => {
                write!(f, "directory entry name contains a null byte")
            }
            Self::ContainsSeparator => {
                write!(f, "directory entry name contains a path separator")
            }
        }
    }
}

impl Error for DirEntryNameError {}

/// Name of a [`DirEntry`], stored as a reference.
///
/// This is guaranteed at construction to be a valid directory entry
/// name.
#[derive(Clone, Copy, Eq, Ord, PartialOrd, Hash)]
pub struct DirEntryName<'a>(pub(crate) &'a [u8]);

impl<'a> DirEntryName<'a> {
    /// Maximum length of a `DirEntryName`.
    pub const MAX_LEN: usize = 255;

    /// Convert to a `&str` if the name is valid UTF-8.
    #[inline]
    pub fn as_str(&self) -> Result<&'a str, Utf8Error> {
        core::str::from_utf8(self.0)
    }

    /// Get an object that implements [`Display`] to allow conveniently
    /// printing names that may or may not be valid UTF-8. Non-UTF-8
    /// characters will be replaced with '�'.
    ///
    /// [`Display`]: core::fmt::Display
    pub fn display(&self) -> BytesDisplay<'_> {
        BytesDisplay(self.0)
    }
}

impl<'a> AsRef<[u8]> for DirEntryName<'a> {
    fn as_ref(&self) -> &'a [u8] {
        self.0
    }
}

impl Debug for DirEntryName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format_bytes_debug(self.0, f)
    }
}

impl<T> PartialEq<T> for DirEntryName<'_>
where
    T: AsRef<[u8]>,
{
    fn eq(&self, other: &T) -> bool {
        self.0 == other.as_ref()
    }
}

impl<'a> TryFrom<&'a [u8]> for DirEntryName<'a> {
    type Error = DirEntryNameError;

    fn try_from(bytes: &'a [u8]) -> Result<Self, DirEntryNameError> {
        if bytes.is_empty() {
            Err(DirEntryNameError::Empty)
        } else if bytes.len() > Self::MAX_LEN {
            Err(DirEntryNameError::TooLong)
        } else if bytes.contains(&0) {
            Err(DirEntryNameError::ContainsNull)
        } else if bytes.contains(&Path::SEPARATOR) {
            Err(DirEntryNameError::ContainsSeparator)
        } else {
            Ok(Self(bytes))
        }
    }
}

impl<'a, const N: usize> TryFrom<&'a [u8; N]> for DirEntryName<'a> {
    type Error = DirEntryNameError;

    fn try_from(bytes: &'a [u8; N]) -> Result<Self, DirEntryNameError> {
        Self::try_from(bytes.as_slice())
    }
}

impl<'a> TryFrom<&'a str> for DirEntryName<'a> {
    type Error = DirEntryNameError;

    fn try_from(s: &'a str) -> Result<Self, DirEntryNameError> {
        Self::try_from(s.as_bytes())
    }
}

#[derive(Clone, Eq, Ord, PartialOrd)]
struct DirEntryNameBuf {
    data: [u8; DirEntryName::MAX_LEN],
    len: u8,
}

impl DirEntryNameBuf {
    #[inline]
    #[must_use]
    fn as_bytes(&self) -> &[u8] {
        &self.data[..usize::from(self.len)]
    }

    #[inline]
    #[must_use]
    fn as_dir_entry_name(&self) -> DirEntryName<'_> {
        DirEntryName(self.as_bytes())
    }
}

impl Debug for DirEntryNameBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format_bytes_debug(self.as_bytes(), f)
    }
}

// Manual implementation of `PartialEq` because we don't want to compare
// the entire `data` array, only up to `len`.
impl PartialEq<Self> for DirEntryNameBuf {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

// Manual implementation of `Hash` because we don't want to include the
// entire `data` array, only up to `len` (see also `PartialEq` impl).
impl Hash for DirEntryNameBuf {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.as_bytes().hash(hasher);
    }
}

impl TryFrom<&[u8]> for DirEntryNameBuf {
    type Error = DirEntryNameError;

    fn try_from(bytes: &[u8]) -> Result<Self, DirEntryNameError> {
        // This performs all the necessary validation of the input.
        DirEntryName::try_from(bytes)?;

        let mut name = Self {
            data: [0; DirEntryName::MAX_LEN],
            // OK to unwrap: already checked against `MAX_LEN`.
            len: u8::try_from(bytes.len()).unwrap(),
        };
        name.data[..bytes.len()].copy_from_slice(bytes);
        Ok(name)
    }
}

/// Directory entry.
#[derive(Clone)]
pub struct DirEntry {
    fs: Ext4,

    /// Number of the inode that this entry points to.
    pub(crate) inode: InodeIndex,

    /// Raw name of the entry.
    name: DirEntryNameBuf,

    /// Path that `read_dir` was called with. This is shared via `Rc` so
    /// that only one allocation is required.
    path: Rc<PathBuf>,

    /// Entry file type.
    file_type: FileType,
}

impl DirEntry {
    /// Read a `DirEntry` from a byte slice.
    ///
    /// If no error occurs, this returns `(Option<DirEntry>, usize)`.
    /// * The first value in this tuple is an `Option` because some
    ///   special data is stored in directory blocks that aren't
    ///   actually directory entries. If the inode pointed to by the
    ///   entry is zero, this value is set to None.
    /// * The `NonZero<usize>` in this tuple is the overall length of
    ///   the entry's data. This is used when iterating over raw dir
    ///   entry data. A `NonZero` is used because it's important for
    ///   iterators that call `DirEntry::from_bytes` to make forward
    ///   progress. A length of zero would cause them to enter an
    ///   infinite loop.
    pub(crate) fn from_bytes(
        fs: Ext4,
        bytes: &[u8],
        inode: InodeIndex,
        path: Rc<PathBuf>,
    ) -> Result<(Option<Self>, NonZero<usize>), Ext4Error> {
        const NAME_OFFSET: usize = 8;

        // Check size (the full entry will usually be larger than this),
        // but these header fields must be present.
        if bytes.len() < NAME_OFFSET {
            return Err(CorruptKind::DirEntryMissingHeader(inode, bytes.len()).into());
        }

        // Get the inode that this entry points to. If zero, this is a
        // special type of entry (such as a checksum entry or hash tree
        // node entry).
        let points_to_inode = read_u32le(bytes, 0);

        // Get the full size of the entry.
        let rec_len = read_u16le(bytes, 4);
        let rec_len = if fs.block_size() == 65536 && matches!(rec_len, 0 | 65535) {
            65536
        } else {
            usize::from(rec_len)
        };

        // Check that the rec_len is somewhat reasonable. Too small a
        // value could indicate the wrong data is being read. And
        // notably, a value of zero would cause an infinite loop when
        // iterating over entries.
        if rec_len < NAME_OFFSET {
            return Err(CorruptKind::DirEntryRecordTooSmall(inode, rec_len).into());
        }
        // OK to unwrap: above check ensures that `rec_len >= NAME_OFFSET`.
        if rec_len > bytes.len() || rec_len % 4 != 0 {
            return Err(CorruptKind::DirEntry(inode).into());
        }
        let rec_len = NonZero::new(rec_len).unwrap();

        // As described above, an inode of zero is used for special
        // entries. Return early since the rest of the fields won't be
        // valid.
        let Some(points_to_inode) = InodeIndex::new(points_to_inode) else {
            return Ok((None, rec_len));
        };

        // Get the size of the entry's name field.
        // OK to unwrap: already checked length.
        let has_type =
            fs.0.superblock
                .incompatible_features
                .contains(crate::features::IncompatibleFeatures::FILE_TYPE_IN_DIR_ENTRY);
        let name_length = if has_type {
            u16::from(bytes[6])
        } else {
            read_u16le(bytes, 6)
        };
        let name_len = u8::try_from(name_length).map_err(|_| CorruptKind::DirEntry(inode))?;
        let name_len_usize = usize::from(name_len);

        // OK to unwrap: `NAME_OFFSET` is 8 and `name_len_usize` is
        // at most 255, so the result fits in a `u16`, which is the
        // minimum size of `usize`.
        let name_end: usize = NAME_OFFSET.checked_add(name_len_usize).unwrap();

        // Get the entry's name.
        if name_end > rec_len.get() {
            return Err(CorruptKind::DirEntry(inode).into());
        }
        let name_slice = bytes
            .get(NAME_OFFSET..name_end)
            .ok_or(CorruptKind::DirEntryNameTooLarge(inode, name_len))?;

        // Legacy records have a 16-bit name length. FILETYPE records may
        // also use UNKNOWN (zero); in both cases consult the target inode.
        let file_type = bytes[7];
        let file_type = if !has_type || file_type == 0 {
            Inode::read(&fs, points_to_inode)?.metadata.file_type
        } else {
            FileType::from_dir_entry(file_type)
                .map_err(|_| CorruptKind::DirEntryInvalidFileType(inode, file_type))?
        };

        let name = DirEntryNameBuf::try_from(name_slice)
            .map_err(|e| CorruptKind::DirEntryInvalidName(inode, e))?;
        let entry = Self {
            fs,
            inode: points_to_inode,
            name,
            path,
            file_type,
        };
        Ok((Some(entry), rec_len))
    }

    /// Get the directory entry's name.
    #[must_use]
    #[inline]
    pub fn file_name(&self) -> DirEntryName<'_> {
        self.name.as_dir_entry_name()
    }

    /// Get the inode number this entry points to.
    ///
    /// lovelycarve uses inode numbers as stable identifiers in its
    /// filesystem tree, so it needs this even though upstream `std::fs`
    /// has no equivalent.
    #[must_use]
    #[inline]
    pub fn inode_index(&self) -> u32 {
        self.inode.get()
    }

    /// Get the entry's path.
    ///
    /// This appends the entry's name to the path that `Ext4::read_dir`
    /// was called with.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.path.join(self.name.as_bytes())
    }

    /// Get the entry's file type.
    pub fn file_type(&self) -> Result<FileType, Ext4Error> {
        // Currently this function cannot fail, but return a `Result` to
        // preserve that option for the future (may be needed for
        // filesystems without `FILE_TYPE_IN_DIR_ENTRY`). This also
        // matches the `std::fs::DirEntry` API.
        Ok(self.file_type)
    }

    /// Get [`Metadata`] for the entry.
    ///
    /// If the entry is a symlink, metadata for the symlink itself will
    /// be returned, not the symlink target.
    pub fn metadata(&self) -> Result<Metadata, Ext4Error> {
        let inode = Inode::read(&self.fs, self.inode)?;
        Ok(inode.metadata)
    }
}

impl Debug for DirEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DirEntry").field(&self.path()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::DefaultHasher;

    #[test]
    fn test_dir_entry_debug() {
        let src = "abc😁\n".as_bytes();
        let expected = r#""abc😁\n""#; // Note the escaped slash.
        assert_eq!(format!("{:?}", DirEntryName(src)), expected);

        let mut src_vec = src.to_vec();
        src_vec.resize(255, 0);
        assert_eq!(
            format!(
                "{:?}",
                DirEntryNameBuf {
                    data: src_vec.try_into().unwrap(),
                    len: src.len().try_into().unwrap(),
                }
            ),
            expected
        );
    }

    #[test]
    fn test_dir_entry_display() {
        let name = DirEntryName([0xc3, 0x28].as_slice());
        assert_eq!(format!("{}", name.display()), "�(");
    }

    #[test]
    fn test_dir_entry_construction() {
        let expected_name = DirEntryName(b"abc");
        let mut v = b"abc".to_vec();
        v.resize(255, 0);
        let expected_name_buf = DirEntryNameBuf {
            data: v.try_into().unwrap(),
            len: 3,
        };

        // Successful construction from a byte slice.
        let src: &[u8] = b"abc";
        assert_eq!(DirEntryName::try_from(src).unwrap(), expected_name);
        assert_eq!(DirEntryNameBuf::try_from(src).unwrap(), expected_name_buf);

        // Successful construction from a string.
        let src: &str = "abc";
        assert_eq!(DirEntryName::try_from(src).unwrap(), expected_name);

        // Successful construction from a byte array.
        let src: &[u8; 3] = b"abc";
        assert_eq!(DirEntryName::try_from(src).unwrap(), expected_name);

        // Error: empty.
        let src: &[u8] = b"";
        assert_eq!(DirEntryName::try_from(src), Err(DirEntryNameError::Empty));
        assert_eq!(
            DirEntryNameBuf::try_from(src),
            Err(DirEntryNameError::Empty)
        );

        // Error: too long.
        let src: &[u8] = [1; 256].as_slice();
        assert_eq!(DirEntryName::try_from(src), Err(DirEntryNameError::TooLong));
        assert_eq!(
            DirEntryNameBuf::try_from(src),
            Err(DirEntryNameError::TooLong)
        );

        // Error:: contains null.
        let src: &[u8] = b"\0".as_slice();
        assert_eq!(
            DirEntryName::try_from(src),
            Err(DirEntryNameError::ContainsNull)
        );
        assert_eq!(
            DirEntryNameBuf::try_from(src),
            Err(DirEntryNameError::ContainsNull)
        );

        // Error: contains separator.
        let src: &[u8] = b"/".as_slice();
        assert_eq!(
            DirEntryName::try_from(src),
            Err(DirEntryNameError::ContainsSeparator)
        );
        assert_eq!(
            DirEntryNameBuf::try_from(src),
            Err(DirEntryNameError::ContainsSeparator)
        );
    }

    #[test]
    fn test_dir_entry_name_buf_hash() {
        fn get_hash<T: Hash>(v: T) -> u64 {
            let mut s = DefaultHasher::new();
            v.hash(&mut s);
            s.finish()
        }

        let name = DirEntryNameBuf::try_from(b"abc".as_slice()).unwrap();
        assert_eq!(get_hash(name), get_hash(b"abc"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_dir_entry_from_bytes() {
        let fs = crate::test_util::load_test_disk1();

        let inode1 = InodeIndex::new(1).unwrap();
        let inode2 = InodeIndex::new(2).unwrap();
        let path = Rc::new(PathBuf::new("path"));

        // Read a normal entry.
        let mut bytes = Vec::new();
        bytes.extend(2u32.to_le_bytes()); // inode
        bytes.extend(72u16.to_le_bytes()); // record length
        bytes.push(3u8); // name length
        bytes.push(1u8); // file type
        bytes.extend("abc".bytes()); // name
        bytes.resize(72, 0u8);
        let (entry, len) = DirEntry::from_bytes(fs.clone(), &bytes, inode1, path.clone()).unwrap();
        let entry = entry.unwrap();
        assert_eq!(len.get(), 72);
        assert_eq!(entry.inode, inode2);
        assert_eq!(
            entry.name,
            DirEntryNameBuf::try_from("abc".as_bytes()).unwrap()
        );
        assert_eq!(entry.path, path);
        assert_eq!(entry.file_type, FileType::Regular);
        assert_eq!(entry.file_name(), "abc");
        assert_eq!(entry.path(), "path/abc");

        // Special entry: inode is zero.
        let mut bytes = Vec::new();
        bytes.extend(0u32.to_le_bytes()); // inode
        bytes.extend(72u16.to_le_bytes()); // record length
        bytes.resize(72, 0u8);
        let (entry, len) = DirEntry::from_bytes(fs.clone(), &bytes, inode1, path.clone()).unwrap();
        assert!(entry.is_none());
        assert_eq!(len.get(), 72);

        // Error: not enough data for the header.
        assert_eq!(
            DirEntry::from_bytes(fs.clone(), &[], inode1, path.clone()).unwrap_err(),
            CorruptKind::DirEntryMissingHeader(inode1, 0)
        );

        // Error: not enough data for a full record.
        let mut bytes = Vec::new();
        bytes.extend(2u32.to_le_bytes()); // inode
        bytes.extend(7u16.to_le_bytes()); // record length
        bytes.resize(72, 0u8);
        assert_eq!(
            DirEntry::from_bytes(fs.clone(), &bytes, inode1, path.clone()).unwrap_err(),
            CorruptKind::DirEntryRecordTooSmall(inode1, 7)
        );

        // Error: invalid file type.
        let mut bytes = Vec::new();
        bytes.extend(2u32.to_le_bytes()); // inode
        bytes.extend(72u16.to_le_bytes()); // record length
        bytes.push(3u8); // name length
        bytes.push(123u8); // file type
        bytes.extend("abc".bytes()); // name
        bytes.resize(72, 0u8);
        assert_eq!(
            DirEntry::from_bytes(fs.clone(), &bytes, inode1, path.clone()).unwrap_err(),
            CorruptKind::DirEntryInvalidFileType(inode1, 123),
        );

        // Error: not enough data for the name.
        let mut bytes = Vec::new();
        bytes.extend(2u32.to_le_bytes()); // inode
        bytes.extend(72u16.to_le_bytes()); // record length
        bytes.push(3u8); // name length
        bytes.push(1u8); // file type
        bytes.extend("a".bytes()); // name
        assert_eq!(
            DirEntry::from_bytes(fs.clone(), &bytes, inode1, path.clone()).unwrap_err(),
            CorruptKind::DirEntry(inode1),
        );

        // Error: name contains invalid characters.
        let mut bytes = Vec::new();
        bytes.extend(2u32.to_le_bytes()); // inode
        bytes.extend(72u16.to_le_bytes()); // record length
        bytes.push(3u8); // name length
        bytes.push(1u8); // file type
        bytes.extend("ab/".bytes()); // name
        bytes.resize(72, 0u8);
        assert_eq!(
            DirEntry::from_bytes(fs.clone(), &bytes, inode1, path).unwrap_err(),
            CorruptKind::DirEntryInvalidName(inode1, DirEntryNameError::ContainsSeparator),
        );
    }

    #[test]
    fn test_dir_entry_name_as_ref() {
        let name = DirEntryName::try_from(b"abc".as_slice()).unwrap();
        let bytes: &[u8] = name.as_ref();
        assert_eq!(bytes, b"abc");
    }

    #[test]
    fn test_dir_entry_name_partial_eq() {
        let name = DirEntryName::try_from(b"abc".as_slice()).unwrap();
        assert_eq!(name, name);

        let v: &str = "abc";
        assert_eq!(name, v);

        let v: &[u8] = b"abc";
        assert_eq!(name, v);

        let v: &[u8; 3] = b"abc";
        assert_eq!(name, v);
    }

    #[test]
    fn test_dir_entry_name_buf_as_dir_entry_name() {
        let name = DirEntryNameBuf::try_from(b"abc".as_slice()).unwrap();
        let r: DirEntryName<'_> = name.as_dir_entry_name();
        assert_eq!(r, "abc");
    }

    #[test]
    fn test_dir_entry_name_as_str() {
        let name = DirEntryName::try_from(b"abc".as_slice()).unwrap();
        assert_eq!(name.as_str().unwrap(), "abc");

        let name = DirEntryName([0xc3, 0x28].as_slice());
        assert!(name.as_str().is_err());
    }
}
