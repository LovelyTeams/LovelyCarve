// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::block_size::BlockSize;
use crate::checksum::Checksum;
use crate::error::{CorruptKind, Ext4Error, IncompatibleKind};
use crate::features::{CompatibleFeatures, IncompatibleFeatures, ReadOnlyCompatibleFeatures};
use crate::inode::InodeIndex;
use crate::util::{read_u16le, read_u32le, u64_from_hilo};
use crate::{Label, Uuid};
use core::num::NonZero;

/// Information about the filesystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Superblock {
    pub(crate) block_size: BlockSize,
    pub(crate) blocks_count: u64,
    pub(crate) blocks_per_group: u32,
    pub(crate) first_data_block: u32,
    pub(crate) first_meta_bg: u32,
    pub(crate) sparse_super2: bool,
    pub(crate) backup_bgs: [u32; 2],
    pub(crate) inode_size: u16,
    pub(crate) inode_count: u32,
    pub(crate) inodes_per_block_group: NonZero<u32>,
    pub(crate) block_group_descriptor_size: u16,
    pub(crate) num_block_groups: u32,
    pub(crate) incompatible_features: IncompatibleFeatures,
    pub(crate) read_only_compatible_features: ReadOnlyCompatibleFeatures,
    pub(crate) checksum_seed: u32,
    pub(crate) htree_hash_seed: [u32; 4],
    pub(crate) journal_inode: Option<InodeIndex>,
    pub(crate) label: Label,
    pub(crate) uuid: Uuid,
}

impl Superblock {
    /// Size (in bytes) of the superblock on disk.
    pub(crate) const SIZE_IN_BYTES_ON_DISK: usize = 1024;

    const S_FEATURE_RO_COMPAT_OFFSET: usize = 0x64;
    const S_DESC_SIZE_OFFSET: usize = 0xfe;

