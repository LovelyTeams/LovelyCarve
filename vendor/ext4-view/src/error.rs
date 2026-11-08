// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::block_index::FsBlockIndex;
use crate::block_size::BlockSize;
use crate::dir_entry::DirEntryNameError;
use crate::features::IncompatibleFeatures;
use crate::inode::{InodeIndex, InodeMode};
use alloc::boxed::Box;
use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter};
use core::num::NonZero;

/// Boxed error, used for IO errors. This is similar in spirit to
/// `anyhow::Error`, although a much simpler implementation.
pub(crate) type BoxedError = Box<dyn Error + Send + Sync + 'static>;

/// Common error type for all [`Ext4`] operations.
///
/// [`Ext4`]: crate::Ext4
#[derive(Debug)]
#[non_exhaustive]
pub enum Ext4Error {
    /// An operation that requires an absolute path was attempted on a
    /// relative path.
    NotAbsolute,

    /// An operation that requires a symlink was attempted on a
    /// non-symlink file.
    NotASymlink,

    /// A path points to a non-existent file.
    NotFound,

    /// An operation that requires a non-directory path was attempted on
    /// a directory path.
    IsADirectory,

    /// An operation that requires a directory path was attempted on a
    /// non-directory path.
    NotADirectory,

    /// An operation that requires a regular file (or a symlink to a
    /// regular file) was attempted on a special file (fifo, character
    /// device, block device, or socket).
    IsASpecialFile,

    /// The file cannot be read into memory because it is too large.
    FileTooLarge,

    /// Data is not valid UTF-8.
    NotUtf8,

    /// Data cannot be converted into a valid path.
    MalformedPath,

    /// Path is too long.
    ///
    /// Maximum path length is not strictly enforced by this library for
    /// all paths, but during path resolution the length may not exceed
    /// 4096 bytes.
    PathTooLong,

    /// Path could not be resolved because it contains too many levels
    /// of symbolic links.
    TooManySymlinks,

    /// Attempted to read an encrypted file.
    ///
    /// Only unencrypted files are currently supported. Please file an
    /// [issue] if you have a use case for reading encrypted files.
    ///
    /// [issue]: https://github.com/nicholasbishop/ext4-view-rs/issues/new
    Encrypted,

    /// An IO operation failed. This error comes from the [`Ext4Read`]
    /// passed to [`Ext4::load`].
    ///
    /// [`Ext4::load`]: crate::Ext4::load
    /// [`Ext4Read`]: crate::Ext4Read
    Io(
        /// Underlying error.
        BoxedError,
    ),

    /// The filesystem is not supported by this library. This does not
    /// indicate a problem with the filesystem, or with the calling
    /// code. Please file a feature request and include the incompatible
    /// features.
    Incompatible(Incompatible),

    /// The filesystem is corrupt in some way.
    Corrupt(Corrupt),
}

impl Ext4Error {
    /// If the error type is [`Ext4Error::Io`], get the underlying error.
    #[must_use]
    pub fn as_io(&self) -> Option<&(dyn Error + Send + Sync + 'static)> {
        if let Self::Io(err) = self {
            Some(&**err)
        } else {
            None
        }
    }
}

impl Display for Ext4Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => write!(f, "path is not absolute"),
            Self::NotASymlink => write!(f, "path is not a symlink"),
            Self::NotFound => write!(f, "file not found"),
            Self::IsADirectory => write!(f, "path is a directory"),
            Self::NotADirectory => write!(f, "path is not a directory"),
            Self::IsASpecialFile => write!(f, "path is a special file"),
            Self::FileTooLarge => {
                write!(f, "file is too large to store in memory")
            }
            Self::NotUtf8 => write!(f, "data is not utf-8"),
            Self::MalformedPath => write!(f, "data is not a valid path"),
            Self::PathTooLong => write!(f, "path is too long"),
            Self::TooManySymlinks => {
                write!(f, "too many levels of symbolic links")
            }
            Self::Encrypted => write!(f, "file is encrypted"),
            // TODO: if the `Error` trait ever makes it into core, stop
            // printing `err` here and return it via `Error::source` instead.
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Incompatible(i) => write!(f, "incompatible filesystem: {i}"),
            Self::Corrupt(c) => write!(f, "corrupt filesystem: {c}"),
        }
    }
}

