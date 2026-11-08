// SPDX-License-Identifier: (MIT OR Apache-2.0)

mod extra_meta;
mod isodirectory;
mod isofile;
mod symlink;

pub use crate::parse::susp::{PosixAttributes, PosixFileMode, PosixTimestamp, SuspExtension};
pub use extra_meta::{ExtraAttributes, ExtraMeta};
pub use isodirectory::{ISODirectory, ISODirectoryIterator};
pub use isofile::{ISOFile, ISOFileReader};
pub use symlink::Symlink;

use crate::parse::directory_entry::{DirectoryEntryHeader, FileFlags};
use crate::{FileRef, ISO9660Reader, Result};

/// An entry inside of a directory on the filesystem.  Returned by the [`ISODirectoryIterator`] iterator.
///
/// # Notes
///
/// ISO 9660 / ECMA-119 define other common unix types such as sockets, pipes, and devices.
/// Currently only regular files, directories, and symbolic links are supported.
#[derive(Clone)]
pub enum DirectoryEntry<T: ISO9660Reader> {
    /// Directory entry.
    Directory(ISODirectory<T>),

    /// Regular file entry.
    File(ISOFile<T>),

    /// Symbolic link entry.
    Symlink(Symlink),
}

impl<T: ISO9660Reader> std::fmt::Debug for DirectoryEntry<T> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Directory(dir) => write!(fmt, "{dir:?}"),
            Self::File(file) => write!(fmt, "{file:?}"),
            Self::Symlink(link) => write!(fmt, "{link:?}"),
        }
    }
}

impl<T: ISO9660Reader> DirectoryEntry<T> {
    pub(crate) fn new(
        header: DirectoryEntryHeader,
        ext: ExtraMeta,
        identifier: String,
        file: FileRef<T>,
    ) -> Result<Self> {
        let is_dir = header.file_flags.contains(FileFlags::DIRECTORY);
        let is_symlink = match ext.attributes {
            Some(ref attributes) => attributes.mode.contains(PosixFileMode::TYPE_SYMLINK),
            None => false,
        };

        if is_dir {
            Ok(DirectoryEntry::Directory(ISODirectory::new(
                header, ext, identifier, file,
            )))
        } else if is_symlink {
            Ok(DirectoryEntry::Symlink(Symlink::new(
                header, ext, identifier,
            )?))
        } else {
            Ok(DirectoryEntry::File(ISOFile::new(
                header, ext, identifier, file,
            )?))
        }
    }

    /// Returns the name of the current `DirectoryEntry`.
    ///
    /// # Notes
    ///
    /// ISO-9660 / ECMA-119 specify various restrictions on what characters are allowed in an
    /// identifier, and how long they can be.  Joliet specifies UTF-16BE encoding for its
    /// alternative directory hierarchies.  Finally, in Rust [`String`]s use UTF-8.  The way this
    /// works in practice is that we're okay with identifiers that are UTF-8 or a subset thereof,
    /// unless a character encoding is explicitly specified (e.g. Joliet).  If UTF-16 is specified,
    /// it is assumed to be *big endian*.
    pub fn identifier(&self) -> &str {
        match *self {
            DirectoryEntry::Directory(ref dir) => &dir.identifier,
            DirectoryEntry::File(ref file) => &file.identifier,
            DirectoryEntry::Symlink(ref link) => &link.identifier,
        }
    }
}

impl<T: ISO9660Reader> ExtraAttributes for DirectoryEntry<T> {
    fn ext(&self) -> &ExtraMeta {
        match *self {
            DirectoryEntry::Directory(ref dir) => &dir.ext,
            DirectoryEntry::File(ref file) => &file.ext,
            DirectoryEntry::Symlink(ref link) => &link.ext,
        }
    }

    fn header(&self) -> &DirectoryEntryHeader {
        match *self {
            DirectoryEntry::Directory(ref dir) => &dir.header,
            DirectoryEntry::File(ref file) => &file.header,
            DirectoryEntry::Symlink(ref link) => &link.header,
        }
    }
}
