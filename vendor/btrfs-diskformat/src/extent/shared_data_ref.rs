use static_assertions::const_assert_eq;
use zerocopy::little_endian::U32 as U32LE;
use zerocopy_derive::*;

/// This structure contains the reference count for a shared back reference for a file data extent.
///
/// This immediately follows an [`ExtentInlineRefHeader`] of type [`SharedDataRef`] inside an extent
/// item.
///
/// [`ExtentInlineRefHeader`]: crate::ExtentInlineRefHeader
/// [`SharedDataRef`]: crate::ExtentInlineRefType::SharedDataRef
#[derive(Copy, Clone, Debug, Hash, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct SharedDataRef {
    /// The reference count.
    pub count: U32LE,
}
const_assert_eq!(core::mem::size_of::<SharedDataRef>(), 4);