impl Error for Ext4Error {}

#[cfg(feature = "std")]
impl From<Ext4Error> for std::io::Error {
    fn from(e: Ext4Error) -> Self {
        use std::io::ErrorKind::*;

        // TODO: Rust 1.87 adds InvalidFilename; use after bumping the MSRV.
        match e {
            Ext4Error::IsASpecialFile
            | Ext4Error::MalformedPath
            | Ext4Error::NotASymlink
            | Ext4Error::NotAbsolute => InvalidInput.into(),

            Ext4Error::Corrupt(_)
            | Ext4Error::Incompatible(_)
            | Ext4Error::PathTooLong
            | Ext4Error::TooManySymlinks => Self::other(e),

            Ext4Error::FileTooLarge => FileTooLarge.into(),
            Ext4Error::Io(inner) => Self::other(inner),
            Ext4Error::IsADirectory => IsADirectory.into(),
            Ext4Error::NotADirectory => NotADirectory.into(),
            Ext4Error::NotFound => NotFound.into(),
            Ext4Error::NotUtf8 => InvalidData.into(),
            Ext4Error::Encrypted => PermissionDenied.into(),
        }
    }
}

/// Error type used in [`Ext4Error::Corrupt`] when the filesystem is
/// corrupt in some way.
#[derive(Clone, Eq, PartialEq)]
pub struct Corrupt(CorruptKind);

impl Debug for Corrupt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <CorruptKind as Debug>::fmt(&self.0, f)
    }
}

impl Display for Corrupt {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <CorruptKind as Display>::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum CorruptKind {
    /// Superblock magic is invalid.
    SuperblockMagic,

    /// Superblock checksum is invalid.
    SuperblockChecksum,

    /// The block size in the superblock is invalid.
    InvalidBlockSize,

    /// The number of block groups does not fit in a [`u32`].
    TooManyBlockGroups,

    /// The number of inodes per block group is zero.
    InodesPerBlockGroup,

    /// The block group descriptor size in the superblock is invalid.
    BlockGroupDescriptorSize,

    /// The number of blocks per group is zero.
    BlocksPerGroup,

    /// The inode size exceeds the block size.
    InodeSize,

    /// The journal inode in the superblock is invalid.
    JournalInode,

    /// Invalid first data block.
    FirstDataBlock(
        /// First data block.
        u32,
    ),

    /// Invalid block group descriptor.
    BlockGroupDescriptor(
        /// Block group number.
        u32,
    ),

    /// Block group descriptor checksum is invalid.
    BlockGroupDescriptorChecksum(
        /// Block group number.
        u32,
    ),

    /// Journal size is invalid.
    JournalSize,

    /// Journal magic is invalid.
    JournalMagic,

    /// Journal superblock checksum is invalid.
    JournalSuperblockChecksum,

    /// Journal block size does not match the filesystem block size.
    JournalBlockSize,

    /// Journal does not have the expected number of blocks.
    JournalTruncated,

    /// Journal first commit doesn't match the sequence number in the superblock.
    JournalSequence,

    /// Journal commit block checksum is invalid.
    JournalCommitBlockChecksum,

    /// Journal descriptor block checksum is invalid.
    JournalDescriptorBlockChecksum,

    /// Journal descriptor tag checksum is invalid.
    JournalDescriptorTagChecksum,

    /// Journal revocation block checksum is invalid.
    JournalRevocationBlockChecksum,

    /// Journal revocation block has an invalid table size.
    JournalRevocationBlockInvalidTableSize(usize),

    /// Journal sequence number overflowed.
    JournalSequenceOverflow,

