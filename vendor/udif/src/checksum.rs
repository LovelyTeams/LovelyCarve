//! Checksum verification for DMG files
//!
//! DMG files use CRC32 checksums (type 2) to verify integrity:
//! - Data checksum: CRC32 of the data fork (compressed blocks)
//! - Master checksum: CRC32 of all mish block checksums concatenated
//! - Mish checksum: CRC32 of the decompressed partition data

use byteorder::{BigEndian, ByteOrder};

/// Checksum type constants
pub const CHECKSUM_TYPE_NONE: u32 = 0;
pub const CHECKSUM_TYPE_CRC32: u32 = 2;

/// Calculate CRC32 checksum of data
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Extract the CRC32 value from a 128-byte checksum array
/// The checksum is stored as big-endian u32 in the first 4 bytes
pub fn extract_crc32(checksum_array: &[u8; 128]) -> u32 {
    BigEndian::read_u32(&checksum_array[0..4])
}

/// Create a 128-byte checksum array from a CRC32 value
pub fn create_checksum_array(crc: u32) -> [u8; 128] {
    let mut array = [0u8; 128];
    BigEndian::write_u32(&mut array[0..4], crc);
    array
}

/// Check if a checksum is present (non-zero)
pub fn has_checksum(checksum_type: u32, checksum_array: &[u8; 128]) -> bool {
    if checksum_type != CHECKSUM_TYPE_CRC32 {
        return false;
    }
    // Check if the checksum value is non-zero
    extract_crc32(checksum_array) != 0
}

/// Verify a CRC32 checksum
/// Returns Ok(()) if checksum matches or if no checksum is present
/// Returns Err with expected/actual values if mismatch
pub fn verify_crc32(
    checksum_type: u32,
    checksum_array: &[u8; 128],
    data: &[u8],
) -> Result<(), (u32, u32)> {
    // If not CRC32 type, skip verification
    if checksum_type != CHECKSUM_TYPE_CRC32 {
        return Ok(());
    }

    let expected = extract_crc32(checksum_array);

    // If checksum is zero, skip verification (not set)
    if expected == 0 {
        return Ok(());
    }

    let actual = crc32(data);

    if expected == actual {
        Ok(())
    } else {
        Err((expected, actual))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_known_value() {
        // "123456789" has well-known CRC32 value
        let data = b"123456789";
        let crc = crc32(data);
        assert_eq!(crc, 0xCBF43926);
    }

    #[test]
    fn test_extract_crc32() {
        let mut array = [0u8; 128];
        array[0] = 0xDE;
        array[1] = 0xAD;
        array[2] = 0xBE;
        array[3] = 0xEF;
        assert_eq!(extract_crc32(&array), 0xDEADBEEF);
    }

    #[test]
    fn test_create_checksum_array() {
        let array = create_checksum_array(0xDEADBEEF);
        assert_eq!(array[0], 0xDE);
        assert_eq!(array[1], 0xAD);
        assert_eq!(array[2], 0xBE);
        assert_eq!(array[3], 0xEF);
        // Rest should be zeros
        for val in &array[4..128] {
            assert_eq!(*val, 0);
        }
    }

    #[test]
    fn test_verify_crc32_match() {
        let data = b"test data";
        let crc = crc32(data);
        let array = create_checksum_array(crc);
        assert!(verify_crc32(CHECKSUM_TYPE_CRC32, &array, data).is_ok());
    }

    #[test]
    fn test_verify_crc32_mismatch() {
        let data = b"test data";
        let array = create_checksum_array(0x12345678);
        let result = verify_crc32(CHECKSUM_TYPE_CRC32, &array, data);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_crc32_zero_skipped() {
        // Zero checksum should be skipped
        let array = [0u8; 128];
        assert!(verify_crc32(CHECKSUM_TYPE_CRC32, &array, b"any data").is_ok());
    }

    #[test]
    fn test_verify_crc32_wrong_type_skipped() {
        // Non-CRC32 type should be skipped
        let array = create_checksum_array(0x12345678);
        assert!(verify_crc32(CHECKSUM_TYPE_NONE, &array, b"any data").is_ok());
    }

    #[test]
    fn test_has_checksum() {
        let array = create_checksum_array(0x12345678);
        assert!(has_checksum(CHECKSUM_TYPE_CRC32, &array));

        let zero_array = [0u8; 128];
        assert!(!has_checksum(CHECKSUM_TYPE_CRC32, &zero_array));
        assert!(!has_checksum(CHECKSUM_TYPE_NONE, &array));
    }
}
