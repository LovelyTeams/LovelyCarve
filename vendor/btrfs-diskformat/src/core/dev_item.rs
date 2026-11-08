use crate::UuidBytes;
use static_assertions::const_assert_eq;
use zerocopy::little_endian::{U32 as U32LE, U64 as U64LE};
use zerocopy_derive::*;

/// Represents a complete block device.
#[derive(Copy, Clone, Debug, Hash, IntoBytes, FromBytes, Unaligned, KnownLayout, Immutable)]
#[repr(C, packed)]
pub struct DevItem {
    /// The internal btrfs device ID.
    ///
    /// This should match the devid found in the filesystem's list of devices.
    pub devid: U64LE,

    /// The size of the device.
    pub total_bytes: U64LE,

    /// The bytes in use by the filesystem on the device.
    pub bytes_used: U64LE,

    /// The optimal I/O alignment for this device.
    pub io_align: U32LE,

    /// The optimal I/O width for this device.
    pub io_width: U32LE,

    /// The minimum I/O size for this device.
    pub sector_size: U32LE,

    /// The type and info for this device.
    pub dev_type: U64LE,

    /// The expected generation for this device.
    pub generation: U64LE,

    /// The starting byte of this partition on the device, to allow for stripe
    /// alignment.
    pub start_offset: U64LE,

    /// Grouping information for allocation decisions.
    pub dev_group: U32LE,

    /// The seek speed of the device on a scale from 0 to 100, where 100 is the
    /// fastest.
    pub seek_speed: u8,

    /// The bandwidth of the device on a scale from 0 to 100, where 100 is the
    /// fastest.
    pub bandwith: u8,

    /// The generated UUID for this device.
    pub uuid: UuidBytes,

    /// The UUID of the filesystem that owns this device.
    pub fsid: UuidBytes,
}
const_assert_eq!(core::mem::size_of::<DevItem>(), 98);
