// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::dir_entry::DirEntryName;
use crate::dir_htree::get_dir_entry_via_htree;
use crate::error::Ext4Error;
use crate::inode::{Inode, InodeFlags};
use crate::iters::read_dir::ReadDir;
use crate::path::PathBuf;

/// Search a directory inode for an entry with the given `name`. If
/// found, return the entry's inode, otherwise return a `NotFound`
/// error.
pub(crate) fn get_dir_entry_inode_by_name(
    fs: &Ext4,
    dir_inode: &Inode,
    name: DirEntryName<'_>,
) -> Result<Inode, Ext4Error> {
    assert!(dir_inode.metadata.is_dir());

    if dir_inode.flags.contains(InodeFlags::DIRECTORY_ENCRYPTED) {
        return Err(Ext4Error::Encrypted);
    }

    if dir_inode.flags.contains(InodeFlags::DIRECTORY_HTREE)
        && !dir_inode.flags.contains(InodeFlags::INLINE_DATA)
        && !fs
            .0
            .superblock
            .incompatible_features
            .contains(crate::features::IncompatibleFeatures::CASEFOLD)
    {
        let entry = get_dir_entry_via_htree(fs, dir_inode, name)?;
        return Inode::read(fs, entry.inode);
    }

    // The entry's `path()` method is not called, so the value of the
    // base path does not matter.
    let path = PathBuf::empty();

    for entry in ReadDir::new(fs.clone(), dir_inode, path)? {
        let entry = entry?;
        if entry.file_name() == name {
            return Inode::read(fs, entry.inode);
        }
    }

    Err(Ext4Error::NotFound)
}

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::load_test_disk1;

    #[test]
    fn test_get_dir_entry_inode_by_name() {
        let fs = load_test_disk1();
        let root_inode = fs.read_root_inode().unwrap();

        let lookup = |name| {
            get_dir_entry_inode_by_name(&fs, &root_inode, DirEntryName::try_from(name).unwrap())
        };

        // Check for a few expected entries.
        // '.' always links to self.
        assert_eq!(lookup(".").unwrap().index, root_inode.index);
        // '..' is normally parent, but in the root dir it's just the
        // root dir again.
        assert_eq!(lookup("..").unwrap().index, root_inode.index);
        // Don't check specific values of these since they might change
        // if the test disk is regenerated
        assert!(lookup("empty_file").is_ok());
        assert!(lookup("empty_dir").is_ok());

        // Check for something that does not exist.
        let err = lookup("does_not_exist").unwrap_err();
        assert!(matches!(err, Ext4Error::NotFound));
    }
}
