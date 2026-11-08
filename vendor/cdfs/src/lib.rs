// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! # cdfs
//!
//! `cdfs` is a portable, userland implementation of the ISO 9660 / ECMA-119 filesystem typically found on CDs and DVDs.
//!
//! # Usage
//!
//! To open an ISO image:
//! ```rust
//! # use std::fs::File;
//! use cdfs::{DirectoryEntry, ISO9660};
//!
//! let file = File::open("images/test.iso")?;
//! let iso = ISO9660::new(file)?;
//! # Ok::<(), cdfs::ISOError>(())
//! ```
//!
//! To read a file:
//! ```rust
//! # use std::{fs::File, io::Read};
//! # use cdfs::{DirectoryEntry, ISO9660};
//! # let file = File::open("images/test.iso")?;
//! # let iso = ISO9660::new(file)?;
//! let mut contents = Vec::new();
//! if let Some(DirectoryEntry::File(file)) = iso.open("README.md")? {
//!   file.read().read_to_end(&mut contents)?;
//! }
//! # Ok::<(), cdfs::ISOError>(())
//! ```
//!
//! To iterate over items in a directory:
//! ```rust
//! # use std::fs::File;
//! # use cdfs::{DirectoryEntry, ISO9660};
//! # let file = File::open("images/test.iso")?;
//! # let iso = ISO9660::new(file)?;
//! if let Some(DirectoryEntry::Directory(dir)) = iso.open("/tmp")? {
//!   for entry in dir.contents() {
//!     println!("{}", entry?.identifier());
//!   }
//! }
//! # Ok::<(), cdfs::ISOError>(())
//! ```
//!
//! To get information about a file:
//!
//! ```rust
//! # use std::fs::File;
//! # use cdfs::{ISO9660, ExtraAttributes};
//! let file = File::open("images/test.iso")?;
//! let iso = ISO9660::new(file)?;
//! let obj = iso.open("GPL_3_0.TXT")?.expect("GPL_3_0.TXT doesn't exist");
//! println!("Last modified at: {:?}", obj.modify_time());
//! # Ok::<(), cdfs::ISOError>(())
//! ```
//!
//! # See Also
//!
//! The examples.

#![warn(missing_docs)]
#![allow(dead_code, mismatched_lifetime_syntaxes, unused_variables)]

/// [`Result`](std::result::Result) that returns an [`ISOError`].
pub type Result<T> = std::result::Result<T, ISOError>;

mod directory_entry;
mod error;
mod fileref;
mod parse;

use fileref::FileRef;
use parse::volume_descriptor::VolumeDescriptor;

pub use directory_entry::{
    DirectoryEntry, ExtraAttributes, ExtraMeta, ISODirectory, ISODirectoryIterator, ISOFile,
    ISOFileReader, PosixAttributes, PosixFileMode, PosixTimestamp, SuspExtension, Symlink,
};
pub use error::ISOError;
pub use fileref::ISO9660Reader;

/// Struct representing an ISO 9660 / ECMA-119 filesystem.
pub struct ISO9660<T: ISO9660Reader> {
    _file: FileRef<T>,
    root: ISODirectory<T>,
    sup_root: Option<ISODirectory<T>>,
    primary: VolumeDescriptor,
}

/// The size of a filesystem block, currently hardcoded to 2048 although the ISO spec allows for other sizes.
pub const BLOCK_SIZE: u16 = 2048;

/// A `u8` array big enough to hold an entire filesystem block.
pub type BlockBuffer = [u8; BLOCK_SIZE as usize];

/// A quick hack to allow for a constructor even though blocks are defined as a primitive type.
pub trait BlockBufferCtor {
    /// Creae a new, zero initialized buffer large enough to hold a filesystem block.
    fn new() -> Self;
}

impl BlockBufferCtor for BlockBuffer {
    #[inline(always)]
    fn new() -> Self {
        [0; BLOCK_SIZE as usize]
    }
}

macro_rules! primary_prop_str {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        pub fn $name(&self) -> &str {
            if let VolumeDescriptor::Primary(table) = &self.primary {
                &table.$name
            } else {
                unreachable!()
            }
        }
    };
}

