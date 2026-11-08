//! Checksums.
//!
//! Two crc32c conventions are in use, both found by computing candidates
//! over reference-formatted images (docs/clean-room.md, S4):
//!
//! * checksum type 1, used for the superblock and btree nodes, is the
//!   standard crc32c: initial value all ones, result inverted;
//! * checksum type 5, used for file data, is crc32c with initial value zero
//!   and the result not inverted.
//!
//! * checksum type 2 is CRC-64 with the ECMA-182 polynomial, most
//!   significant bit first, initial value all ones, result inverted
//!   (found the same way, over the crc64 fixture's superblock);
//! * checksum type 7 is XXH64 with seed 0 (xxhash fixture).
//!
//! The data-side counterparts of types 2 and 7 are checked by the oracle
//! tier against the reference lister's printed checksums.

/// Checksum type 1: standard crc32c.
pub fn crc32c_nonzero(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
}

/// Checksum type 5: crc32c from zero, not inverted.
pub fn crc32c_zero(data: &[u8]) -> u32 {
    // crc32c_append(seed, d) starts from !seed and inverts the result, so a
    // seed of all ones starts from zero and one more inversion undoes it.
    !crc32c::crc32c_append(0xffff_ffff, data)
}

/// Checksum type 2: CRC-64/ECMA-182, MSB first, init and xorout all ones.
pub fn crc64_nonzero(data: &[u8]) -> u64 {
    !crc64_msb(!0, data)
}

/// CRC-64/ECMA-182, MSB first, from zero, not inverted.
pub fn crc64_zero(data: &[u8]) -> u64 {
    crc64_msb(0, data)
}

const CRC64_POLY: u64 = 0x42f0_e1eb_a9ea_3693;

static CRC64_TABLE: std::sync::OnceLock<[u64; 256]> = std::sync::OnceLock::new();

fn crc64_msb(init: u64, data: &[u8]) -> u64 {
    let t = CRC64_TABLE.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = (i as u64) << 56;
            for _ in 0..8 {
                c = if c & (1 << 63) != 0 {
                    (c << 1) ^ CRC64_POLY
                } else {
                    c << 1
                };
            }
            *e = c;
        }
        t
    });
    let mut c = init;
    for &b in data {
        c = t[((c >> 56) as u8 ^ b) as usize] ^ (c << 8);
    }
    c
}

/// Checksum type 7: XXH64, seed 0.
pub fn xxhash64(data: &[u8]) -> u64 {
    twox_hash::XxHash64::oneshot(0, data)
}

/// The checksum of `data` of the given type, for writing one.
pub fn compute(csum_type: u8, data: &[u8]) -> crate::Result<u64> {
    Ok(match csum_type {
        0 => 0,
        1 => crc32c_nonzero(data) as u64,
        2 => crc64_nonzero(data),
        5 => crc32c_zero(data) as u64,
        6 => crc64_zero(data),
        7 => xxhash64(data),
        t => {
            return Err(crate::Error::Unsupported(format!(
                "checksum type {t} cannot be written"
            )))
        }
    })
}

/// Verify `data` against a stored checksum of the given type. Returns the
/// computed value on a mismatch -- and `Err(0)` for a type this reader does
/// not know, which never verifies: a checksum nobody computed is not a
/// checksum that matched. Callers name the type first with [`is_known`];
/// this is the backstop for one that forgets.
pub fn verify(csum_type: u8, data: &[u8], stored: u64) -> Result<(), u64> {
    let computed = match csum_type {
        0 => return Ok(()),
        1 => crc32c_nonzero(data) as u64,
        2 => crc64_nonzero(data),
        5 => crc32c_zero(data) as u64,
        6 => crc64_zero(data),
        7 => xxhash64(data),
        _ => return Err(0),
    };
    if computed == stored {
        Ok(())
    } else {
        Err(computed)
    }
}

/// Whether this reader can verify a checksum type.
pub fn is_known(csum_type: u8) -> bool {
    matches!(csum_type, 0 | 1 | 2 | 5 | 6 | 7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_checksum_type_never_verifies() {
        for t in [3u8, 4, 8, 15, 0xff] {
            assert!(!is_known(t));
            assert!(verify(t, b"anything", 0).is_err(), "type {t} passed");
            assert!(compute(t, b"anything").is_err(), "type {t} computed");
        }
        assert!(verify(0, b"anything", 12345).is_ok());
    }

    #[test]
    fn the_two_crc32c_conventions_differ_as_measured() {
        // "abc" in a zero-padded sector: the data checksum the reference
        // lister printed for that sector was 0xfed26c9d.
        let mut s = [0u8; 512];
        s[..3].copy_from_slice(b"abc");
        assert_eq!(crc32c_zero(&s), 0xfed2_6c9d);
        // The standard check value of crc32c.
        assert_eq!(crc32c_nonzero(b"123456789"), 0xe306_9283);
        // CRC-64/WE's published check value.
        assert_eq!(crc64_nonzero(b"123456789"), 0x62ec_59e3_f1a4_f00a);
        // XXH64's published value for the empty input, seed 0.
        assert_eq!(xxhash64(b""), 0xef46_db37_51d8_e999);
    }
}