    /// Journal has a truncated descriptor block. Either it is missing a
    /// tag with the `LAST_TAG` flag set, or the final tag does have
    /// that flag set but there are not enough bytes to read the full
    /// tag.
    JournalDescriptorBlockTruncated,

    /// An inode's checksum is invalid.
    InodeChecksum(InodeIndex),

    /// An inode is too small.
    InodeTruncated { inode: InodeIndex, size: usize },

    /// An inode's block group is invalid.
    InodeBlockGroup {
        inode: InodeIndex,
        block_group: u32,
        num_block_groups: usize,
    },

    /// Failed to calculate an inode's location.
    ///
    /// This error can be returned by various calculations in
    /// `get_inode_location`. The fields here are sufficient to
    /// reconstruct which specific calculation failed.
    InodeLocation {
        inode: InodeIndex,
        block_group: u32,
        inodes_per_block_group: NonZero<u32>,
        inode_size: u16,
        block_size: BlockSize,
        inode_table_first_block: FsBlockIndex,
    },

    /// An inode's file type is invalid.
    InodeFileType { inode: InodeIndex, mode: InodeMode },

    /// An inode's timestamp is invalid.
    InodeTimestamp { inode: InodeIndex },

    /// The target of a symlink is not a valid path.
    SymlinkTarget(InodeIndex),

    /// Invalid extended attribute table or value.
    Xattr(InodeIndex),

    /// Inline payload is missing, truncated, or inconsistent.
    InlineData(InodeIndex),

    /// The number of blocks in a file exceeds 2^32.
    TooManyBlocksInFile,

    /// An extent's magic is invalid.
    ExtentMagic(InodeIndex),

    /// An extent's checksum is invalid.
    ExtentChecksum(InodeIndex),

    /// An extent's depth is greater than five.
    ExtentDepth(InodeIndex),

    /// Not enough data is present to read an extent node.
    ExtentNotEnoughData(InodeIndex),

    /// An extent points to an invalid block.
    ExtentBlock(InodeIndex),

    /// An extent node's size exceeds the block size.
    ExtentNodeSize(InodeIndex),

    /// A directory block's checksum is invalid.
    DirBlockChecksum(InodeIndex),

    /// A directory entry is too small to contain the required header.
    DirEntryMissingHeader(InodeIndex, usize),

    /// A directory entry's length field is too small.
    DirEntryRecordTooSmall(InodeIndex, usize),

    /// A directory entry's name length is too large.
    DirEntryNameTooLarge(InodeIndex, u8),

    /// A directory entry's file type is invalid.
    DirEntryInvalidFileType(InodeIndex, u8),

    /// A directory entry's name is invalid.
    DirEntryInvalidName(InodeIndex, DirEntryNameError),

    /// An htree internal node is too small to contain the required header.
    HtreeInternalNodeMissingHeader { inode: InodeIndex, num_bytes: usize },

    /// An htree internal node specifies a count that is larger than the
    /// available data.
    HtreeInternalNodeCountTooLarge {
        inode: InodeIndex,
        num_bytes: usize,
        count: usize,
    },

    // TODO: consider breaking this down into more specific problems.
    /// A directory entry is invalid.
    DirEntry(InodeIndex),

    /// Invalid read of a block.
    BlockRead {
        /// Absolute block index.
        block_index: FsBlockIndex,

        /// Absolute block index, without remapping from the journal. If
        /// this block was not remapped by the journal, this field will
        /// be the same as `block_index`.
        original_block_index: FsBlockIndex,

        /// Offset in bytes within the block.
        offset_within_block: u32,

        /// Length in bytes of the read.
        read_len: usize,
    },