    /// Construct `Superblock` from bytes.
    ///
    /// # Panics
    ///
    /// Panics if the length of `bytes` is less than
    /// [`Self::SIZE_IN_BYTES_ON_DISK`].
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, Ext4Error> {
        assert!(bytes.len() >= Self::SIZE_IN_BYTES_ON_DISK);

        // OK to unwrap: already checked the length.
        let s_blocks_count_lo = read_u32le(bytes, 0x4);
        let s_first_data_block = read_u32le(bytes, 0x14);
        let s_log_block_size = read_u32le(bytes, 0x18);
        let s_blocks_per_group = read_u32le(bytes, 0x20);
        let s_inodes_per_group = read_u32le(bytes, 0x28);
        let s_magic = read_u16le(bytes, 0x38);
        let s_inode_size = if read_u32le(bytes, 0x4c) == 0 {
            128
        } else {
            read_u16le(bytes, 0x58)
        };
        let s_feature_compat = read_u32le(bytes, 0x5c);
        let s_feature_incompat = read_u32le(bytes, 0x60);
        let s_feature_ro_compat = read_u32le(bytes, Self::S_FEATURE_RO_COMPAT_OFFSET);
        let s_uuid = &bytes[0x68..0x68 + 16];
        let s_volume_name = &bytes[0x78..0x78 + 16];
        let s_journal_inum = read_u32le(bytes, 0xe0);
        const S_HASH_SEED_OFFSET: usize = 0xec;
        let s_hash_seed = [
            read_u32le(bytes, S_HASH_SEED_OFFSET),
            read_u32le(bytes, S_HASH_SEED_OFFSET + 4),
            read_u32le(bytes, S_HASH_SEED_OFFSET + 8),
            read_u32le(bytes, S_HASH_SEED_OFFSET + 12),
        ];
        let s_desc_size = read_u16le(bytes, Self::S_DESC_SIZE_OFFSET);
        let s_blocks_count_hi = read_u32le(bytes, 0x150);
        let s_checksum_seed = read_u32le(bytes, 0x270);
        const S_CHECKSUM_OFFSET: usize = 0x3fc;
        let s_checksum = read_u32le(bytes, S_CHECKSUM_OFFSET);

        let blocks_count = if s_feature_incompat & IncompatibleFeatures::IS_64BIT.bits() != 0 {
            u64_from_hilo(s_blocks_count_hi, s_blocks_count_lo)
        } else {
            u64::from(s_blocks_count_lo)
        };

        let block_size = BlockSize::from_superblock_value(s_log_block_size)
            .ok_or(CorruptKind::InvalidBlockSize)?;

        if block_size.to_u32() > 65536 {
            return Err(CorruptKind::InvalidBlockSize.into());
        }
        if s_magic != 0xef53 {
            return Err(CorruptKind::SuperblockMagic.into());
        }

        let incompatible_features = check_incompat_features(s_feature_incompat)?;
        let read_only_compatible_features =
            ReadOnlyCompatibleFeatures::from_bits_retain(s_feature_ro_compat);
        let compatible_features = CompatibleFeatures::from_bits_retain(s_feature_compat);

        // s_first_data_block is usually 1 if the block size is 1KiB,
        // and otherwise its usually 0.
        let num_data_blocks = blocks_count
            .checked_sub(u64::from(s_first_data_block))
            .ok_or(CorruptKind::FirstDataBlock(s_first_data_block))?;
        // Use div_ceil to round up in case `num_data_blocks` isn't an
        // even multiple of `s_blocks_per_group`. (Consider for example
        // `num_data_blocks = 3` and `s_blocks_per_group = 4`; that is
        // one block group, but regular division would calculate zero
        // instead of one.)
        let blocks_per_group =
            NonZero::new(s_blocks_per_group).ok_or(CorruptKind::BlocksPerGroup)?;
        let num_block_groups =
            u32::try_from(num_data_blocks.div_ceil(u64::from(blocks_per_group.get())))
                .map_err(|_| CorruptKind::TooManyBlockGroups)?;

        let inodes_per_block_group =
            NonZero::new(s_inodes_per_group).ok_or(CorruptKind::InodesPerBlockGroup)?;

        let block_group_descriptor_size =
            if incompatible_features.contains(IncompatibleFeatures::IS_64BIT) {
                if s_desc_size < 64 {
                    return Err(CorruptKind::BlockGroupDescriptorSize.into());
                }
                s_desc_size
            } else {
                32
            };

        // Inodes are not allowed to exceed the block size.
        if s_inode_size < 128 || !s_inode_size.is_power_of_two() || s_inode_size > block_size {
            return Err(CorruptKind::InodeSize.into());
        }

        let journal_inode = if compatible_features.contains(CompatibleFeatures::HAS_JOURNAL)
            && incompatible_features.contains(IncompatibleFeatures::RECOVERY)
        {
            // For now a separate journal device is not supported, so
            // assert that feature is not present. This assert cannot
            // fail because of the call to `check_incompat_features`
            // above.
            assert!(!incompatible_features.contains(IncompatibleFeatures::SEPARATE_JOURNAL_DEVICE));

            Some(InodeIndex::new(s_journal_inum).ok_or(CorruptKind::JournalInode)?)
        } else {
            None
        };

        // Validate the superblock checksum.
        if read_only_compatible_features.contains(ReadOnlyCompatibleFeatures::METADATA_CHECKSUMS) {
            let mut checksum = Checksum::new();
            checksum.update(&bytes[..S_CHECKSUM_OFFSET]);
            if s_checksum != checksum.finalize() {
                return Err(CorruptKind::SuperblockChecksum.into());
            }
        }

        let checksum_seed =
            if incompatible_features.contains(IncompatibleFeatures::CHECKSUM_SEED_IN_SUPERBLOCK) {
                s_checksum_seed
            } else {
                let mut checksum = Checksum::new();
                checksum.update(s_uuid);
                checksum.finalize()
            };

        // OK to unwrap: `s_volume_name` is always 16 bytes.
        let label = Label::new(s_volume_name.try_into().unwrap());

        // OK to unwrap: `s_uuid` is always 16 bytes.
        let uuid = Uuid(s_uuid.try_into().unwrap());

        Ok(Self {
            block_size,
            blocks_count,
            blocks_per_group: s_blocks_per_group,
            first_data_block: s_first_data_block,
            first_meta_bg: read_u32le(bytes, 0x104),
            sparse_super2: s_feature_compat & 0x200 != 0,
            backup_bgs: [read_u32le(bytes, 0x24c), read_u32le(bytes, 0x250)],
            inode_size: s_inode_size,
            inode_count: read_u32le(bytes, 0),
            inodes_per_block_group,
            block_group_descriptor_size,
            num_block_groups,
            incompatible_features,
            read_only_compatible_features,
            checksum_seed,
            htree_hash_seed: s_hash_seed,
            journal_inode,
            label,
            uuid,
        })
    }
}

