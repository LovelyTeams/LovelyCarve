use static_assertions::const_assert_eq;
use zerocopy::little_endian::{U32 as U32LE, U64 as U64LE};
use zerocopy_derive::*;

/// Contains an indirect back reference for a file data extent.
///
/// Immediately follows an [`ExtentInlineRef`]. See that documentation for more details.
///
/// [`ExtentInlineRef`]: crate::ExtentInlineRef
#[derive(Copy, Clone, Debug, Hash, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentDataRef {
    /// The object ID for the file tree that references this extent.
    pub root: U64LE,

    /// The object ID of the inode that contains the extent data that references this extent.
    pub objectid: U64LE,

    /// The offset within the file that corresponds to this extent.
    pub offset: U64LE,

    /// The reference count being held.
    pub count: U32LE,
}
const_assert_eq!(core::mem::size_of::<ExtentDataRef>(), 28);