    /// Attempting to read too much data in the block cache.
    BlockCacheReadTooLarge {
        num_blocks: u32,
        block_size: BlockSize,
    },
}

impl Display for CorruptKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SuperblockMagic => write!(f, "invalid superblock magic"),
            Self::SuperblockChecksum => {
                write!(f, "invalid superblock checksum")
            }
            Self::InvalidBlockSize => write!(f, "invalid block size"),
            Self::TooManyBlockGroups => write!(f, "too many block groups"),
            Self::InodesPerBlockGroup => {
                write!(f, "inodes per block group is zero")
            }
            Self::BlockGroupDescriptorSize => {
                write!(f, "invalid block group descriptor size")
            }
            Self::BlocksPerGroup => {
                write!(f, "blocks per group is zero")
            }
            Self::InodeSize => write!(f, "inode size is invalid"),
            Self::JournalInode => write!(f, "invalid journal inode"),
            Self::FirstDataBlock(block) => {
                write!(f, "invalid first data block: {block}")
            }
            Self::BlockGroupDescriptor(block_group_num) => {
                write!(f, "block group descriptor {block_group_num} is invalid")
            }
            Self::BlockGroupDescriptorChecksum(block_group_num) => write!(
                f,
                "invalid checksum for block group descriptor {block_group_num}"
            ),
            Self::JournalSize => {
                write!(f, "journal size is invalid")
            }
            Self::JournalMagic => {
                write!(f, "journal magic is invalid")
            }
            Self::JournalSuperblockChecksum => {
                write!(f, "journal superblock checksum is invalid")
            }
            Self::JournalBlockSize => {
                write!(f, "journal block size does not match filesystem block size")
            }
            Self::JournalTruncated => write!(f, "journal is truncated"),
            Self::JournalSequence => write!(
                f,
                "journal's first commit doesn't match the expected sequence"
            ),
            Self::JournalCommitBlockChecksum => {
                write!(f, "journal commit block checksum is invalid")
            }
            Self::JournalDescriptorBlockChecksum => {
                write!(f, "journal descriptor block checksum is invalid")
            }
            Self::JournalDescriptorTagChecksum => {
                write!(f, "journal descriptor tag checksum is invalid")
            }
            Self::JournalRevocationBlockChecksum => {
                write!(f, "journal revocation block checksum is invalid")
            }
            Self::JournalRevocationBlockInvalidTableSize(size) => {
                write!(f, "journal revocation block table size is invalid: {size}")
            }
            Self::JournalSequenceOverflow => {
                write!(f, "journal sequence number overflowed")
            }
            Self::JournalDescriptorBlockTruncated => {
                write!(f, "journal descriptor block is truncated")
            }
            Self::InodeChecksum(inode) => {
                write!(f, "invalid checksum for inode {inode}")
            }
            Self::InodeTruncated { inode, size } => {
                write!(f, "inode {inode} is truncated: size={size}")
            }
            Self::InodeBlockGroup {
                inode,
                block_group,
                num_block_groups,
            } => {
                write!(
                    f,
                    "inode {inode} has an invalid block group index: block_group={block_group}, num_block_groups={num_block_groups}"
                )
            }
            Self::InodeLocation {
                inode,
                block_group,
                inodes_per_block_group,
                inode_size,
                block_size,
                inode_table_first_block,
            } => {
                write!(
                    f,
                    "inode {inode} has invalid location: block_group={block_group}, inodes_per_block_group={inodes_per_block_group}, inode_size={inode_size}, block_size={block_size}, inode_table_first_block={inode_table_first_block}"
                )
            }
            Self::InodeFileType { inode, mode } => {
                write!(
                    f,
                    "inode {inode} has invalid file type: mode=0x{mode:04x}",
                    mode = mode.bits()
                )
            }
            Self::InodeTimestamp { inode } => {
                write!(f, "inode {inode} has invalid timestamp",)
            }
            Self::Xattr(inode) => write!(f, "invalid extended attributes for inode {inode}"),
            Self::InlineData(inode) => write!(f, "invalid inline data for inode {inode}"),
            Self::SymlinkTarget(inode) => {
                write!(f, "inode {inode} has an invalid symlink path")
            }
            Self::TooManyBlocksInFile => write!(f, "too many blocks in file"),
            Self::ExtentMagic(inode) => {
                write!(f, "extent in inode {inode} has invalid magic")
            }
            Self::ExtentChecksum(inode) => {
                write!(f, "extent in inode {inode} has an invalid checksum")
            }
            Self::ExtentDepth(inode) => {
                write!(f, "extent in inode {inode} has an invalid depth")
            }
            Self::ExtentNotEnoughData(inode) => {
                write!(f, "extent data in inode {inode} is invalid")
            }
            Self::ExtentBlock(inode) => {
                write!(f, "extent in inode {inode} points to an invalid block")
            }
            Self::ExtentNodeSize(inode) => {
                write!(f, "extent in inode {inode} has a node with an invalid size")
            }
            Self::DirBlockChecksum(inode) => write!(
                f,
                "directory block in inode {inode} has an invalid checksum"
            ),
            Self::DirEntryMissingHeader(inode, num_bytes) => {
                write!(
                    f,
                    "directory in inode {inode} is too small to contain header: {num_bytes}"
                )
            }
            Self::DirEntryRecordTooSmall(inode, len) => {
                write!(
                    f,
                    "directory entry in inode {inode} record length is too small: {len}"
                )
            }
            Self::DirEntryNameTooLarge(inode, len) => {
                write!(
                    f,
                    "name of directory entry in inode {inode} is too large: {len}"
                )
            }
            Self::DirEntryInvalidFileType(inode, value) => {
                write!(
                    f,
                    "directory entry in inode {inode} has invalid file type: {value}"
                )
            }
            Self::DirEntryInvalidName(inode, err) => {
                write!(
                    f,
                    "directory entry in inode {inode} has invalid name: {err}"
                )
            }
            Self::HtreeInternalNodeMissingHeader { inode, num_bytes } => {
                write!(
                    f,
                    "htree internal node in inode {inode} is too small to contain header: {num_bytes}"
                )
            }
            Self::HtreeInternalNodeCountTooLarge {
                inode,
                num_bytes,
                count,
            } => {
                write!(
                    f,
                    "htree internal node in inode {inode} has too large many entries: num_bytes={num_bytes}, count={count}"
                )
            }
            Self::DirEntry(inode) => {
                write!(f, "invalid directory entry in inode {inode}")
            }
            Self::BlockRead {
                block_index,
                original_block_index,
                offset_within_block,
                read_len,
            } => {
                write!(
                    f,
                    "invalid read of length {read_len} from block {block_index} (originally {original_block_index}) at offset {offset_within_block}"
                )
            }
            Self::BlockCacheReadTooLarge {
                num_blocks,
                block_size,
            } => write!(
                f,
                "attempted to read {num_blocks} blocks with block_size {block_size}"
            ),
        }
    }
}

