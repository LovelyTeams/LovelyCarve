use static_assertions::const_assert_eq;
use zerocopy::little_endian::{I64 as I64LE, U32 as U32LE};
use zerocopy_derive::*;

/// The layout of timestamps on disk.
#[derive(
    Copy,
    Clone,
    Debug,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    IntoBytes,
    FromBytes,
    Unaligned,
    KnownLayout,
    Immutable,
)]
#[repr(C, packed)]
pub struct Time {
    /// The timestamp using Unix time convention.
    pub timestamp: I64LE,

    /// The number of nanoseconds past the beginning of the second denoted in [timestamp].
    ///
    /// [timestamp]: Time::timestamp
    pub nanoseconds: U32LE,
}
const_assert_eq!(core::mem::size_of::<Time>(), 12);
