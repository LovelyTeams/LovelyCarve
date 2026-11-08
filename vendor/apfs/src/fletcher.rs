//! Fletcher-64 checksum used by APFS.
//!
//! Every on-disk object has a 64-bit checksum at offset 0, computed over
//! bytes 8..block_size using a modular Fletcher-64 variant.

/// Compute APFS Fletcher-64 checksum over a byte slice.
///
/// The input should be the object data starting at offset 8 (skipping the
/// checksum field itself). Data length must be a multiple of 4.
pub fn fletcher64(data: &[u8]) -> u64 {
    // APFS uses a variant of Fletcher-64 that operates on 32-bit words.
    // The modulus is 2^32 - 1 (0xFFFFFFFF).
    let mod_val: u64 = 0xFFFFFFFF;

    let mut sum1: u64 = 0;
    let mut sum2: u64 = 0;

    // Process 4 bytes at a time (little-endian u32)
    for chunk in data.as_chunks::<4>().0 {
        let word = u32::from_le_bytes(*chunk) as u64;
        sum1 = (sum1 + word) % mod_val;
        sum2 = (sum2 + sum1) % mod_val;
    }

    let check1 = mod_val - ((sum1 + sum2) % mod_val);
    let check2 = mod_val - ((sum1 + check1) % mod_val);

    (check2 << 32) | check1
}

/// Verify the Fletcher-64 checksum of an APFS on-disk object block.
///
/// The block must be at least 8 bytes (checksum at offset 0..8, data at 8..).
/// Returns true if the stored checksum matches the computed checksum.
pub fn verify_object(block: &[u8]) -> bool {
    if block.len() < 8 {
        return false;
    }

    let stored = u64::from_le_bytes([
        block[0], block[1], block[2], block[3], block[4], block[5], block[6], block[7],
    ]);

    let computed = fletcher64(&block[8..]);
    stored == computed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fletcher64_known_words() {
        // Hand-computed Fletcher-64 over a small buffer of 8 bytes (two 32-bit LE words).
        // Words: [1, 2] → sum1 = (0+1)%M = 1, sum2 = (0+1)%M = 1
        //                  sum1 = (1+2)%M = 3, sum2 = (1+3)%M = 4
        // check1 = M - ((3+4) % M) = M - 7
        // check2 = M - ((3 + check1) % M) = M - (3 + M - 7) % M = M - (M - 4) % M = M - (M-4) = 4
        let data = [
            1u8, 0, 0, 0, // word 0 = 1
            2, 0, 0, 0, // word 1 = 2
        ];
        let m: u64 = 0xFFFFFFFF;
        let checksum = fletcher64(&data);
        let check1 = checksum & 0xFFFFFFFF;
        let check2 = checksum >> 32;
        assert_eq!(check1, m - 7);
        assert_eq!(check2, 4);
    }

    #[test]
    fn test_verify_object_valid() {
        // Build a 64-byte block: checksum at [0..8], data at [8..64]
        let mut block = vec![0u8; 64];
        // Fill data region with a pattern
        for (i, byte) in block[8..].iter_mut().enumerate() {
            *byte = (i & 0xFF) as u8;
        }
        // Compute and store the checksum
        let checksum = fletcher64(&block[8..]);
        block[..8].copy_from_slice(&checksum.to_le_bytes());
        assert!(verify_object(&block));
    }

    #[test]
    fn test_verify_object_invalid() {
        let mut block = vec![0u8; 64];
        for (i, byte) in block[8..].iter_mut().enumerate() {
            *byte = (i & 0xFF) as u8;
        }
        let checksum = fletcher64(&block[8..]);
        block[..8].copy_from_slice(&checksum.to_le_bytes());
        // Corrupt one byte in the data region
        block[16] ^= 0xFF;
        assert!(!verify_object(&block));
    }
}
