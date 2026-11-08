// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::inode::InodeMode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileTypeError;

/// File type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum FileType {
    /// Block device.
    BlockDevice,

    /// Character device.
    CharacterDevice,

    /// Directory.
    Directory,

    /// First-in first-out (FIFO) special file.
    Fifo,

    /// Regular file.
    Regular,

    /// Socket file.
    Socket,

    /// Symbolic link.
    Symlink,
}

impl FileType {
    pub(crate) fn from_dir_entry(val: u8) -> Result<Self, FileTypeError> {
        match val {
            1 => Ok(Self::Regular),
            2 => Ok(Self::Directory),
            3 => Ok(Self::CharacterDevice),
            4 => Ok(Self::BlockDevice),
            5 => Ok(Self::Fifo),
            6 => Ok(Self::Socket),
            7 => Ok(Self::Symlink),
            _ => Err(FileTypeError),
        }
    }

    /// Returns true if the file is a block device.
    #[must_use]
    pub fn is_block_dev(self) -> bool {
        self == Self::BlockDevice
    }

    /// Returns true if the file is a character device.
    #[must_use]
    pub fn is_char_dev(self) -> bool {
        self == Self::CharacterDevice
    }

    /// Returns true if the file is a directory.
    #[must_use]
    pub fn is_dir(self) -> bool {
        self == Self::Directory
    }

    /// Returns true if the file is a first-in first-out (FIFO) special file.
    #[must_use]
    pub fn is_fifo(self) -> bool {
        self == Self::Fifo
    }

    /// Returns true if the file is a regular file.
    #[must_use]
    pub fn is_regular_file(self) -> bool {
        self == Self::Regular
    }

    /// Returns true if the file is a socket.
    #[must_use]
    pub fn is_socket(self) -> bool {
        self == Self::Socket
    }

    /// Returns true if the file is a symlink.
    #[must_use]
    pub fn is_symlink(self) -> bool {
        self == Self::Symlink
    }
}

impl TryFrom<InodeMode> for FileType {
    type Error = FileTypeError;

    fn try_from(mode: InodeMode) -> Result<Self, Self::Error> {
        // Mask out the lower bits.
        let mode = InodeMode::from_bits_retain(mode.bits() & 0xf000);

        if mode == InodeMode::S_IFIFO {
            Ok(Self::Fifo)
        } else if mode == InodeMode::S_IFCHR {
            Ok(Self::CharacterDevice)
        } else if mode == InodeMode::S_IFDIR {
            Ok(Self::Directory)
        } else if mode == InodeMode::S_IFBLK {
            Ok(Self::BlockDevice)
        } else if mode == InodeMode::S_IFREG {
            Ok(Self::Regular)
        } else if mode == InodeMode::S_IFLNK {
            Ok(Self::Symlink)
        } else if mode == InodeMode::S_IFSOCK {
            Ok(Self::Socket)
        } else {
            Err(FileTypeError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type() {
        // Check each valid file type.
        assert!(FileType::try_from(InodeMode::S_IFIFO).unwrap().is_fifo());
        assert!(
            FileType::try_from(InodeMode::S_IFCHR)
                .unwrap()
                .is_char_dev()
        );
        assert!(
            FileType::try_from(InodeMode::S_IFBLK)
                .unwrap()
                .is_block_dev()
        );
        assert!(
            FileType::try_from(InodeMode::S_IFREG)
                .unwrap()
                .is_regular_file()
        );
        assert!(FileType::try_from(InodeMode::S_IFLNK).unwrap().is_symlink());
        assert!(FileType::try_from(InodeMode::S_IFSOCK).unwrap().is_socket());

        // Check that other bits being set in the mode don't impact the
        // file type.
        assert!(
            FileType::try_from(InodeMode::S_IFREG | InodeMode::S_IXOTH)
                .unwrap()
                .is_regular_file()
        );

        // Error, no file type set.
        assert_eq!(
            FileType::try_from(InodeMode::empty()).unwrap_err(),
            FileTypeError
        );
    }
}
