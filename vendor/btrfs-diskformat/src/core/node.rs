use crate::{Key, UuidBytes, constants::CSUM_SIZE};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use static_assertions::const_assert_eq;
use strum::EnumIter;
use zerocopy::little_endian::{U32 as U32LE, U64 as U64LE};
use zerocopy_derive::*;

/// The data stored at the start of every node.
#[derive(Clone, Debug, IntoBytes, TryFromBytes, Unaligned, KnownLayout)]
#[repr(C, packed)]
pub struct Header {
    /// The checksum of everything after this field, including the
    /// internal/leaf node specific part.
    pub csum: [u8; CSUM_SIZE],

    /// The filesystem UUID.
    pub fs_uuid: UuidBytes,

    /// The logical address of this node.
    pub logical_address: U64LE,

    /// The first 7 bits represent flags.
    pub flags: [u8; 7],

    /// The backref revision, which maps to a [`BackrefRevision`] value.
    pub backref_rev: BackrefRevision,

    /// The chunk tree UUID.
    pub chunk_tree_uuid: UuidBytes,

    /// The generation of this node.
    pub generation: U64LE,

    /// The ID of the tree containing this node.
    pub tree_id: U64LE,

    /// The number of items held in this node.
    pub num_items: U32LE,

    /// The level of this node. 0 indicates it is a leaf node.
    pub level: u8,
}
const_assert_eq!(core::mem::size_of::<Header>(), 101);

/// For internal (non-leaf) nodes, the [node header] is followed by a dynamic amount of key
/// pointers.
///
/// [node header]: Header
#[derive(Copy, Clone, Debug, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct KeyPointer {
    pub key: Key,
    pub block_pointer: U64LE,
    pub generation: U64LE,
}
const_assert_eq!(core::mem::size_of::<KeyPointer>(), 33);

/// For leaf nodes, the [node header] is followed by a dynamic number of items.
///
/// The item data is stored at the end of the node, as pointed to by the [offset] and [size].
/// The contents of the item are specified in the [key].
///
/// [node header]: Header
/// [offset]: Item::offset
/// [size]: Item::size
/// [key]: Item::key
#[derive(Copy, Clone, Debug, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct Item {
    /// The key that contains the ID and contents of this [Item].
    pub key: Key,

    /// Offset relative to the end of the header.
    pub offset: U32LE,

    /// The size of the data.
    pub size: U32LE,
}
const_assert_eq!(core::mem::size_of::<Item>(), 25);

#[derive(
    Copy,
    Clone,
    Debug,
    Hash,
    PartialEq,
    EnumIter,
    IntoPrimitive,
    TryFromPrimitive,
    IntoBytes,
    TryFromBytes,
    Unaligned,
    KnownLayout,
)]
#[repr(u8)]
pub enum BackrefRevision {
    /// Indicates asn old filesystem.
    Old = 0,

    /// Indicates a new filesystem.
    Mixed = 1,
}
const_assert_eq!(core::mem::size_of::<BackrefRevision>(), 1);
