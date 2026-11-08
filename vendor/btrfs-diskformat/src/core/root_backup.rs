use static_assertions::const_assert_eq;
use zerocopy::little_endian::U64 as U64LE;
use zerocopy_derive::*;

#[derive(Copy, Clone, Debug, FromBytes, IntoBytes, Unaligned, KnownLayout)]
#[repr(C, packed)]
pub struct RootBackup {
    pub tree_root: U64LE,
    pub tree_root_gen: U64LE,

    pub chunk_root: U64LE,
    pub chunk_root_gen: U64LE,

    pub extent_root: U64LE,
    pub extent_root_gen: U64LE,

    pub fs_root: U64LE,
    pub fs_root_gen: U64LE,

    pub dev_root: U64LE,
    pub dev_root_gen: U64LE,

    pub csum_root: U64LE,
    pub csum_root_gen: U64LE,

    pub total_bytes: U64LE,
    pub bytes_used: U64LE,

    pub num_devices: U64LE,

    /// Reserved for future use.
    pub _unused_u64s: [u64; 4],

    pub tree_root_level: u8,

    pub chunk_root_level: u8,

    pub extent_root_level: u8,

    pub fs_root_level: u8,

    pub dev_root_level: u8,

    pub csum_root_level: u8,

    /// Reserved for future use.
    pub _unused_u8s: [u8; 10],
}
const_assert_eq!(core::mem::size_of::<RootBackup>(), 168);
