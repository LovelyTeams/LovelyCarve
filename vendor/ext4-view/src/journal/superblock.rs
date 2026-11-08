// Copyright 2025 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::Ext4;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error, IncompatibleKind};
use crate::inode::Inode;
use crate::iters::file_blocks::FileBlocks;
use crate::journal::block_header::{JournalBlockHeader, JournalBlockType};
use crate::util::read_u32be;
use crate::uuid::Uuid;
use alloc::vec;
use bitflags::bitflags;

/// Size in bytes of the journal superblock. (Note that the underlying
/// block size may be larger, but only the first 1024 bytes are used.)
const SUPERBLOCK_SIZE: usize = 1024;

const CHECKSUM_TYPE_CRC32C: u8 = 4;

// Field offsets within the superblock.
const SUPERBLOCK_BLOCKSIZE_OFFSET: usize = 0xc;
const SUPERBLOCK_SEQUENCE_OFFSET: usize = 0x18;
const SUPERBLOCK_START_OFFSET: usize = 0x1c;
const SUPERBLOCK_FEATURE_INCOMPAT_OFFSET: usize = 0x28;
const SUPERBLOCK_UUID_OFFSET: usize = 0x30;
const SUPERBLOCK_CHECKSUM_TYPE_OFFSET: usize = 0x50;
const SUPERBLOCK_CHECKSUM_OFFSET: usize = 0xfc;

/// Features that must be present for this library to read the journal.
const REQUIRED_FEATURES: JournalIncompatibleFeatures =
    JournalIncompatibleFeatures::IS_64BIT.union(JournalIncompatibleFeatures::CHECKSUM_V3);

/// Features that may be present, but are not required.
const ALLOWED_FEATURES: JournalIncompatibleFeatures =
    JournalIncompatibleFeatures::BLOCK_REVOCATIONS;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct JournalSuperblock {
    /// Size in bytes of journal blocks. This must be the same block
    /// size as the main filesystem.
    pub(super) block_size: u32,

    /// Sequence number of the first journal commit to apply.
    pub(super) sequence: u32,

    /// Index of the journal block from which to start reading
    /// data. This index is relative to the journal superblock.
    pub(super) start_block: u32,

    /// Journal UUID used for checksums.
    pub(super) uuid: Uuid,
}

impl JournalSuperblock {
    /// Load the journal superblock from the filesystem.
    ///
    /// An error is returned if:
    /// * The superblock cannot be read from the filesystem.
    /// * `JournalSuperblock::read_bytes` fails.
    /// * The journal's block size does not match the filesystem block size.
    pub(super) fn load(fs: &Ext4, journal_inode: &Inode) -> Result<Self, Ext4Error> {
        // Get an iterator over the journal's block indices.
        let mut journal_block_iter = FileBlocks::new(fs.clone(), journal_inode)?;

        // Read the first 1024 bytes of the first block. This is the
        // journal's superblock.
        let block_index = journal_block_iter
            .next()
            .ok_or(CorruptKind::JournalSize)??;
        let mut block = vec![0; SUPERBLOCK_SIZE];
        fs.read_from_block(block_index, 0, &mut block)?;

        let superblock = Self::read_bytes(&block)?;

        // Ensure the journal block size matches the rest of the
        // filesystem.
        if superblock.block_size != fs.0.superblock.block_size {
            return Err(CorruptKind::JournalBlockSize.into());
        }

        Ok(superblock)
    }

