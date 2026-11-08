use static_assertions::const_assert_eq;
use zerocopy::little_endian::{U16 as U16LE, U64 as U64LE};
use zerocopy_derive::*;

/// References a subvolume filesystem tree root. This is used for both forward and
/// backward root references.
///
/// The name of the tree is stored after the end of the struct.
#[derive(Copy, Clone, Debug, IntoBytes, FromBytes, Unaligned, KnownLayout)]
#[repr(C, packed)]
pub struct RootRef {
    /// The subtree ID.
    pub dirid: U64LE,

    /// The directory sequence number of the subtree entry.
    pub sequence: U64LE,

    /// The length of the subtree name, stored after this field.
    pub name_len: U16LE,
}
const_assert_eq!(core::mem::size_of::<RootRef>(), 18);
