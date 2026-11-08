// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error};
use crate::journal::superblock::JournalSuperblock;
use crate::util::read_u32be;

/// Ensure a commit block's checksum is valid.
///
/// The checksum covers the entire block. The checksum field is treated
/// as zero for the checksum calculation.
pub(super) fn validate_commit_block_checksum(
    superblock: &JournalSuperblock,
    block: &[u8],
) -> Result<(), Ext4Error> {
    // The kernel documentation says that fields 0xc and 0xd contain the
    // checksum type and size, but this is not correct. If the
    // superblock features include `CHECKSUM_V3`, the type/size fields
    // are both zero.

    const CHECKSUM_OFFSET: usize = 16;
    const CHECKSUM_SIZE: usize = 4;

    let expected_checksum = read_u32be(block, CHECKSUM_OFFSET);

    let mut checksum = Checksum::new();
    checksum.update(superblock.uuid.as_bytes());
    checksum.update(&block[..CHECKSUM_OFFSET]);
    checksum.update(&[0; CHECKSUM_SIZE]);
    checksum.update(&block[CHECKSUM_OFFSET + CHECKSUM_SIZE..]);

    if checksum.finalize() == expected_checksum {
        Ok(())
    } else {
        Err(CorruptKind::JournalCommitBlockChecksum.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Uuid;

    /// Test success and failure cases of `validate_commit_block_checksum`.
    #[test]
    fn test_validate_commit_block_checksum() {
        let superblock = JournalSuperblock {
            block_size: 1024,
            sequence: 0,
            start_block: 0,
            uuid: Uuid([0; 16]),
        };

        // Valid checksum.
        let mut block = vec![0xab; 1024];
        block[16..20].copy_from_slice(&[0x66, 0x51, 0x5e, 0x6f]);
        assert!(validate_commit_block_checksum(&superblock, &block).is_ok());

        // Change a single byte at the end, verify checksum becomes invalid.
        block[1023] = 1;
        assert_eq!(
            validate_commit_block_checksum(&superblock, &block).unwrap_err(),
            CorruptKind::JournalCommitBlockChecksum
        );
    }
}
