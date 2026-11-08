//! The 512-byte CoreStorage physical volume header (partition start, LE fields).
//!
//! See `docs/RESEARCH.md` — every offset below is cross-checked against libfvde
//! and the dfvfs `fvdetest` ground truth.

use crate::error::FileVaultError;
use crate::read::{bytes16, le_u16, le_u32, le_u64};

/// CoreStorage signature `"CS"` (bytes 0x43 0x53 at offsets 88,89) read
/// little-endian at offset 88 -> 0x5343.
const CS_SIGNATURE_LE: u16 = 0x5343;
/// Encryption method code for AES-XTS-128.
const ENCRYPTION_METHOD_AES_XTS_128: u32 = 2;

/// Number of metadata block-number slots in the header (offset 104, 4 × u64).
const METADATA_BLOCK_SLOTS: usize = 4;

/// The parsed physical volume header.
#[derive(Debug, Clone)]
pub struct VolumeHeader {
    /// CoreStorage block size in bytes (typically 4096); the multiplier that
    /// turns a block number into a byte offset.
    pub block_size: u32,
    /// Bytes per sector (typically 512); the AES-XTS unit for LV decryption.
    pub bytes_per_sector: u32,
    /// Physical volume size in bytes (header offset 64).
    pub physical_volume_size: u64,
    /// Metadata block numbers (header offset 104, 4 × u64). Multiply by
    /// `block_size` for a byte offset.
    pub metadata_block_numbers: [u64; METADATA_BLOCK_SLOTS],
    /// The 16-byte metadata XTS key1 (header offset 176, first 16 bytes of the
    /// 128-byte key data).
    pub key_data: [u8; 16],
    /// The physical volume identifier (header offset 304, on-disk byte order) —
    /// the metadata XTS key2.
    pub physical_volume_identifier: [u8; 16],
    /// Logical volume group UUID from the same selected physical header.
    pub logical_volume_group_identifier: [u8; 16],
}

impl VolumeHeader {
    /// Parse the 512-byte header from the start of a CoreStorage partition.
    ///
    /// # Errors
    /// - [`FileVaultError::NotCoreStorage`] if the `"CS"` signature is absent.
    /// - [`FileVaultError::UnsupportedEncryptionMethod`] if not AES-XTS-128.
    pub fn parse(data: &[u8]) -> Result<Self, FileVaultError> {
        let signature = le_u16(data, 88);
        if signature != CS_SIGNATURE_LE {
            return Err(FileVaultError::NotCoreStorage { found: signature });
        }

        let encryption_method = le_u32(data, 172);
        if encryption_method != ENCRYPTION_METHOD_AES_XTS_128 {
            return Err(FileVaultError::UnsupportedEncryptionMethod {
                found: encryption_method,
            });
        }

        let mut metadata_block_numbers = [0u64; METADATA_BLOCK_SLOTS];
        for (i, slot) in metadata_block_numbers.iter_mut().enumerate() {
            *slot = le_u64(data, 104 + i * 8);
        }

        Ok(VolumeHeader {
            block_size: le_u32(data, 96),
            bytes_per_sector: le_u32(data, 48),
            physical_volume_size: le_u64(data, 64),
            metadata_block_numbers,
            key_data: bytes16(data, 176),
            physical_volume_identifier: bytes16(data, 304),
            logical_volume_group_identifier: bytes16(data, 320),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 512-byte header carrying the dfvfs `fvdetest` ground-truth field
    /// values (RESEARCH.md Tier-2 table).
    fn ground_truth_header() -> [u8; 512] {
        let mut h = [0u8; 512];
        h[88] = b'C';
        h[89] = b'S';
        h[48..52].copy_from_slice(&512u32.to_le_bytes());
        h[96..100].copy_from_slice(&4096u32.to_le_bytes());
        h[64..72].copy_from_slice(&536_829_952u64.to_le_bytes());
        h[172..176].copy_from_slice(&2u32.to_le_bytes());
        for (i, block) in [1u64, 1025, 129_013, 130_037].iter().enumerate() {
            h[104 + i * 8..104 + i * 8 + 8].copy_from_slice(&block.to_le_bytes());
        }
        h[176..192].copy_from_slice(&hex("18eaeb7da9ab0852ead69e9dabc86f59"));
        h[304..320].copy_from_slice(&hex("3273a0553b8b47e8b970df35eecda81b"));
        h
    }

    fn hex(s: &str) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn parses_ground_truth_fields() {
        let header = VolumeHeader::parse(&ground_truth_header()).unwrap();
        assert_eq!(header.block_size, 4096);
        assert_eq!(header.bytes_per_sector, 512);
        assert_eq!(header.physical_volume_size, 536_829_952);
        assert_eq!(header.metadata_block_numbers, [1, 1025, 129_013, 130_037]);
        assert_eq!(header.key_data, hex("18eaeb7da9ab0852ead69e9dabc86f59"));
        assert_eq!(
            header.physical_volume_identifier,
            hex("3273a0553b8b47e8b970df35eecda81b")
        );
    }

    #[test]
    fn rejects_non_corestorage() {
        let mut h = ground_truth_header();
        h[88] = b'X';
        let err = VolumeHeader::parse(&h).unwrap_err();
        assert!(
            matches!(err, FileVaultError::NotCoreStorage { found } if found & 0x00ff == u16::from(b'X'))
        );
    }

    #[test]
    fn rejects_unsupported_encryption_method() {
        let mut h = ground_truth_header();
        h[172..176].copy_from_slice(&7u32.to_le_bytes());
        let err = VolumeHeader::parse(&h).unwrap_err();
        assert!(matches!(
            err,
            FileVaultError::UnsupportedEncryptionMethod { found: 7 }
        ));
    }

    #[test]
    fn short_input_is_not_corestorage() {
        assert!(matches!(
            VolumeHeader::parse(&[0u8; 10]),
            Err(FileVaultError::NotCoreStorage { .. })
        ));
    }
}
