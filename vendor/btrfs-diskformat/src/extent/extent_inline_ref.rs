use crate::{ExtentDataRef, SharedDataRef};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use static_assertions::{const_assert, const_assert_eq};
use strum::EnumIter;
use zerocopy::little_endian::U64 as U64LE;
use zerocopy_derive::*;

/// This acts as a header for different types of inline extent back references inside extent or
/// metadata items.
///
/// For the full version that contains a union of all possible inline extent references, see
/// [`ExtentInlineRefFull`]. That type may be more convenient if you are reading the entire
/// reference from disk at once to save trips to disk.
///
/// [`ExtentInlineRefFull`]: crate::ExtentInlineRefFull
#[derive(Copy, Clone, Debug, Hash, TryFromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineRefHeader {
    /// The type of reference, which corresponds with a value from [`ExtentInlineRefType`].
    /// This field also determines the semantic importance of [`offset`].
    ///
    /// [`ExtentInlineRefType`]: crate::ExtentInlineRefType
    /// [`offset`]: ExtentInlineRef::offset
    pub ref_type: ExtentInlineRefType,

    /// This field has different functions depending on the value of [`ref_type`].
    ///
    /// [`ref_type`]: ExtentInlineRef::ref_type
    pub offset: U64LE,
}
const_assert_eq!(core::mem::size_of::<ExtentInlineRefHeader>(), 9);

/// This type contains a union of all possible inline extent references. Using this can be helpful
/// when an entire inline reference is read from disk to save trips to the disk but the type of
/// the reference is not known in advance.
///
/// For the header-only version, see [`ExtentInlineRefHeader`].
#[derive(Copy, Clone, TryFromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineRefFull {
    /// This should be equal to [`ExtentInlineRefType::ExtentDataRef`].
    pub ref_type: ExtentInlineRefType,

    /// This field has different functions depending on the value of [`ref_type`].
    ///
    /// [`ref_type`]: ExtentInlineRef::ref_type
    pub tail: ExtentInlineRefTail,
}
const_assert_eq!(core::mem::size_of::<ExtentInlineRefFull>(), 29);

impl ExtentInlineRefFull {
    /// Returns the value of the offset field, if it is valid for the given [`ref_type`].
    ///
    /// [`ref_type`]: Self::ref_type
    pub fn offset(&self) -> Option<u64> {
        match self.ref_type {
            ExtentInlineRefType::TreeBlockRef | ExtentInlineRefType::SharedBlockRef => {
                Some(unsafe { self.tail.offset.get() })
            }
            ExtentInlineRefType::SharedDataRef => {
                Some(unsafe { self.tail.shared_data_tail.offset.get() })
            }
            _ => None,
        }
    }

    /// Returns the extent data reference if the reference type is
    /// [`ExtentInlineRefType::ExtentDataRef`].
    pub fn extent_data_ref(&self) -> Option<ExtentDataRef> {
        if self.ref_type == ExtentInlineRefType::ExtentDataRef {
            Some(unsafe { self.tail.extent_data_ref })
        } else {
            None
        }
    }

    /// Returns the shared data tail if the reference type is
    /// [`ExtentInlineRefType::SharedDataRef`].
    pub fn shared_data_tail(&self) -> Option<ExtentInlineRefSharedDataTail> {
        if self.ref_type == ExtentInlineRefType::SharedDataRef {
            Some(unsafe { self.tail.shared_data_tail })
        } else {
            None
        }
    }

    pub fn as_tree_block_ref(&self) -> Option<&ExtentInlineTreeBlockRef> {
        const_assert!(
            core::mem::size_of::<ExtentInlineRefFull>()
                >= core::mem::size_of::<ExtentInlineTreeBlockRef>()
        );

        if self.ref_type == ExtentInlineRefType::TreeBlockRef {
            // Safety: We know that the bytes are valid for `ExtentInlineTreeBlockRef` because
            // `ref_type` is `ExtentInlineRefType::TreeBlockRef`.
            Some(unsafe {
                &*(self as *const ExtentInlineRefFull as *const ExtentInlineTreeBlockRef)
            })
        } else {
            None
        }
    }

