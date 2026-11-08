// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::util::read_u32be;

/// Header at the start of every non-data block in the journal.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct JournalBlockHeader {
    pub(super) block_type: JournalBlockType,
    pub(super) sequence: u32,
}

impl JournalBlockHeader {
    pub(super) const MAGIC: u32 = 0xc03b3998;

    /// Size of the header in bytes.
    pub(super) const SIZE: usize = 12;

    /// Read a `JournalBlockHeader` from raw bytes.
    ///
    /// If the bytes do not start with the expected magic number, return
    /// `None`.
    ///
    /// # Panics
    ///
    /// Panics if the length of `bytes` is less than 12.
    pub(super) fn read_bytes(bytes: &[u8]) -> Option<Self> {
        // Return early if this is not a journal block.
        let h_magic = read_u32be(bytes, 0x0);
        if h_magic != Self::MAGIC {
            return None;
        }

        let h_blocktype = read_u32be(bytes, 0x4);
        let h_sequence = read_u32be(bytes, 0x8);

        Some(Self {
            block_type: JournalBlockType(h_blocktype),
            sequence: h_sequence,
        })
    }
}

/// Journal block type.
///
/// This is represented as a wrapper around a `u32` rather than an
/// `enum` so that unknown values can be treated as unsupported rather
/// than being unrepresentable states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct JournalBlockType(pub(super) u32);

impl JournalBlockType {
    pub(super) const DESCRIPTOR: Self = Self(1);
    pub(super) const COMMIT: Self = Self(2);
    pub(super) const SUPERBLOCK_V2: Self = Self(4);
    pub(super) const REVOCATION: Self = Self(5);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_block_header_missing_magic() {
        let bytes = [0; 12];
        assert!(JournalBlockHeader::read_bytes(&bytes).is_none());
    }

    #[test]
    fn test_journal_block_header_successful_read() {
        let mut bytes = [0; 12];
        // Set magic.
        bytes[..4].copy_from_slice(&JournalBlockHeader::MAGIC.to_be_bytes());
        // Set block type.
        bytes[4..8].copy_from_slice(&123u32.to_be_bytes());
        // Set sequence.
        bytes[8..12].copy_from_slice(&456u32.to_be_bytes());
        assert_eq!(
            JournalBlockHeader::read_bytes(&bytes).unwrap(),
            JournalBlockHeader {
                block_type: JournalBlockType(123),
                sequence: 456,
            }
        );
    }
}
