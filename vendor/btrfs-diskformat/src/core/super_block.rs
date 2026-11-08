use crate::{
    DevItem, RootBackup,
    constants::{CSUM_SIZE, FSID_SIZE, LABEL_SIZE, MAX_SYSTEM_CHUNK_ARRAY_SIZE, NUM_BACKUP_ROOTS},
};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use static_assertions::const_assert_eq;
use strum::EnumIter;
use zerocopy::little_endian::{U32 as U32LE, U64 as U64LE};
use zerocopy_derive::*;

/// The layout of the superblock. A valid superblock must exist for most btrfs implementations to
/// mount the filesystem.
///
/// The primary superblock is located at [`PRIMARY_SUPERBLOCK_ADDR`].
///
/// There are additional copies of the superblock located at [`SUPERBLOCK_ADDRS`], if those addresses
/// are valid, respectively.
///
///
/// [`PRIMARY_SUPERBLOCK_ADDR`]: crate::constants::PRIMARY_SUPERBLOCK_ADDR
/// [`SUPERBLOCK_ADDRS`]: crate::constants::SUPERBLOCK_ADDRS
///
/// # Resources
///
///  * <https://btrfs.wiki.kernel.org/index.php/Data_Structures#btrfs_super_block>
///  * <https://btrfs.wiki.kernel.org/index.php/On-disk_Format#Superblock>
#[derive(Copy, Clone, IntoBytes, TryFromBytes, Unaligned, KnownLayout)]
#[repr(C, packed)]
pub struct SuperBlock {
    /// Checksum of everything past this field.
    pub csum: [u8; CSUM_SIZE],

    /// Filesystem UUID.
    pub fsid: [u8; FSID_SIZE],

    /// The physical address of this block.
    pub bytenr: U64LE,

    /// Flags
    pub flags: U64LE,

    /// The magic must be equal to `"_BHRfS_M"` in ASCII.
    pub magic: U64LE,

    /// The generation of the superblock. In SSD mode, not all superblocks may be updated, so the
    /// latest generation superblock should be used.
    pub generation: U64LE,

    /// The logical address of the root tree's root.
    pub root: U64LE,

    /// The logical address of the chunk tree's root.
    pub chunk_root: U64LE,

    /// The logical address of the log tree's root.
    pub log_root: U64LE,

    /// FIXME: find out what this is!
    pub log_root_transid: U64LE,

    /// FIXME: document this!
    pub total_bytes: U64LE,

    pub bytes_used: U64LE,

    /// The root directory's object ID, which is typically 6.
    pub root_dir_objectid: U64LE,

    /// The number of devices the current filesystem spans.
    pub num_devices: U64LE,

    /// The size of a sector.
    pub sectorsize: U32LE,

    pub nodesize: U32LE,

    /// This is currently unused.
    pub leafsize: U32LE,

    pub stripesize: U32LE,

    /// The size of [`sys_chunk_array`] found in the superblock.
    ///
    /// [`sys_chunk_array`]: SuperBlock::sys_chunk_array
    pub sys_chunk_array_size: U32LE,

    pub chunk_root_generation: U64LE,

    pub compat_flags: U64LE,

    /// Only implementations that support these flags can write to the filesystem.
    pub compat_ro_flags: U64LE,

    /// Only implementations that support these flags can use the filesystem.
    pub incompat_flags: U64LE,

    /// The checksum type.
    ///
    /// This should correspond with a value from [`ChecksumType`].
    ///
    /// [`ChecksumType`]: crate::ChecksumType
    pub csum_type: ChecksumType,

    pub root_level: u8,

    pub chunk_root_level: u8,

    pub log_root_level: u8,

    pub dev_item: DevItem,

    /// The label represented as a null-terminated UTF-8 string. May not contain `'/'` or `'\\'`.
    pub label: [u8; LABEL_SIZE],

    pub cache_generation: U64LE,

    pub uuid_tree_generation: U64LE,

    /// Reserved for extensibility.
    pub _reserved: [U64LE; 30],

    pub sys_chunk_array: [u8; MAX_SYSTEM_CHUNK_ARRAY_SIZE],

    pub super_roots: [RootBackup; NUM_BACKUP_ROOTS],

    pub _unused1: [u8; 565],
}
const_assert_eq!(core::mem::size_of::<SuperBlock>(), 4096);

/// The hashing algorithm used for checksumming.
#[derive(
    Copy,
    Clone,
    Debug,
    Hash,
    PartialEq,
    EnumIter,
    IntoPrimitive,
    TryFromPrimitive,
    TryFromBytes,
    IntoBytes,
    KnownLayout,
)]
#[repr(u16)]
pub enum ChecksumType {
    CRC32C = 0u16.to_le(),
    XXHASH64 = 1u16.to_le(),
    SHA256 = 2u16.to_le(),
    BLAKE2b = 3u16.to_le(),
}
const_assert_eq!(core::mem::size_of::<ChecksumType>(), 2);
