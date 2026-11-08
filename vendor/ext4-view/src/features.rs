// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// TODO: fill in more docstrings.
#![allow(missing_docs)]

use bitflags::bitflags;

bitflags! {
    /// File system features that affect whether the data can be read.
    ///
    /// For each of these features, the library must know how to handle
    /// its presence or absence in order to safely read the file system,
    /// even in read-only mode.
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct IncompatibleFeatures: u32 {
        const COMPRESSION = 0x1;

        /// Directory entries store the file type.
        const FILE_TYPE_IN_DIR_ENTRY = 0x2;

        /// Filesystem needs recovery.
        const RECOVERY = 0x4;

        /// Filesystem has a separate journal device.
        const SEPARATE_JOURNAL_DEVICE = 0x8;

        const META_BLOCK_GROUPS = 0x10;
        const EXTENTS = 0x40;
        const IS_64BIT = 0x80;
        const MULTIPLE_MOUNT_PROTECTION = 0x100;
        const FLEXIBLE_BLOCK_GROUPS = 0x200;
        const LARGE_EXTENDED_ATTRIBUTES_IN_INODES = 0x400;
        const DATA_IN_DIR_ENTRY = 0x1000;

        /// The superblock contains the checksum seed. If not present,
        /// the checksum seed is calculated from the filesystem UUID
        const CHECKSUM_SEED_IN_SUPERBLOCK = 0x2000;

        const LARGE_DIRECTORIES = 0x4000;
        const DATA_IN_INODE = 0x8000;
        const ENCRYPTED_INODES = 0x1_0000;
        const CASEFOLD = 0x2_0000;
    }

    /// File system features that do not prevent read-only access to the data.
    ///
    /// The presence or absence of these features does not prevent
    /// loading the file system in read-only mode, even if the library
    /// does not know how to handle some features.
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct ReadOnlyCompatibleFeatures: u32 {
        const SPARSE_SUPERBLOCKS = 0x1;
        const LARGE_FILES = 0x2;
        const BTREE_DIR = 0x4;
        const HUGE_FILES = 0x8;
        const GROUP_DESCRIPTOR_CHECKSUMS = 0x10;
        const LARGE_DIRECTORIES = 0x20;
        const LARGE_INODES = 0x40;
        const HAS_SNAPSHOT = 0x80;
        const QUOTA = 0x100;
        const BIG_ALLOC = 0x200;
        const METADATA_CHECKSUMS = 0x400;
        const REPLICA = 0x800;
        const READ_ONLY = 0x1000;
        const PROJECT_QUOTAS = 0x2000;
        const VERITY = 0x8000;
        const ORPHAN_PRESENT = 0x1_0000;
    }
}

bitflags! {
    /// Optional file system features.
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct CompatibleFeatures: u32 {
        const HAS_JOURNAL = 0x4;
    }
}
