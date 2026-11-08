// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! This crate provides read-only access to [ext4] filesystems. It also
//! works with [ext2] filesystems.
//!
//! The main entry point is the [`Ext4`] struct.
//!
//! [ext2]: https://en.wikipedia.org/wiki/Ext2
//! [ext4]: https://en.wikipedia.org/wiki/Ext4
//!
//! # Example
//!
//! This example reads the filesystem data from a byte vector, then
//! looks at files and directories in the filesystem.
//!
//! ```
//! use ext4_view::{Ext4, Ext4Error, Metadata};
//!
//! fn in_memory_example(fs_data: Vec<u8>) -> Result<(), Ext4Error> {
//!     let fs = Ext4::load(Box::new(fs_data)).unwrap();
//!
//!     let path = "/some/file";
//!
//!     // Read a file's contents.
//!     let file_data: Vec<u8> = fs.read(path)?;
//!
//!     // Read a file's contents as a string.
//!     let file_str: String = fs.read_to_string(path)?;
//!
//!     // Check whether a path exists.
//!     let exists: bool = fs.exists(path)?;
//!
//!     // Get metadata (file type, permissions, etc).
//!     let metadata: Metadata = fs.metadata(path)?;
//!
//!     // Print each entry in a directory.
//!     for entry in fs.read_dir("/some/dir")? {
//!         let entry = entry?;
//!         println!("{}", entry.path().display());
//!     }
//!
//!     Ok(())
//! }
//! ```
//! # Loading a filesystem
//!
//! Call [`Ext4::load`] to load a filesystem. The source data can be
//! anything that implements the [`Ext4Read`] trait. The simplest form
//! of source data is a `Vec<u8>` containing the whole filesystem.
//!
//! If the `std` feature is enabled, [`Ext4Read`] is implemented for
//! [`std::fs::File`]. As a shortcut, you can also use
//! [`Ext4::load_from_path`] to open a path and read the filesystem from
//! it.
//!
//! For other cases, implement [`Ext4Read`] for your data source. This
//! trait has a single method which reads bytes into a byte slice.
//!
//! Note that the underlying data should never be changed while the
//! filesystem is in use.
//!
//! # Paths
//!
//! Paths in the filesystem are represented by [`Path`] and
//! [`PathBuf`]. These types are similar to the types of the same names
//! in [`std::path`].
//!
//! Functions that take a path as input accept a variety of types
//! including strings.
//!
//! # Errors
//!
//! Most functions return [`Ext4Error`] on failure. This type is broadly
//! similar to [`std::io::Error`], with a few notable additions:
//! * Errors that come from the underlying reader are returned as
//!   [`Ext4Error::Io`].
//! * If the filesystem is corrupt in some way, [`Ext4Error::Corrupt`]
//!   is returned.
//! * If the filesystem can't be read due to a limitation of the
//!   library, [`Ext4Error::Incompatible`] is returned. Please [file a
//!   bug][issues] if you encounter an incompatibility so we know to
//!   prioritize a fix!
//!
//! Some functions list specific errors that may occur. These lists are
//! not exhaustive; calling code should be prepared to handle other
//! errors such as [`Ext4Error::Io`].
//!
//! [issues]: https://github.com/nicholasbishop/ext4-view-rs/issues

#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::must_use_candidate,
    clippy::use_self
)]
#![warn(missing_docs)]
#![warn(unreachable_pub)]

extern crate alloc;

mod block_cache;
mod block_group;
mod block_index;
mod block_size;
mod checksum;
mod dir;
mod dir_block;
mod dir_entry;
mod dir_entry_hash;
mod dir_htree;
mod error;
mod extent;
mod features;
mod file;
mod file_type;
mod format;
mod inode;
mod iters;
mod journal;
mod label;
mod metadata;
mod path;
mod reader;
mod resolve;
mod superblock;
mod timestamp;
mod util;
mod uuid;
mod volume_info;
mod xattr;
pub use volume_info::VolumeInfo;
pub use xattr::ExtendedAttribute;

#[cfg(all(test, feature = "std"))]
mod test_util;