    /// Read superblock data from `bytes`.
    ///
    /// An error is returned if:
    /// * The superblock magic number is not present.
    /// * The superblock type is unsupported.
    /// * The checksum type is unsupported.
    /// * The superblock's checksum is incorrect.
    fn read_bytes(bytes: &[u8]) -> Result<Self, Ext4Error> {
        assert_eq!(bytes.len(), SUPERBLOCK_SIZE);

        let header = JournalBlockHeader::read_bytes(bytes).ok_or(CorruptKind::JournalMagic)?;

        // For now only superblock v2 is supported.
        if header.block_type != JournalBlockType::SUPERBLOCK_V2 {
            return Err(IncompatibleKind::JournalSuperblockType(header.block_type.0).into());
        }

        let s_blocksize = read_u32be(bytes, SUPERBLOCK_BLOCKSIZE_OFFSET);
        let s_sequence = read_u32be(bytes, SUPERBLOCK_SEQUENCE_OFFSET);
        let s_start = read_u32be(bytes, SUPERBLOCK_START_OFFSET);
        let s_feature_incompat = read_u32be(bytes, SUPERBLOCK_FEATURE_INCOMPAT_OFFSET);
        let s_uuid = &bytes[SUPERBLOCK_UUID_OFFSET..SUPERBLOCK_UUID_OFFSET + 16];
        let s_checksum_type = bytes[SUPERBLOCK_CHECKSUM_TYPE_OFFSET];
        let s_checksum = read_u32be(bytes, SUPERBLOCK_CHECKSUM_OFFSET);

        check_incompat_features(s_feature_incompat)?;

        // For now only one checksum type is supported.
        if s_checksum_type != CHECKSUM_TYPE_CRC32C {
            return Err(IncompatibleKind::JournalChecksumType(s_checksum_type).into());
        }

        // Validate the superblock checksum.
        let mut checksum = Checksum::new();
        checksum.update(&bytes[..SUPERBLOCK_CHECKSUM_OFFSET]);
        checksum.update_u32_le(0);
        checksum.update(&bytes[SUPERBLOCK_CHECKSUM_OFFSET + 4..SUPERBLOCK_SIZE]);
        if checksum.finalize() != s_checksum {
            return Err(CorruptKind::JournalSuperblockChecksum.into());
        }

        // OK to unwrap: `s_uuid` is always 16 bytes.
        let uuid = Uuid(s_uuid.try_into().unwrap());

        Ok(Self {
            block_size: s_blocksize,
            sequence: s_sequence,
            start_block: s_start,
            uuid,
        })
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub(crate) struct JournalIncompatibleFeatures: u32 {
        const BLOCK_REVOCATIONS = 0x1;
        const IS_64BIT = 0x2;
        const ASYNC_COMMITS = 0x4;
        const CHECKSUM_V2 = 0x8;
        const CHECKSUM_V3 = 0x10;
        const FAST_COMMITS = 0x20;
    }
}

/// Check that journal features required by this library are present,
/// and that no unsupported features are present.
fn check_incompat_features(s_feature_incompat: u32) -> Result<(), IncompatibleKind> {
    let present = JournalIncompatibleFeatures::from_bits_retain(s_feature_incompat);

    let present_required = present & REQUIRED_FEATURES;
    if present_required != REQUIRED_FEATURES {
        return Err(IncompatibleKind::MissingRequiredJournalFeatures(
            REQUIRED_FEATURES.difference(present).bits(),
        ));
    }

    // Note: the `bits` conversion is needed because otherwise the `!`
    // would only negate "known" bits specified in the bitflags
    // definition. Convert to raw bits first to correct this.
    let unsupported = !((REQUIRED_FEATURES | ALLOWED_FEATURES).bits());

    let present_unsupported = present.bits() & unsupported;
    if present_unsupported != 0 {
        return Err(IncompatibleKind::UnsupportedJournalFeatures(
            present_unsupported,
        ));
    }

    Ok(())
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::test_util::load_compressed_filesystem;

    #[test]
    fn test_load_journal_superblock() {
        let fs = load_compressed_filesystem("test_disk_4k_block_journal.bin.zst");
        let journal_inode = Inode::read(&fs, fs.0.superblock.journal_inode.unwrap()).unwrap();
        let superblock = JournalSuperblock::load(&fs, &journal_inode).unwrap();
        assert_eq!(
            superblock,
            JournalSuperblock {
                block_size: 4096,
                sequence: 3,
                start_block: 289,
                uuid: Uuid([
                    0xd2, 0x28, 0xa8, 0x78, 0xb9, 0xa7, 0x49, 0xe4, 0x9e, 0x3d, 0xbb, 0xee, 0xd5,
                    0x60, 0x1c, 0xd3
                ]),
            }
        );
    }

    fn write_u32be(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn create_test_superblock() -> Vec<u8> {
        let mut block = vec![0; 1024];
        // Set magic.
        write_u32be(&mut block, 0, JournalBlockHeader::MAGIC);
        // Set superblock type.
        write_u32be(&mut block, 4, 4);
        // Set block size.
        write_u32be(&mut block, SUPERBLOCK_BLOCKSIZE_OFFSET, 4096);
        // Set sequence.
        write_u32be(&mut block, SUPERBLOCK_SEQUENCE_OFFSET, 123);
        // Set start block.
        write_u32be(&mut block, SUPERBLOCK_START_OFFSET, 456);
        // Set features.
        write_u32be(&mut block, SUPERBLOCK_FEATURE_INCOMPAT_OFFSET, 0x12);
        // Set UUID.
        block[SUPERBLOCK_UUID_OFFSET..SUPERBLOCK_UUID_OFFSET + 16].copy_from_slice(&[0xab; 16]);
        // Set checksum type.
        block[SUPERBLOCK_CHECKSUM_TYPE_OFFSET] = CHECKSUM_TYPE_CRC32C;
        // Set checksum.
        write_u32be(&mut block, SUPERBLOCK_CHECKSUM_OFFSET, 0x78a2_c32b);
        block
    }

    #[test]
    fn test_journal_superblock_read_success() {
        let block = create_test_superblock();
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap(),
            JournalSuperblock {
                block_size: 4096,
                sequence: 123,
                start_block: 456,
                uuid: Uuid([0xab; 16]),
            }
        );
    }

    #[test]
    fn test_journal_superblock_invalid_magic() {
        let mut block = create_test_superblock();
        // Override magic in the block header.
        write_u32be(&mut block, 0, 0);
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            CorruptKind::JournalMagic
        );
    }