    pub fn as_shared_block_ref(&self) -> Option<&ExtentInlineSharedBlockRef> {
        const_assert!(
            core::mem::size_of::<ExtentInlineRefFull>()
                >= core::mem::size_of::<ExtentInlineSharedBlockRef>()
        );

        if self.ref_type == ExtentInlineRefType::SharedBlockRef {
            // Safety: We know that the bytes are valid for `ExtentInlineSharedBlockRef` because
            // `ref_type` is `ExtentInlineRefType::SharedBlockRef`.
            Some(unsafe {
                &*(self as *const ExtentInlineRefFull as *const ExtentInlineSharedBlockRef)
            })
        } else {
            None
        }
    }

    pub fn as_extent_data_ref(&self) -> Option<&ExtentInlineExtentDataRef> {
        const_assert!(
            core::mem::size_of::<ExtentInlineRefFull>()
                >= core::mem::size_of::<ExtentInlineExtentDataRef>()
        );

        if self.ref_type == ExtentInlineRefType::ExtentDataRef {
            // Safety: We know that the bytes are valid for `ExtentInlineExtentDataRef` because
            // `ref_type` is `ExtentInlineRefType::ExtentDataRef`.
            Some(unsafe {
                &*(self as *const ExtentInlineRefFull as *const ExtentInlineExtentDataRef)
            })
        } else {
            None
        }
    }

    pub fn as_shared_data_ref(&self) -> Option<&ExtentInlineSharedDataRef> {
        const_assert!(
            core::mem::size_of::<ExtentInlineRefFull>()
                >= core::mem::size_of::<ExtentInlineSharedDataRef>()
        );

        if self.ref_type == ExtentInlineRefType::SharedDataRef {
            // Safety: We know that the bytes are valid for `ExtentInlineSharedDataRef` because
            // `ref_type` is `ExtentInlineRefType::SharedDataRef`.
            Some(unsafe {
                &*(self as *const ExtentInlineRefFull as *const ExtentInlineSharedDataRef)
            })
        } else {
            None
        }
    }
}

/// Union that contains the actual data for the inline extent reference.
#[derive(Copy, Clone, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub union ExtentInlineRefTail {
    pub offset: U64LE,
    pub extent_data_ref: ExtentDataRef,
    pub shared_data_tail: ExtentInlineRefSharedDataTail,
}

/// A variant of [`ExtentInlineRefTail`] for [`ExtentInlineRefType::SharedDataRef`].
#[derive(Copy, Clone, Debug, Hash, FromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineRefSharedDataTail {
    /// The byte offset of the metadata that contains the extent data item that describes this
    /// extent.
    pub offset: U64LE,

    /// The shared data reference count.
    pub shared_data_ref: SharedDataRef,
}

/// An [`ExtentInlineRefHeader`] and/or [`ExtentInlineRefFull`] where the value is known to be
/// [`ExtentInlineRefType::TreeBlockRef`].
#[derive(Copy, Clone, Debug, Hash, TryFromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineTreeBlockRef {
    /// This is here to allow [`TryFromBytes`] to enforce the type of the reference.
    ref_type: ExtentInlineTreeBlockRefType,

    /// The object ID of the tree root that allocated the block.
    pub offset: U64LE,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, TryFromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(u8)]
enum ExtentInlineTreeBlockRefType {
    #[allow(dead_code)]
    TreeBlockRef = ExtentInlineRefType::TreeBlockRef as u8,
}
const_assert_eq!(
    core::mem::size_of::<ExtentInlineTreeBlockRefType>(),
    core::mem::size_of::<ExtentInlineRefType>()
);