impl<T: ISO9660Reader> ISO9660<T> {
    /// Returns a new [`ISO9660`] instance from an [`ISO9660Reader`] instance.  `ISO9660Reader` has
    /// a blanket implementation for all types that implement [`Read`](std::io::Read) and
    /// [`Seek`](std::io::Seek), so this function can be called with e.g. a [`File`](std::fs::File)
    /// or [`Cursor`](std::io::Cursor).
    ///
    /// # Errors
    ///
    /// Upon encountering an error parsing the filesystem image or an I/O error, an error variant
    /// will be returned.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::fs::File;
    /// # use cdfs::ISO9660;
    /// let file = File::open("images/test.iso")?;
    /// let iso = ISO9660::new(file)?;
    /// # Ok::<(), cdfs::ISOError>(())
    /// ```
    pub fn new(mut reader: T) -> Result<ISO9660<T>> {
        let blksize = usize::from(BLOCK_SIZE);

        let mut buf = BlockBuffer::new();

        let mut root = None;
        let mut primary = None;

        let mut sup_root = None;

        // Skip the "system area"
        let mut lba = 16;

        // Read volume descriptors
        loop {
            let count = reader.read_at(&mut buf, lba)?;

            if count != blksize {
                return Err(ISOError::ReadSize(count));
            }

            let descriptor = VolumeDescriptor::parse(&buf)?;
            match &descriptor {
                Some(VolumeDescriptor::Primary(table)) => {
                    if usize::from(table.logical_block_size) != blksize {
                        // This is almost always the case, but technically
                        // not guaranteed by the standard.
                        // TODO: Implement this
                        return Err(ISOError::InvalidFs("Block size not 2048"));
                    }

                    root = Some((
                        table.root_directory_entry.clone(),
                        table.root_directory_entry_identifier.clone(),
                    ));
                    primary = descriptor;
                }
                Some(VolumeDescriptor::Supplementary(table)) => {
                    if usize::from(table.logical_block_size) != blksize {
                        // This is almost always the case, but technically
                        // not guaranteed by the standard.
                        // TODO: Implement this
                        return Err(ISOError::InvalidFs("Block size not 2048"));
                    }

                    sup_root = Some((
                        table.root_directory_entry.clone(),
                        table.root_directory_entry_identifier.clone(),
                    ));
                }
                Some(VolumeDescriptor::VolumeDescriptorSetTerminator) => break,
                _ => {}
            }

            lba += 1;
        }

        let file = FileRef::new(reader);
        let file2 = file.clone();
        let file3 = file.clone();

        let (root, primary) = match (root, primary) {
            (Some(root), Some(primary)) => (root, primary),
            _ => {
                return Err(ISOError::InvalidFs("No primary volume descriptor"));
            }
        };

        Ok(ISO9660 {
            _file: file,
            root: ISODirectory::new(root.0, ExtraMeta::default(), root.1, file2),
            sup_root: sup_root.map(|sup_root| {
                ISODirectory::new(sup_root.0, ExtraMeta::default(), sup_root.1, file3)
            }),
            primary,
        })
    }

    /// Returns a [`DirectoryEntry`] for a given path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the object on the filesystem
    ///
    /// # Errors
    ///
    /// Upon encountering an I/O error or an error parsing the filesystem, an error variant is returned.
    /// If the path cannot be found on the filesystem `Ok(None)` is returned.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::fs::File;
    /// # use cdfs::ISO9660;
    /// # let file = File::open("images/test.iso")?;
    /// # let iso = ISO9660::new(file)?;
    /// let entry = iso.open("/README.TXT")?;
    /// # Ok::<(), cdfs::ISOError>(())
    /// ```
    pub fn open(&self, path: &str) -> Result<Option<DirectoryEntry<T>>> {
        self.root().find_recursive(path)
    }

    /// Returns true if Rock Ridge extensions are present
    pub fn is_rr(&self) -> bool {
        match self.root.contents().next() {
            Some(Ok(DirectoryEntry::Directory(dirent))) => dirent.is_rock_ridge(),
            _ => false, // again…
        }
    }

    /// Returns the most featureful root directory.
    ///
    /// # Root selection
    /// * If the primary volume descriptor has Rock Ridge SUSP entries, use it
    /// * ElseIf a supplementary volume descriptor (e.g. Joliet) exists, use it
    /// * Else fall back on the primary volume descriptor with short filenames
    ///
    /// # See Also
    /// ISO-9660 / ECMA-119 §§ 8.4, 8.5
    pub fn root(&self) -> &ISODirectory<T> {
        if self.is_rr() {
            &self.root
        } else {
            match self.sup_root.as_ref() {
                Some(sup_root) => sup_root,
                None => &self.root,
            }
        }
    }

    /// Returns the root directory entry.
    ///
    ///
    /// # Arguments
    ///
    /// * `index` - An integer indicating which root entry to return
    ///   * 0 = primary
    ///   * 1 = secondary (if not present, `None` is returned)
    ///
    /// # See Also
    /// ISO-9660 / ECMA-119 §§ 8.4, 8.5
    pub fn root_at(&self, index: usize) -> Option<&ISODirectory<T>> {
        match index {
            0 => Some(&self.root),
            1 => self.sup_root.as_ref(),
            _ => unimplemented!(),
        }
    }

    /// Returns [`BLOCK_SIZE`].
    ///
    /// This implementation hardcodes the block size to 2048.
    ///
    /// # See Also
    /// ISO-9660 / ECMA-119 § 6.1.2
    pub fn block_size(&self) -> u16 {
        BLOCK_SIZE
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.13
        volume_set_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.14
        publisher_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.15
        data_preparer_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.16
        application_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.17
        copyright_file_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.18
        abstract_file_identifier
    }

    primary_prop_str! {
        /// # See Also
        /// ISO-9660 / ECMA-119 § 8.5.19
        bibliographic_file_identifier
    }
}