    #[test]
    fn test_journal_superblock_unsupported_type() {
        let mut block = create_test_superblock();
        // Override type in the block header.
        write_u32be(&mut block, 4, 0);
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            IncompatibleKind::JournalSuperblockType(0)
        );
    }

    #[test]
    fn test_journal_superblock_missing_required_features() {
        let mut block = create_test_superblock();
        write_u32be(&mut block, SUPERBLOCK_FEATURE_INCOMPAT_OFFSET, 0);
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            IncompatibleKind::MissingRequiredJournalFeatures(
                (JournalIncompatibleFeatures::IS_64BIT | JournalIncompatibleFeatures::CHECKSUM_V3)
                    .bits()
            ),
        );
    }

    #[test]
    fn test_journal_superblock_unsupported_features() {
        let mut block = create_test_superblock();
        write_u32be(
            &mut block,
            SUPERBLOCK_FEATURE_INCOMPAT_OFFSET,
            (REQUIRED_FEATURES
                // Known but unsupported features.
                | JournalIncompatibleFeatures::FAST_COMMITS
                | JournalIncompatibleFeatures::ASYNC_COMMITS)
                .bits()
                // An unknown and unsupported feature.
                | 0x10_000,
        );
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            IncompatibleKind::UnsupportedJournalFeatures(
                (JournalIncompatibleFeatures::FAST_COMMITS
                    | JournalIncompatibleFeatures::ASYNC_COMMITS)
                    .bits()
                    | 0x10_000
            ),
        );
    }

    #[test]
    fn test_journal_superblock_unsupported_checksum_type() {
        let mut block = create_test_superblock();
        block[SUPERBLOCK_CHECKSUM_TYPE_OFFSET] = 0;
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            IncompatibleKind::JournalChecksumType(0)
        );
    }

    #[test]
    fn test_journal_superblock_incorrect_checksum() {
        let mut block = create_test_superblock();
        write_u32be(&mut block, SUPERBLOCK_CHECKSUM_OFFSET, 0);
        assert_eq!(
            JournalSuperblock::read_bytes(&block).unwrap_err(),
            CorruptKind::JournalSuperblockChecksum,
        );
    }
}
