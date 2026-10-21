//! Filesystem-volume readers — one submodule per format. Every module
//! exposes the readers `fs_tree` needs (id parsing, source opening,
//! children listing, file read/stat/export). Adding a new filesystem is
//! a new submodule plus the matching `fs_tree::FileSystem` impl.

pub mod ad1;
pub mod apfs;
pub mod btrfs;
pub mod cdfs;
pub mod docker;
pub mod exfat;
pub mod exfat_recover;
pub mod ext4;
pub mod ext4_block_map;
pub mod ext4_journal;
pub mod ext4_raw;
#[cfg(test)]
mod ext4_tests;
pub mod ext4_xattr;
pub mod fat;
pub mod fat_deleted;
pub mod folder;
pub mod hfsplus;
pub mod lvm;
pub mod ntfs;
pub mod ntfs_usn;
pub mod refs;
mod refs_backing;
#[cfg(feature = "refs-dll-oracle")]
pub mod refs_dll_oracle;
mod refs_walk;
pub mod sevenz;
pub mod squashfs;
pub mod tar;
pub mod ubi;
pub mod ufs;
pub mod xfs;
pub mod zfs;
pub mod zip;