#[cfg(all(test, feature = "std"))]
mod audit_tests;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use block_cache::BlockCache;
use block_group::BlockGroupDescriptor;
use block_index::FsBlockIndex;
use core::cell::RefCell;
use core::fmt::{self, Debug, Formatter};
use error::CorruptKind;
use features::ReadOnlyCompatibleFeatures;
use inode::{Inode, InodeIndex};
use journal::Journal;
use resolve::FollowSymlinks;
use superblock::Superblock;
use util::usize_from_u32;

pub use dir_entry::{DirEntry, DirEntryName, DirEntryNameError};
pub use error::{Corrupt, Ext4Error, Incompatible};
pub use features::IncompatibleFeatures;
pub use file::File;
pub use file_type::FileType;
pub use format::BytesDisplay;
pub use iters::read_dir::ReadDir;
pub use label::Label;
pub use metadata::Metadata;
pub use path::{Component, Components, Path, PathBuf, PathError};
pub use reader::{Ext4Read, MemIoError};
pub use timestamp::Timestamp;
pub use uuid::Uuid;

struct Ext4Inner {
    superblock: Superblock,
    block_group_descriptors: Vec<BlockGroupDescriptor>,
    journal: Journal,
    block_cache: RefCell<BlockCache>,

    /// Reader providing access to the underlying storage.
    ///
    /// Stored as `Box<dyn Ext4Read>` rather than a generic type to make
    /// the `Ext4` type more convenient to pass around for users of the API.
    ///
    /// The `Ext4Read::read` method takes `&mut self`, because readers
    /// like `std::fs::File` are mutable. However, the `Ext4` API is
    /// logically const -- it provides read-only access to the
    /// filesystem. So the box is wrapped in `RefCell` to allow the
    /// mutable method to be called with an immutable `&Ext4Inner`
    /// reference. `RefCell` enforces at runtime that only one mutable
    /// borrow exists at a time.
    reader: RefCell<Box<dyn Ext4Read>>,
}

/// Read-only access to an [ext4] filesystem.
///
/// [ext4]: https://en.wikipedia.org/wiki/Ext4
#[derive(Clone)]
pub struct Ext4(Rc<Ext4Inner>);

impl Ext4 {
    /// Load an `Ext4` instance from the given `reader`.
    ///
    /// This reads and validates the superblock, block group
    /// descriptors, and journal. No other data is read.
    pub fn load(mut reader: Box<dyn Ext4Read>) -> Result<Self, Ext4Error> {
        // The first 1024 bytes are reserved for "weird" stuff like x86
        // boot sectors.
        let superblock_start = 1024;
        let mut data = vec![0; Superblock::SIZE_IN_BYTES_ON_DISK];
        reader
            .read(superblock_start, &mut data)
            .map_err(Ext4Error::Io)?;

        let superblock = Superblock::from_bytes(&data)?;
        let block_cache = BlockCache::new(superblock.block_size, superblock.blocks_count)?;

        let mut fs = Self(Rc::new(Ext4Inner {
            block_group_descriptors: BlockGroupDescriptor::read_all(&superblock, &mut *reader)?,
            reader: RefCell::new(reader),
            superblock,
            // Initialize with an empty journal, because loading the
            // journal requires a valid `Ext4` object.
            journal: Journal::empty(),
            block_cache: RefCell::new(block_cache),
        }));

        // Load the actual journal, if present.
        let journal = Journal::load(&fs)?;
        Rc::get_mut(&mut fs.0).unwrap().journal = journal;

        Ok(fs)
    }

    /// Load an `Ext4` filesystem from the given `path`.
    ///
    /// This reads and validates the superblock and block group
    /// descriptors. No other data is read.
    #[cfg(feature = "std")]
    pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self, Ext4Error> {
        fn inner(path: &std::path::Path) -> Result<Ext4, Ext4Error> {
            let file = std::fs::File::open(path).map_err(|e| Ext4Error::Io(Box::new(e)))?;
            Ext4::load(Box::new(file))
        }

        inner(path.as_ref())
    }

    /// Get the filesystem label.
    #[must_use]
    pub fn label(&self) -> &Label {
        &self.0.superblock.label
    }