impl PartialEq<CorruptKind> for Ext4Error {
    fn eq(&self, ck: &CorruptKind) -> bool {
        if let Self::Corrupt(c) = self {
            c.0 == *ck
        } else {
            false
        }
    }
}

impl From<CorruptKind> for Ext4Error {
    fn from(c: CorruptKind) -> Self {
        Self::Corrupt(Corrupt(c))
    }
}

/// Error type used in [`Ext4Error::Incompatible`] when the filesystem
/// cannot be read due to incomplete support in this library.
#[derive(Clone, Eq, PartialEq)]
pub struct Incompatible(IncompatibleKind);

impl Debug for Incompatible {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <IncompatibleKind as Debug>::fmt(&self.0, f)
    }
}

impl Display for Incompatible {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <IncompatibleKind as Display>::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum IncompatibleKind {
    /// One or more required features are missing.
    MissingRequiredFeatures(
        /// The missing features.
        IncompatibleFeatures,
    ),

    /// One or more unsupported features are present.
    #[allow(clippy::enum_variant_names)]
    UnsupportedFeatures(
        /// The unsupported features.
        IncompatibleFeatures,
    ),

    /// The directory hash algorithm is not supported.
    DirectoryHash(
        /// The algorithm identifier.
        u8,
    ),

    /// The journal superblock type is not supported.
    JournalSuperblockType(
        /// Raw journal block type.
        u32,
    ),

