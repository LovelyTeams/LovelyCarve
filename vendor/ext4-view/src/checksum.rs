// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use core::fmt::{self, Debug, Formatter};

/// Stateful checksum calculator.
///
/// Ext4 has [metadata checksums][0] for most data structures
/// (superblock, block group descriptor, inode, etc). The checksum
/// algorithm more or less matches [CRC32C][1], but with the bits
/// flipped when finalizing. When initializing with a seed, the seed's
/// bits are reversed.
///
/// [0]: https://www.kernel.org/doc/html/latest/filesystems/ext4/overview.html#checksums
/// [1]: https://reveng.sourceforge.io/crc-catalogue/all.htm#crc.cat.crc-32-iscsi
///
/// # Seed
///
/// The default seed for CRC32C is `0xffff_ffff`. This is used for the
/// superblock checksum. Other structures use a checksum seed stored in
/// the superblock. The checksum seed is derived from the filesystem's
/// initial UUID.
#[derive(Clone)]
pub(crate) struct Checksum {
    digest: crc::Digest<'static, u32>,
}

impl Checksum {
    /// The CRC algorithm, referred to as CRC32C in the kernel.
    const ALGORITHM: crc::Algorithm<u32> = crc::CRC_32_ISCSI;

    /// Create a `Checksum` with the default seed (`0xffff_ffff`).
    pub(crate) fn new() -> Self {
        Self::with_seed(Self::ALGORITHM.init)
    }

    /// Create a `Checksum` with the given `seed`.
    pub(crate) fn with_seed(seed: u32) -> Self {
        const CRC32C: crc::Crc<u32> = crc::Crc::<u32>::new(&Checksum::ALGORITHM);

        Self {
            digest: CRC32C.digest_with_initial(seed.reverse_bits()),
        }
    }

    /// Extend the digest with arbitrary data.
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.digest.update(data);
    }

    /// Extend the digest with a big-endian `u32`.
    pub(crate) fn update_u32_be(&mut self, data: u32) {
        self.update(&data.to_be_bytes());
    }

    /// Extend the digest with a little-endian `u16`.
    pub(crate) fn update_u16_le(&mut self, data: u16) {
        self.update(&data.to_le_bytes());
    }

    /// Extend the digest with a little-endian `u32`.
    pub(crate) fn update_u32_le(&mut self, data: u32) {
        self.update(&data.to_le_bytes());
    }

    /// Get the final value of the checksum.
    ///
    /// This consumes the `Checksum`.
    pub(crate) fn finalize(self) -> u32 {
        self.digest.finalize() ^ (!0)
    }
}

impl Debug for Checksum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Not a particularly informative Debug impl, but allows
        // `Checksum` to be embedded in other structs that derive
        // `Debug`.
        f.debug_struct("Checksum").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum() {
        let mut c = Checksum::new();
        c.update_u32_le(1);
        c.update_u32_le(2);
        assert_eq!(c.finalize(), 0x858c13d3);

        let mut c = Checksum::with_seed(123);
        c.update_u32_le(1);
        c.update_u32_le(2);
        assert_eq!(c.finalize(), 0xfc527a0a);

        assert_eq!(format!("{:?}", Checksum::new()), "Checksum { .. }");
    }
}