    /// Get the filesystem UUID.
    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.0.superblock.uuid
    }

    /// Return true if the filesystem has metadata checksums enabled,
    /// false otherwise.
    fn has_metadata_checksums(&self) -> bool {
        self.0
            .superblock
            .read_only_compatible_features
            .contains(ReadOnlyCompatibleFeatures::METADATA_CHECKSUMS)
    }

    /// Read the inode of the root `/` directory.
    fn read_root_inode(&self) -> Result<Inode, Ext4Error> {
        let root_inode_index = InodeIndex::new(2).unwrap();
        Inode::read(self, root_inode_index)
    }

    /// Read data from a block.
    ///
    /// `block_index`: an absolute block within the filesystem.
    ///
    /// `offset_within_block`: the byte offset within the block to start
    /// reading from.
    ///
    /// `dst`: byte buffer to read into. This also controls the length
    /// of the read.
    ///
    /// The first 1024 bytes of the filesystem are reserved for
    /// non-filesystem data. Reads are not allowed there.
    ///
    /// The read cannot cross block boundaries. This implies that:
    /// * `offset_within_block < block_size`
    /// * `offset_within_block + dst.len() <= block_size`
    ///
    /// If any of these conditions are violated, a `CorruptKind::BlockRead`
    /// error is returned.
    fn read_from_block(
        &self,
        original_block_index: FsBlockIndex,
        offset_within_block: u32,
        dst: &mut [u8],
    ) -> Result<(), Ext4Error> {
        let block_index = self.0.journal.map_block_index(original_block_index);

        let err = || {
            Ext4Error::from(CorruptKind::BlockRead {
                block_index,
                original_block_index,
                offset_within_block,
                read_len: dst.len(),
            })
        };

        // The first 1024 bytes are reserved for non-filesystem
        // data. This conveniently allows for something like a null
        // pointer check.
        if block_index == 0 && offset_within_block < 1024 {
            return Err(err());
        }

        // Check the block index.
        if block_index >= self.0.superblock.blocks_count {
            return Err(err());
        }

        // The start of the read must be less than the block size.
        let block_size = self.0.superblock.block_size;
        if offset_within_block >= block_size {
            return Err(err());
        }

        // The end of the read must be less than or equal to the block size.
        let read_end = usize_from_u32(offset_within_block)
            .checked_add(dst.len())
            .ok_or_else(err)?;
        if read_end > block_size {
            return Err(err());
        }

        let mut block_cache = self.0.block_cache.borrow_mut();
        let cached_block = block_cache.get_or_insert_blocks(block_index, |buf: &mut [u8]| {
            // Get the absolute byte to start reading from.
            let start_byte = block_index
                .checked_mul(block_size.to_u64())
                .ok_or_else(err)?;
            self.0
                .reader
                .borrow_mut()
                .read(start_byte, buf)
                .map_err(Ext4Error::Io)
        })?;

        dst.copy_from_slice(&cached_block[usize_from_u32(offset_within_block)..read_end]);

        Ok(())
    }

    /// Read the entire contents of a file into a `Vec<u8>`.
    ///
    /// Holes are filled with zero.
    ///
    /// Fails with `FileTooLarge` if the size of the file is too large
    /// to fit in a [`usize`].
    fn read_inode_file(&self, inode: &Inode) -> Result<Vec<u8>, Ext4Error> {
        // Get the file size and initialize the output vector.
        let file_size_in_bytes =
            usize::try_from(inode.metadata.size_in_bytes).map_err(|_| Ext4Error::FileTooLarge)?;
        let mut dst = vec![0; file_size_in_bytes];

        // Use `File` to read the data in chunks.
        let mut file = File::open_inode(self, inode.clone())?;
        let mut remaining = dst.as_mut();
        loop {
            let bytes_read = file.read_bytes(remaining)?;
            if bytes_read == 0 {
                break;
            }
            remaining = &mut remaining[bytes_read..];
        }
        Ok(dst)
    }

    /// Follow a path to get an inode.
    fn path_to_inode(&self, path: Path<'_>, follow: FollowSymlinks) -> Result<Inode, Ext4Error> {
        resolve::resolve_path(self, path, follow).map(|v| v.0)
    }
}

