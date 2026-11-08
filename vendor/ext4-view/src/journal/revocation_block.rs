// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::block_index::FsBlockIndex;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::journal::JournalSuperblock;
use crate::journal::block_header::JournalBlockHeader;
use crate::util::read_u32be;
use crate::util::usize_from_u32;
use alloc::vec::Vec;

/// Ensure a revocation block's checksum is valid.
///
/// The checksum is stored in the last four bytes of the block.
pub(super) fn validate_revocation_block_checksum(
    superblock: &JournalSuperblock,
    block: &[u8],
) -> Result<(), Ext4Error> {
    // OK to unwrap: minimum block length is 1024.
    let checksum_offset = block.len().checked_sub(4).unwrap();
    let expected_checksum = read_u32be(block, checksum_offset);
    let mut checksum = Checksum::new();
    checksum.update(superblock.uuid.as_bytes());
    checksum.update(&block[..checksum_offset]);
    checksum.update_u32_be(0);

    if checksum.finalize() == expected_checksum {
        Ok(())
    } else {
        Err(CorruptKind::JournalRevocationBlockChecksum.into())
    }
}

/// Read the revoked block indices from a revocation block.
///
/// The entries are appended to the end of `table`.
pub(super) fn read_revocation_block_table(
    block: &[u8],
    table: &mut Vec<FsBlockIndex>,
) -> Result<(), Ext4Error> {
    // Note: if this library adds support for 32-bit journals, this
    // size will need to be conditionally set to either 4 or 8.
    const BLOCK_INDEX_SIZE_IN_BYTES: usize = 8;

    // Skip past the block header bytes, and remove the trailing
    // checksum bytes.
    let data = &block[JournalBlockHeader::SIZE..
               // OK to unwrap: minimum block length is 1024.
               block.len().checked_sub(4).unwrap()];

    // Get the size (in bytes) of the block-index array.
    let num_bytes = usize_from_u32(read_u32be(data, 0));

    // Ensure that the table size is an even multiple of the index size.
    if num_bytes % BLOCK_INDEX_SIZE_IN_BYTES != 0 {
        return Err(CorruptKind::JournalRevocationBlockInvalidTableSize(num_bytes).into());
    }

    // Skip past the size field.
    let data = &data[size_of::<u32>()..];

    // Ensure that the table size fits within the block.
    let mut data =
        data.get(..num_bytes)
            .ok_or(CorruptKind::JournalRevocationBlockInvalidTableSize(
                num_bytes,
            ))?;

    // Read each entry and append to `table`.
    while !data.is_empty() {
        let block_index = u64::from_be_bytes(
            // OK to unwrap: `BLOCK_INDEX_SIZE_IN_BYTES` matches the
            // size of `u64`.
            data[..BLOCK_INDEX_SIZE_IN_BYTES].try_into().unwrap(),
        );

        table.push(block_index);

        data = &data[BLOCK_INDEX_SIZE_IN_BYTES..];
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid::Uuid;

    /// Test success and failure cases of `validate_revocation_block_checksum`.
    #[test]
    fn test_validate_revocation_block_checksum() {
        let superblock = JournalSuperblock {
            block_size: 1024,
            sequence: 0,
            start_block: 0,
            uuid: Uuid([0; 16]),
        };
        let mut block = vec![0; 1024];
        assert_eq!(
            validate_revocation_block_checksum(&superblock, &block).unwrap_err(),
            CorruptKind::JournalRevocationBlockChecksum
        );

        block[1020..].copy_from_slice(&[0x74, 0xef, 0x0e, 0xf6]);
        assert!(validate_revocation_block_checksum(&superblock, &block).is_ok());
    }

    fn create_test_revocation_block() -> Vec<u8> {
        let mut block = Vec::new();

        // Add header data (all zeros since only the length matters for this test).
        block.extend([0; JournalBlockHeader::SIZE]);

        // Add size field (three 8-byte entries).
        block.extend(24u32.to_be_bytes());

        // Add three entries.
        block.extend(100u64.to_be_bytes());
        block.extend(101u64.to_be_bytes());
        block.extend(102u64.to_be_bytes());

        // Add another entry that isn't used because of the size.
        block.extend(103u64.to_be_bytes());

        // Pad out to a full block size.
        block.resize(1024usize, 0u8);

        block
    }

    /// Test a successful call to `read_revocation_block_table`.
    #[test]
    fn test_read_revocation_block_table_success() {
        let block = create_test_revocation_block();
        let mut table = Vec::new();
        read_revocation_block_table(&block, &mut table).unwrap();
        assert_eq!(table, [100, 101, 102]);
    }

    /// Test that `read_revocation_block_table` rejects a table size
    /// that is not an even multiple of the table entry size.
    #[test]
    fn test_read_revocation_block_table_uneven_size() {
        let mut block = create_test_revocation_block();
        block[JournalBlockHeader::SIZE..JournalBlockHeader::SIZE + size_of::<u32>()]
            .copy_from_slice(&7u32.to_be_bytes());
        let mut table = Vec::new();
        assert_eq!(
            read_revocation_block_table(&block, &mut table).unwrap_err(),
            CorruptKind::JournalRevocationBlockInvalidTableSize(7)
        );
    }

    /// Test that `read_revocation_block_table` rejects a table size
    /// that is bigger than the available space in the block.
    #[test]
    fn test_read_revocation_block_table_size_too_large() {
        let mut block = create_test_revocation_block();
        block[JournalBlockHeader::SIZE..JournalBlockHeader::SIZE + size_of::<u32>()]
            .copy_from_slice(&1008u32.to_be_bytes());
        let mut table = Vec::new();
        assert_eq!(
            read_revocation_block_table(&block, &mut table).unwrap_err(),
            CorruptKind::JournalRevocationBlockInvalidTableSize(1008)
        );
    }
}
