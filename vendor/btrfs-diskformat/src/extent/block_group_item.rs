use bitflags::bitflags;
use static_assertions::const_assert_eq;
use zerocopy::little_endian::U64 as U64LE;
use zerocopy_derive::*;

/// Defines the location, properties, and usage of a block group.
#[derive(Copy, Clone, Debug, Hash, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct BlockGroupItem {
    /// The space used, in bytes, in this block group.
    pub used: U64LE,

    /// The object ID of the chunk backing this block group.
    pub chunk_objectid: U64LE,

    /// Flags indicating allocation type and replication policy.
    pub flags: U64LE,
}
const_assert_eq!(core::mem::size_of::<BlockGroupItem>(), 24);

bitflags! {
    /// The type of storage a block group allows.
    ///
    /// [Data] and [metadata] chunks may be mixed within a [block group], as indicated by its
    /// [flags]. However, [system] chunks cannot be mixed.
    ///
    /// [Data]: AllocationType::DATA
    /// [metadata]: AllocationType::METADATA
    /// [block group]: BlockGroupItem
    /// [flags]: BlockGroupItem::flags
    /// [SYSTEM]: AllocationType::SYSTEM
    pub struct AllocationType: u64 {
        const DATA = 0x1;
        const SYSTEM = 0x2;
        const METADATA = 0x4;
    }
}

bitflags! {
    /// The replication policy a [block group] implements.
    ///
    /// Only one policy may be set for a given group.
    ///
    /// [block group]: BlockGroupItem
    pub struct ReplicationPolicy: u64 {
        const RAID0 = 0x8;
        const RAID1 = 0x10;
        const DUP = 0x20;
        const RAID10 = 0x40;
        const RAID5 = 0x80;
        const RAID6 = 0x100;
        const RAID1C3 = 0x200;
        const RAID1C4 = 0x400;
    }
}