/// Shim methods used by lovelycarve's forensic tooling. These let the
/// library be driven by inode index rather than by absolute path so the
/// caller can keep stable inode-based node IDs across directory walks.
impl Ext4 {
    /// Read inode-resident, external-block and EA-inode attributes.
    pub fn extended_attributes(
        &self,
        inode_index: u32,
    ) -> Result<Vec<ExtendedAttribute>, Ext4Error> {
        let index = core::num::NonZeroU32::new(inode_index).ok_or(Ext4Error::NotFound)?;
        let inode = Inode::read(self, index)?;
        xattr::read(self, &inode)
    }

    /// Filesystem block size in bytes (always a power of two `>= 1024`).
    #[must_use]
    pub fn block_size(&self) -> u32 {
        self.0.superblock.block_size.to_u32()
    }

    /// Metadata by inode number, without path resolution.
    pub fn inode_metadata(&self, inode_index: u32) -> Result<Metadata, Ext4Error> {
        let index = InodeIndex::new(inode_index).ok_or(Ext4Error::NotFound)?;
        Ok(Inode::read(self, index)?.metadata)
    }

    /// Read a symbolic link target by inode number without following it.
    pub fn read_link_inode(&self, inode_index: u32) -> Result<PathBuf, Ext4Error> {
        let index = InodeIndex::new(inode_index).ok_or(Ext4Error::NotFound)?;
        Inode::read(self, index)?.symlink_target(self)
    }

    /// Read the raw on-disk inode struct (`superblock.inode_size` bytes).
    ///
    /// The returned bytes are the unparsed inode payload — callers can
    /// extract `i_atime`/`i_mtime`/`i_ctime`/`i_dtime`, `i_flags`, and the
    /// 60-byte `i_block` area without going through the upstream `Inode`
    /// type, which deliberately hides those fields.
    pub fn read_raw_inode(&self, inode_index: u32) -> Result<Vec<u8>, Ext4Error> {
        let index = InodeIndex::new(inode_index).ok_or(Ext4Error::NotFound)?;
        Inode::read_raw_bytes(self, index)
    }

    /// Open a file by inode index. Behaves like [`Ext4::open`] but skips
    /// path resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the inode does not point to a regular file.
    pub fn open_inode_by_index(&self, inode_index: u32) -> Result<File, Ext4Error> {
        let index = InodeIndex::new(inode_index).ok_or(Ext4Error::NotFound)?;
        let inode = Inode::read(self, index)?;
        if inode.metadata.is_dir() {
            return Err(Ext4Error::IsADirectory);
        }
        if !inode.metadata.file_type.is_regular_file() {
            return Err(Ext4Error::IsASpecialFile);
        }
        File::open_inode(self, inode)
    }

    /// Iterate directory entries by inode index. Behaves like
    /// [`Ext4::read_dir`] but skips path resolution. The yielded
    /// `DirEntry::path()` values will be relative to an empty base.
    ///
    /// # Errors
    ///
    /// Returns an error if the inode does not point to a directory.
    pub fn read_dir_inode(&self, inode_index: u32) -> Result<ReadDir, Ext4Error> {
        let index = InodeIndex::new(inode_index).ok_or(Ext4Error::NotFound)?;
        let inode = Inode::read(self, index)?;
        if !inode.metadata.is_dir() {
            return Err(Ext4Error::NotADirectory);
        }
        ReadDir::new(self.clone(), &inode, PathBuf::empty())
    }
}

