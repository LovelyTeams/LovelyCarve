use crate::UuidBytes;
use static_assertions::const_assert_eq;
use zerocopy::little_endian::U64 as U64LE;
use zerocopy_derive::*;

/// This structure is used to define the backing device storage that compose a
/// [`Chunk`].
///
/// [`Chunk`]: crate::Chunk
#[derive(
    Copy, Clone, Debug, Hash, PartialEq, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable,
)]
#[repr(C, packed)]
pub struct Stripe {
    /// The ID of the device that contains this stripe.
    pub devid: U64LE,

    /// Location of the start of the stripe, in bytes.
    ///
    /// The length is determined by the `stripe_len` field of the associated
    /// `Chunk`.
    pub offset: U64LE,

    /// UUID of the device that contains this stripe.
    ///
    /// This can be used to confirm that the correct device has been retrieved.
    pub dev_uuid: UuidBytes,
}
const_assert_eq!(core::mem::size_of::<Stripe>(), 32);