    /// The journal checksum type is not supported.
    JournalChecksumType(
        /// Raw journal checksum type.
        u8,
    ),

    /// One or more required journal features are missing.
    MissingRequiredJournalFeatures(
        /// The missing feature bits.
        u32,
    ),

    /// One or more unsupported journal features are present.
    #[allow(clippy::enum_variant_names)]
    UnsupportedJournalFeatures(
        /// The unsupported feature bits.
        u32,
    ),

    /// The journal contains an unsupported block type.
    JournalBlockType(
        /// Raw journal block type.
        u32,
    ),

    /// The journal contains an escaped block.
    JournalBlockEscaped,
}

impl Display for IncompatibleKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredFeatures(feat) => {
                write!(f, "missing required features: {feat:?}")
            }
            Self::UnsupportedFeatures(feat) => {
                write!(f, "unsupported features: {feat:?}")
            }
            Self::DirectoryHash(algorithm) => {
                write!(f, "unsupported directory hash algorithm: {algorithm}")
            }
            Self::JournalSuperblockType(val) => {
                write!(f, "journal superblock type is not supported: {val}")
            }
            Self::JournalBlockType(val) => {
                write!(f, "journal block type is not supported: {val}")
            }
            Self::JournalBlockEscaped => {
                write!(f, "journal contains an escaped data block")
            }
            Self::JournalChecksumType(val) => {
                write!(f, "journal checksum type is not supported: {val}")
            }
            Self::MissingRequiredJournalFeatures(feat) => {
                write!(f, "missing required journal features: {feat:?}")
            }
            Self::UnsupportedJournalFeatures(feat) => {
                write!(f, "unsupported journal features: {feat:?}")
            }
        }
    }
}

impl PartialEq<IncompatibleKind> for Ext4Error {
    fn eq(&self, other: &IncompatibleKind) -> bool {
        if let Self::Incompatible(Incompatible(i)) = self {
            i == other
        } else {
            false
        }
    }
}

impl From<IncompatibleKind> for Ext4Error {
    fn from(k: IncompatibleKind) -> Self {
        Self::Incompatible(Incompatible(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test the `Display` and `Debug` impls for a corruption error.
    ///
    /// Only one `CorruptKind` variant is tested, the focus of the test
    /// is the formatting of the nested error type:
    /// `Ext4Error::Corrupt(Corrupt(CorruptKind))`
    #[test]
    fn test_corrupt_format() {
        let err: Ext4Error = CorruptKind::BlockRead {
            block_index: 123,
            original_block_index: 124,
            offset_within_block: 456,
            read_len: 789,
        }
        .into();

        assert_eq!(
            format!("{err}"),
            "corrupt filesystem: invalid read of length 789 from block 123 (originally 124) at offset 456"
        );

        assert_eq!(
            format!("{err:?}"),
            "Corrupt(BlockRead { block_index: 123, original_block_index: 124, offset_within_block: 456, read_len: 789 })"
        );
    }

    /// Test the `Display` and `Debug` impls for an `Incompatible` error.
    ///
    /// Only one `IncompatibleKind` variant is tested, the focus of the test
    /// is the formatting of the nested error type:
    /// `Ext4Error::Incompatible(Incompatible(IncompatibleKind))`
    #[test]
    fn test_incompatible_format() {
        let err: Ext4Error = IncompatibleKind::DirectoryHash(123).into();

        assert_eq!(
            format!("{err}"),
            "incompatible filesystem: unsupported directory hash algorithm: 123"
        );

        assert_eq!(format!("{err:?}"), "Incompatible(DirectoryHash(123))");
    }
}