/// These methods mirror the [`std::fs`][stdfs] API.
///
/// [stdfs]: https://doc.rust-lang.org/std/fs/index.html
impl Ext4 {
    /// Get the canonical, absolute form of a path with all intermediate
    /// components normalized and symbolic links resolved.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn canonicalize<'p, P>(&self, path: P) -> Result<PathBuf, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        let path = path.try_into().map_err(|_| Ext4Error::MalformedPath)?;
        resolve::resolve_path(self, path, FollowSymlinks::All).map(|v| v.1)
    }

    /// Open the file at `path`.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    /// * `path` is a directory or special file type.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn open<'p, P>(&self, path: P) -> Result<File, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        File::open(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Read the entire contents of a file as raw bytes.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    /// * `path` is a directory or special file type.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn read<'p, P>(&self, path: P) -> Result<Vec<u8>, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<Vec<u8>, Ext4Error> {
            let inode = fs.path_to_inode(path, FollowSymlinks::All)?;

            if inode.metadata.is_dir() {
                return Err(Ext4Error::IsADirectory);
            }
            if !inode.metadata.file_type.is_regular_file() {
                return Err(Ext4Error::IsASpecialFile);
            }

            fs.read_inode_file(&inode)
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Read the entire contents of a file as a string.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    /// * `path` is a directory or special file type.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn read_to_string<'p, P>(&self, path: P) -> Result<String, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<String, Ext4Error> {
            let content = fs.read(path)?;
            String::from_utf8(content).map_err(|_| Ext4Error::NotUtf8)
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Get the target of a symbolic link.
    ///
    /// The final component of `path` must be a symlink. If the path
    /// contains any symlinks in components prior to the end, they will
    /// be fully resolved as normal.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * The final component of `path` is not a symlink.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn read_link<'p, P>(&self, path: P) -> Result<PathBuf, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<PathBuf, Ext4Error> {
            let inode = fs.path_to_inode(path, FollowSymlinks::ExcludeFinalComponent)?;
            inode.symlink_target(fs)
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Get an iterator over the entries in a directory.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    /// * `path` is not a directory.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn read_dir<'p, P>(&self, path: P) -> Result<ReadDir, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<ReadDir, Ext4Error> {
            let inode = fs.path_to_inode(path, FollowSymlinks::All)?;

            if !inode.metadata.is_dir() {
                return Err(Ext4Error::NotADirectory);
            }

            ReadDir::new(fs.clone(), &inode, path.into())
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Check if `path` exists.
    ///
    /// Returns `Ok(true)` if `path` exists, or `Ok(false)` if it does
    /// not exist.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn exists<'p, P>(&self, path: P) -> Result<bool, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<bool, Ext4Error> {
            match fs.path_to_inode(path, FollowSymlinks::All) {
                Ok(_) => Ok(true),
                Err(Ext4Error::NotFound) => Ok(false),
                Err(err) => Err(err),
            }
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Get [`Metadata`] for `path`.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn metadata<'p, P>(&self, path: P) -> Result<Metadata, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<Metadata, Ext4Error> {
            let inode = fs.path_to_inode(path, FollowSymlinks::All)?;
            Ok(inode.metadata)
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }

    /// Get [`Metadata`] for `path`.
    ///
    /// If the final component of `path` is a symlink, information about
    /// the symlink itself will be returned, not the symlink's
    /// targets. Any other symlink components of `path` are resolved as
    /// normal.
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    /// * `path` is not absolute.
    /// * `path` does not exist.
    ///
    /// This is not an exhaustive list of errors, see the
    /// [crate documentation](crate#errors).
    pub fn symlink_metadata<'p, P>(&self, path: P) -> Result<Metadata, Ext4Error>
    where
        P: TryInto<Path<'p>>,
    {
        fn inner(fs: &Ext4, path: Path<'_>) -> Result<Metadata, Ext4Error> {
            let inode = fs.path_to_inode(path, FollowSymlinks::ExcludeFinalComponent)?;
            Ok(inode.metadata)
        }

        inner(self, path.try_into().map_err(|_| Ext4Error::MalformedPath)?)
    }
}

impl Debug for Ext4 {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Exclude the reader field, which does not impl Debug. Even if
        // it did, it could be annoying to print out (e.g. if the reader
        // is a Vec it might contain many megabytes of data).
        f.debug_struct("Ext4")
            .field("superblock", &self.0.superblock)
            .field("block_group_descriptors", &self.0.block_group_descriptors)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use super::*;
    use test_util::load_test_disk1;

    #[test]
    fn test_load_errors() {
        // Not enough data.
        assert!(matches!(
            Ext4::load(Box::new(vec![])).unwrap_err(),
            Ext4Error::Io(_)
        ));

        // Invalid superblock.
        assert_eq!(
            Ext4::load(Box::new(vec![0; 2048])).unwrap_err(),
            CorruptKind::SuperblockMagic
        );

        // Not enough data to read the block group descriptors.
        let mut fs_data = vec![0; 2048];
        fs_data[1024..2048].copy_from_slice(include_bytes!("../test_data/raw_superblock.bin"));
        assert!(matches!(
            Ext4::load(Box::new(fs_data.clone())).unwrap_err(),
            Ext4Error::Io(_)
        ));

        // Invalid block group descriptor checksum.
        fs_data.resize(3048usize, 0u8);
        assert_eq!(
            Ext4::load(Box::new(fs_data.clone())).unwrap_err(),
            CorruptKind::BlockGroupDescriptorChecksum(0)
        );
    }

