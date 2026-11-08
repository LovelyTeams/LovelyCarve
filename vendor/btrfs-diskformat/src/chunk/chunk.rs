use crate::Stripe;
use static_assertions::const_assert_eq;
use zerocopy::{
    CastError, FromBytes as _,
    little_endian::{U16 as U16LE, U32 as U32LE, U64 as U64LE},
};
use zerocopy_derive::*;

/// This structure contains the mapping from a virtualized usable byte range within the backing
/// storage to a set of one or more stripes on individual backing devices. In addition to the
/// mapping, hints on optimal I/O parameters for this chunk. It is associated with `CHUNK_ITEM`.
///
/// Although the structure definition only contains one stripe member, `CHUNK_ITEM` contain as
/// many struct [`Stripe`] structures as specified in the [`num_stripes`] and [`sub_stripes`]
/// fields.
///
/// For the dynamically-sized version of this type with the stripes following, see [`ChunkDynamic`].
///
/// [`Stripe`]: crate::Stripe
/// [`num_stripes`]: Chunk::num_stripes
/// [`sub_stripes`]: Chunk::sub_stripes
#[derive(Copy, Clone, Debug, Hash, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct Chunk {
    /// The size of this chunk, in bytes.
    pub length: U64LE,

    /// The object ID of the root referencing this chunk. This is always the ID of an extent root.
    pub owner: U64LE,

    /// The replication stripe length.
    pub stripe_len: U64LE,

    /// Flags indicating allocation type and replication policy.
    pub chunk_type: U64LE,

    /// The optimal I/O alignment for this chunk.
    pub io_align: U32LE,

    /// The optimal I/O width for this chunk.
    pub io_width: U32LE,

    /// The minimal I/O size for this chunk.
    pub sector_size: U32LE,

    /// The number of replication stripes.
    pub num_stripes: U16LE,

    /// The number of sub-stripes. This is only used for RAID-10.
    ///
    /// This is 2 for RAID-10, and 1 for all other chunk types.
    pub sub_stripes: U16LE,
}
const_assert_eq!(core::mem::size_of::<Chunk>(), 48);

impl Chunk {
    /// A convenience method for converting a [`Chunk`] into a [`ChunkDynamic`].
    ///
    /// For a safe version, use [`ChunkDynamic::ref_from_prefix_with_elems`].
    ///
    /// # Safety
    /// This function is unsafe because it assumes that the bytes following the `Chunk` structure
    /// are valid and contain the expected number of `Stripe` structures as specified by
    /// `num_stripes`. If this assumption is violated, it may lead to undefined behavior.
    pub unsafe fn into_dynamic(
        &self,
        num_stripes: usize,
    ) -> Result<&ChunkDynamic, CastError<&[u8], ChunkDynamic>> {
        // This is simple arithmetic, since this is a packed structure.
        let expected_size =
            core::mem::size_of::<Chunk>() + num_stripes * core::mem::size_of::<Stripe>();

        // Safety: We assume the bytes for `num_stripes` after the `Chunk` structure are valid, as
        // part of the contract of this function.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Chunk as *const u8, expected_size)
        };

        ChunkDynamic::ref_from_prefix_with_elems(bytes, num_stripes).map(|(chunk, _)| chunk)
    }
}

/// This structure contains the mapping from a virtualized usable byte range within the backing
/// storage to a set of one or more stripes on individual backing devices. In addition to the
/// mapping, hints on optimal I/O parameters for this chunk. It is associated with `CHUNK_ITEM`.
///
/// For the constant-sized version of the type, see [`Chunk`].`
#[derive(IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct ChunkDynamic {
    /// The size of this chunk, in bytes.
    pub length: U64LE,

    /// The object ID of the root referencing this chunk. This is always the ID of an extent root.
    pub owner: U64LE,

    /// The replication stripe length.
    pub stripe_len: U64LE,

    /// Flags indicating allocation type and replication policy.
    pub chunk_type: U64LE,

    /// The optimal I/O alignment for this chunk.
    pub io_align: U32LE,

    /// The optimal I/O width for this chunk.
    pub io_width: U32LE,

    /// The minimal I/O size for this chunk.
    pub sector_size: U32LE,

    /// The number of replication stripes.
    pub num_stripes: U16LE,

    /// The number of sub-stripes. This is only used for RAID-10.
    pub sub_stripes: U16LE,

    /// The first of one or more stripes that map to device extents.
    pub stripe: [Stripe],
}