fn check_incompat_features(
    s_feature_incompat: u32,
) -> Result<IncompatibleFeatures, IncompatibleKind> {
    let actual = IncompatibleFeatures::from_bits_retain(s_feature_incompat);
    let actual_known = IncompatibleFeatures::from_bits_truncate(s_feature_incompat);
    if actual != actual_known {
        return Err(IncompatibleKind::UnsupportedFeatures(
            actual.difference(actual_known),
        ));
    }

    // TODO: for now, be strict on many incompat features. May be able to
    // relax some of these in the future.
    let required_features = IncompatibleFeatures::empty();
    let disallowed_features = IncompatibleFeatures::COMPRESSION
        | IncompatibleFeatures::SEPARATE_JOURNAL_DEVICE
        | IncompatibleFeatures::DATA_IN_DIR_ENTRY;

    let present_required = actual & required_features;
    if present_required != required_features {
        return Err(IncompatibleKind::MissingRequiredFeatures(
            required_features.difference(present_required),
        ));
    }

    let present_disallowed = actual & disallowed_features;
    if !present_disallowed.is_empty() {
        return Err(IncompatibleKind::UnsupportedFeatures(present_disallowed));
    }

    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_superblock() {
        let data = include_bytes!("../test_data/raw_superblock.bin");
        let sb = Superblock::from_bytes(data).unwrap();
        assert_eq!(
            sb,
            Superblock {
                block_size: BlockSize::from_superblock_value(0).unwrap(),
                blocks_count: 128,
                blocks_per_group: 8192,
                first_data_block: 1,
                first_meta_bg: 0,
                sparse_super2: false,
                backup_bgs: [0, 0],
                inode_size: 256,
                inode_count: 16,
                inodes_per_block_group: NonZero::new(16).unwrap(),
                block_group_descriptor_size: 64,
                num_block_groups: 1,
                incompatible_features: IncompatibleFeatures::FILE_TYPE_IN_DIR_ENTRY
                    | IncompatibleFeatures::EXTENTS
                    | IncompatibleFeatures::IS_64BIT
                    | IncompatibleFeatures::FLEXIBLE_BLOCK_GROUPS
                    | IncompatibleFeatures::CHECKSUM_SEED_IN_SUPERBLOCK,
                read_only_compatible_features: ReadOnlyCompatibleFeatures::SPARSE_SUPERBLOCKS
                    | ReadOnlyCompatibleFeatures::LARGE_FILES
                    | ReadOnlyCompatibleFeatures::HUGE_FILES
                    | ReadOnlyCompatibleFeatures::LARGE_DIRECTORIES
                    | ReadOnlyCompatibleFeatures::LARGE_INODES
                    | ReadOnlyCompatibleFeatures::METADATA_CHECKSUMS,
                checksum_seed: 0xfd3cc0be,
                htree_hash_seed: [0xbb071441, 0x7746982f, 0x6007bb8f, 0xb61a9b7],
                journal_inode: None,
                label: Label::new([0; 16]),
                uuid: Uuid([
                    0xb6, 0x20, 0x21, 0xd2, 0x70, 0xe5, 0x4d, 0x2c, 0x8a, 0x2d, 0x50, 0x93, 0x4f,
                    0x1b, 0xaf, 0x77
                ]),
            }
        );
    }

    /// Test that the checksum seed gets correctly calculated from the
    /// filesystem uuid if the `CHECKSUM_SEED_IN_SUPERBLOCK` incompat
    /// feature is not set.
    #[test]
    fn test_no_checksum_seed() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();

        // Byte range of `s_feature_incompat`.
        let ifeat_range = 0x60..0x64;

        // Get the current features value, remove `CHECKSUM_SEED_IN_SUPERBLOCK`,
        // and write it back out.
        let mut ifeat = IncompatibleFeatures::from_bits_retain(u32::from_le_bytes(
            data[ifeat_range.clone()].try_into().unwrap(),
        ));
        ifeat.remove(IncompatibleFeatures::CHECKSUM_SEED_IN_SUPERBLOCK);
        data[ifeat_range].copy_from_slice(&ifeat.bits().to_le_bytes());

        // Byte range of `s_checksum_seed`.
        let seed_range = 0x270..0x274;

        // Get the current seed value, then clear those bytes.
        let expected_seed = u32::from_le_bytes(data[seed_range.clone()].try_into().unwrap());
        let fill_seed = 0u32;
        data[seed_range].copy_from_slice(&fill_seed.to_le_bytes());
        // Ensure that the fill seed doesn't match the existing seed,
        // otherwise this test isn't testing anything.
        assert_ne!(expected_seed, fill_seed);

        // Update the checksum.
        let mut checksum = Checksum::new();
        checksum.update(&data[..0x3fc]);
        data[0x3fc..].copy_from_slice(&checksum.finalize().to_le_bytes());

        let sb = Superblock::from_bytes(&data).unwrap();
        // Check that the correct seed was calculated.
        assert_eq!(sb.checksum_seed, expected_seed);
    }

    #[test]
    fn test_too_many_block_groups() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();
        // Set `s_blocks_count_hi` to a very large value so that
        // `num_block_groups` no longer fits in a `u32`.
        data[0x150..0x154].copy_from_slice(&[0xff; 4]);
        assert_eq!(
            Superblock::from_bytes(&data).unwrap_err(),
            CorruptKind::TooManyBlockGroups
        );
    }

    #[test]
    fn test_invalid_inode_size() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();
        data[0x58..0x5a].copy_from_slice(&1025u16.to_le_bytes());
        assert_eq!(
            Superblock::from_bytes(&data).unwrap_err(),
            CorruptKind::InodeSize
        );
    }

    #[test]
    fn test_bad_superblock_checksum() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();
        // Modify a reserved byte. Nothing currently uses this data, but
        // it is still part of the checksum.
        data[0x284] = 0xff;
        assert_eq!(
            Superblock::from_bytes(&data).unwrap_err(),
            CorruptKind::SuperblockChecksum
        );
    }

    /// Test that an error is returned if an unknown incompatible
    /// feature bit is set.
    #[test]
    fn test_unknown_incompat_flags() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();
        data[0x62] |= 0x04;
        assert_eq!(
            Superblock::from_bytes(&data).unwrap_err(),
            IncompatibleKind::UnsupportedFeatures(IncompatibleFeatures::from_bits_retain(0x4_0000))
        );
    }

    /// Test that an error is returned if `s_blocks_per_group` is zero,
    /// rather than panicking with a divide-by-zero in `div_ceil`.
    #[test]
    fn test_blocks_per_group_zero() {
        let mut data = include_bytes!("../test_data/raw_superblock.bin").to_vec();
        // s_blocks_per_group is a 4-byte LE field at offset 0x20.
        data[0x20] = 0;
        data[0x21] = 0;
        data[0x22] = 0;
        data[0x23] = 0;
        assert_eq!(
            Superblock::from_bytes(&data).unwrap_err(),
            CorruptKind::BlocksPerGroup
        );
    }

    #[test]
    fn test_check_incompat_features() {
        let required = (IncompatibleFeatures::FILE_TYPE_IN_DIR_ENTRY
            | IncompatibleFeatures::FLEXIBLE_BLOCK_GROUPS
            | IncompatibleFeatures::CHECKSUM_SEED_IN_SUPERBLOCK)
            .bits();

        // Success.
        assert!(check_incompat_features(required).is_ok());

        // Unknown incompatible bit is an error.
        assert_eq!(
            check_incompat_features(required | 0x4_0000).unwrap_err(),
            IncompatibleKind::UnsupportedFeatures(IncompatibleFeatures::from_bits_retain(0x4_0000))
        );

        assert!(check_incompat_features(0).is_ok());

        assert_eq!(
            check_incompat_features(
                required | IncompatibleFeatures::SEPARATE_JOURNAL_DEVICE.bits()
            )
            .unwrap_err(),
            IncompatibleKind::UnsupportedFeatures(IncompatibleFeatures::SEPARATE_JOURNAL_DEVICE)
        );
    }

    /// Test that an invalid `s_desc_size` produces an error rather than
    /// crashing.
    #[cfg(feature = "std")]
    #[test]
    fn test_invalid_s_desc_size() {
        let mut data = crate::test_util::load_compressed_data("test_disk1.bin.zst");
        let superblock_start = 1024;

        // Turn off checksums to make the filesystem easier to modify.
        let mut s_feature_ro_compat = u32::from_le_bytes(
            data[superblock_start + Superblock::S_FEATURE_RO_COMPAT_OFFSET..][..4]
                .try_into()
                .unwrap(),
        );
        s_feature_ro_compat &= !ReadOnlyCompatibleFeatures::METADATA_CHECKSUMS.bits();
        data[superblock_start + Superblock::S_FEATURE_RO_COMPAT_OFFSET..][..4]
            .copy_from_slice(&s_feature_ro_compat.to_le_bytes());

        // Set `s_desc_size`.
        data[superblock_start + Superblock::S_DESC_SIZE_OFFSET..][..2]
            .copy_from_slice(&1u16.to_le_bytes());

        assert_eq!(
            crate::Ext4::load(Box::new(data)).unwrap_err(),
            CorruptKind::BlockGroupDescriptorSize
        );
    }
}
