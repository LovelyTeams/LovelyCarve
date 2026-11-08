// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::util::usize_from_u32;
use core::cmp::Ordering;
use core::fmt::{self, Display, Formatter};
use core::num::NonZero;

/// File system block size.
///
/// The block size is guaranteed to be a multiple of `1024` in the range
/// `1024..=2_147_483_648`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub(crate) struct BlockSize(NonZero<u32>);

impl BlockSize {
    pub(crate) fn from_superblock_value(log_block_size: u32) -> Option<Self> {
        let exp = log_block_size.checked_add(10)?;
        let block_size = 2u32.checked_pow(exp)?;
        // OK to unwrap: the smallest value of `log_block_size` is 0, so
        // the smallest value of `block_size` is `2^(0+10)=1024`.
        Some(Self(NonZero::new(block_size).unwrap()))
    }

    pub(crate) const fn to_u32(self) -> u32 {
        self.0.get()
    }

    pub(crate) const fn to_nz_u32(self) -> NonZero<u32> {
        self.0
    }

    pub(crate) const fn to_u64(self) -> u64 {
        // Cannot use `u64::try_from` in a `const fn`.
        #[expect(clippy::as_conversions)]
        {
            self.0.get() as u64
        }
    }

    pub(crate) const fn to_nz_u64(self) -> NonZero<u64> {
        // OK to unwrap: a `NonZero<u32>` always fits in a `NonZero<u64>`.
        NonZero::new(self.to_u64()).unwrap()
    }

    pub(crate) const fn to_usize(self) -> usize {
        usize_from_u32(self.0.get())
    }
}

impl Display for BlockSize {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl PartialEq<u32> for BlockSize {
    fn eq(&self, v: &u32) -> bool {
        self.to_u32() == *v
    }
}

impl PartialEq<BlockSize> for u16 {
    fn eq(&self, v: &BlockSize) -> bool {
        u32::from(*self) == v.to_u32()
    }
}

impl PartialEq<BlockSize> for u32 {
    fn eq(&self, v: &BlockSize) -> bool {
        *self == v.to_u32()
    }
}

impl PartialEq<BlockSize> for usize {
    fn eq(&self, v: &BlockSize) -> bool {
        *self == v.to_usize()
    }
}

impl PartialOrd<BlockSize> for u16 {
    fn partial_cmp(&self, v: &BlockSize) -> Option<Ordering> {
        u32::from(*self).partial_cmp(&v.to_u32())
    }
}

impl PartialOrd<BlockSize> for u32 {
    fn partial_cmp(&self, v: &BlockSize) -> Option<Ordering> {
        self.partial_cmp(&v.to_u32())
    }
}

impl PartialOrd<BlockSize> for usize {
    fn partial_cmp(&self, v: &BlockSize) -> Option<Ordering> {
        self.partial_cmp(&v.to_usize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_size_display() {
        let bs = BlockSize::from_superblock_value(0).unwrap();
        assert_eq!(format!("{bs}"), "1024");
    }

    #[test]
    fn test_block_size_from_superblock_value() {
        assert_eq!(BlockSize::from_superblock_value(0).unwrap().0.get(), 1024);
        assert_eq!(BlockSize::from_superblock_value(1).unwrap().0.get(), 2048);
        assert_eq!(BlockSize::from_superblock_value(2).unwrap().0.get(), 4096);
        assert_eq!(
            BlockSize::from_superblock_value(21).unwrap().0.get(),
            2_147_483_648
        );
        assert!(BlockSize::from_superblock_value(22).is_none());
    }

    #[test]
    fn test_block_size_to() {
        let bs = BlockSize::from_superblock_value(0).unwrap();
        assert_eq!(bs.0.get(), 1024);
        assert_eq!(bs.to_u32(), 1024u32);
        assert_eq!(bs.to_u64(), 1024u64);
        assert_eq!(bs.to_usize(), 1024usize);
    }

    #[test]
    fn test_block_size_eq() {
        let bs = BlockSize::from_superblock_value(0).unwrap();
        assert!(bs == 1024u32);
        assert!(1024u16 == bs);
        assert!(1024u32 == bs);
        assert!(1024usize == bs);
        assert!(1023u16 < bs);
        assert!(1023u32 < bs);
        assert!(1023usize < bs);
    }
}