/// An [`ExtentInlineRefHeader`] and/or [`ExtentInlineRefFull`] where the value is known to be
/// [`ExtentInlineRefType::SharedBlockRef`].
#[derive(Copy, Clone, Debug, Hash, TryFromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineSharedBlockRef {
    /// This is here to allow [`TryFromBytes`] to enforce the type of the reference.
    ref_type: ExtentInlineSharedBlockRefType,

    /// The byte offset of the node one level above in the tree where this block is located.
    pub offset: U64LE,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, TryFromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(u8)]
#[allow(dead_code)]
enum ExtentInlineSharedBlockRefType {
    SharedBlockRef = ExtentInlineRefType::SharedBlockRef as u8,
}

/// An [`ExtentInlineRefHeader`] and/or [`ExtentInlineRefFull`] where the value is known to be
/// [`ExtentInlineRefType::ExtentDataRef`].
#[derive(Copy, Clone, Debug, Hash, TryFromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineExtentDataRef {
    /// This is here to allow [`TryFromBytes`] to enforce the type of the reference.
    ref_type: ExtentInlineExtentDataRefType,

    /// The extent data reference that overlaps the unused offset field.
    pub extent_data_ref: ExtentDataRef,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, TryFromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(u8)]
enum ExtentInlineExtentDataRefType {
    #[allow(dead_code)]
    ExtentDataRef = ExtentInlineRefType::ExtentDataRef as u8,
}
const_assert_eq!(
    core::mem::size_of::<ExtentInlineExtentDataRefType>(),
    core::mem::size_of::<ExtentInlineRefType>()
);

/// An [`ExtentInlineRefHeader`] and/or [`ExtentInlineRefFull`] where the value is known to be
/// [`ExtentInlineRefType::SharedDataRef`].
#[derive(Copy, Clone, Debug, Hash, TryFromBytes, IntoBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ExtentInlineSharedDataRef {
    /// This is here to allow [`TryFromBytes`] to enforce the type of the reference.
    ref_type: ExtentInlineSharedDataRefType,

    /// The tail of a shared data reference that contains the byte offset of the metadata that
    /// contains the extent data item that describes this extent and the shared data reference
    /// count.
    pub shared_data_tail: ExtentInlineRefSharedDataTail,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, TryFromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(u8)]
enum ExtentInlineSharedDataRefType {
    #[allow(dead_code)]
    SharedDataRef = ExtentInlineRefType::SharedDataRef as u8,
}
const_assert_eq!(
    core::mem::size_of::<ExtentInlineSharedDataRefType>(),
    core::mem::size_of::<ExtentInlineRefType>()
);

/// The type of [`ExtentInlineRefHeader`] or [`ExtentInlineRefFull`].
///
/// [`ExtentInlineRefHeader`]: crate::ExtentInlineRefHeader
/// [`ExtentInlineRefFull`]: crate::ExtentInlineRefFull
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
    Immutable,
)]
#[repr(u8)]
pub enum ExtentInlineRefType {
    /// The reference is indirect for a tree block.
    ///
    /// [`offset`] contains the object ID of the tree root that allocated the block.
    ///
    /// [`offset`]: ExtentInlineRefHeader::offset
    TreeBlockRef = 176,

    /// The reference is shared for a tree block.
    ///
    /// [`offset`] contains the byte offset of the node one level above in the tree where this block
    /// is located.
    ///
    /// [`offset`]: ExtentInlineRefHeader::offset
    SharedBlockRef = 182,

    /// The reference is indirect for a data extent.
    ///
    /// An [`ExtentDataRef`] is located immediately after the [`type`] field and overlaps the unused
    /// [`offset`] field.
    ///
    /// [`type`]: ExtentInlineRef::type
    /// [`offset`]: ExtentInlineRefHeader::offset
    ExtentDataRef = 178,

    /// The reference is shared for a data extent.
    ///
    /// [`offset`] contains the byte offset of the metadata that contains the extent data item that
    /// describes this extent.
    ///
    /// Immediately following [`offset`] (and the end of [`ExtentInlineRefHeader`] structure) is a
    /// [`SharedDataRef`] that contains the reference count.
    ///
    /// [`offset`]: ExtentInlineRefHeader::offset
    SharedDataRef = 184,
}