    /// Test that loading the data from
    /// https://github.com/nicholasbishop/ext4-view-rs/issues/280 does not
    /// panic.
    #[test]
    fn test_invalid_ext4_data() {
        // Fill in zeros for the first 1024 bytes, then add the test data.
        let mut data = vec![0; 1024];
        data.extend(include_bytes!("../test_data/not_ext4.bin"));

        assert_eq!(
            Ext4::load(Box::new(data)).unwrap_err(),
            CorruptKind::InvalidBlockSize
        );
    }

    fn block_read_error(
        block_index: FsBlockIndex,
        offset_within_block: u32,
        read_len: usize,
    ) -> CorruptKind {
        CorruptKind::BlockRead {
            block_index,
            original_block_index: block_index,
            offset_within_block,
            read_len,
        }
    }

    /// Test that reading from the first 1024 bytes of the file fails.
    #[test]
    fn test_read_from_block_first_1024() {
        let fs = load_test_disk1();
        let mut dst = vec![0; 1];
        assert_eq!(
            fs.read_from_block(0, 1023, &mut dst).unwrap_err(),
            block_read_error(0, 1023, 1),
        );
    }

    /// Test that reading past the last block of the file fails.
    #[test]
    fn test_read_from_block_past_file_end() {
        let fs = load_test_disk1();
        let mut dst = vec![0; 1024];
        assert_eq!(
            fs.read_from_block(999_999_999, 0, &mut dst).unwrap_err(),
            block_read_error(999_999_999, 0, 1024),
        );
    }

    /// Test that reading at an offset >= the block size fails.
    #[test]
    fn test_read_from_block_invalid_offset() {
        let fs = load_test_disk1();
        let mut dst = vec![0; 1024];
        assert_eq!(
            fs.read_from_block(1, 1024, &mut dst).unwrap_err(),
            block_read_error(1, 1024, 1024),
        );
    }

    /// Test that reading past the end of the block fails.
    #[test]
    fn test_read_from_block_past_block_end() {
        let fs = load_test_disk1();
        let mut dst = vec![0; 25];
        assert_eq!(
            fs.read_from_block(1, 1000, &mut dst).unwrap_err(),
            block_read_error(1, 1000, 25),
        );
    }

    #[test]
    fn test_path_to_inode() {
        let fs = load_test_disk1();

        let follow = FollowSymlinks::All;

        let inode = fs
            .path_to_inode(Path::try_from("/").unwrap(), follow)
            .unwrap();
        assert_eq!(inode.index.get(), 2);

        // Successful lookup.
        assert!(
            fs.path_to_inode(Path::try_from("/empty_file").unwrap(), follow)
                .is_ok()
        );

        // Successful lookup with a "." component.
        assert!(
            fs.path_to_inode(Path::try_from("/./empty_file").unwrap(), follow)
                .is_ok()
        );

        // Successful lookup with a ".." component.
        let inode = fs
            .path_to_inode(Path::try_from("/empty_dir/..").unwrap(), follow)
            .unwrap();
        assert_eq!(inode.index.get(), 2);

        // Successful lookup with symlink.
        assert!(
            fs.path_to_inode(Path::try_from("/sym_simple").unwrap(), follow)
                .is_ok()
        );

        // Error: not an absolute path.
        assert!(
            fs.path_to_inode(Path::try_from("empty_file").unwrap(), follow)
                .is_err()
        );

        // Error: invalid child of a valid directory.
        assert!(
            fs.path_to_inode(Path::try_from("/empty_dir/does_not_exist").unwrap(), follow)
                .is_err()
        );

        // Error: attempted to lookup child of a regular file.
        assert!(
            fs.path_to_inode(
                Path::try_from("/empty_file/does_not_exist").unwrap(),
                follow
            )
            .is_err()
        );

        // TODO: add deeper paths to the test disk and test here.
    }
}
