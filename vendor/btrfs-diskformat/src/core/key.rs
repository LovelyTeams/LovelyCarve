use static_assertions::const_assert_eq;
use zerocopy::little_endian::U64 as U64LE;
use zerocopy_derive::*;

/// A key used to describe and locate any item in any tree.
#[derive(Copy, Clone, Debug, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct Key {
    pub objectid: U64LE,
    pub key_type: u8,
    pub offset: U64LE,
}
const_assert_eq!(core::mem::size_of::<Key>(), 17);
