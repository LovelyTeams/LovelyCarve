//! Bounds-checked integer/byte readers over attacker-controllable image data.
//!
//! Every accessor returns a value (0 / empty on out-of-range) rather than
//! panicking or indexing out of bounds — a length or offset field taken from
//! the image can be arbitrary, so reads are validated against the slice length
//! first (Paranoid Gatekeeper).

/// Reads a little-endian `u16` at `off`, or `0` if the 2 bytes are out of range.
#[must_use]
pub fn le_u16(data: &[u8], off: usize) -> u16 {
    match off.checked_add(2).and_then(|end| data.get(off..end)) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}

/// Reads a little-endian `u32` at `off`, or `0` if the 4 bytes are out of range.
#[must_use]
pub fn le_u32(data: &[u8], off: usize) -> u32 {
    match off.checked_add(4).and_then(|end| data.get(off..end)) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

/// Reads a little-endian `u64` at `off`, or `0` if the 8 bytes are out of range.
#[must_use]
pub fn le_u64(data: &[u8], off: usize) -> u64 {
    match off.checked_add(8).and_then(|end| data.get(off..end)) {
        Some(s) => u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
        None => 0,
    }
}

/// Reads a little-endian 6-byte block number at `off` (widened to `u64`), or
/// `0` if out of range. CoreStorage stores physical-volume block numbers as
/// 48-bit little-endian values.
#[must_use]
pub fn le_u48(data: &[u8], off: usize) -> u64 {
    match off.checked_add(6).and_then(|end| data.get(off..end)) {
        Some(s) => {
            u64::from(s[0])
                | (u64::from(s[1]) << 8)
                | (u64::from(s[2]) << 16)
                | (u64::from(s[3]) << 24)
                | (u64::from(s[4]) << 32)
                | (u64::from(s[5]) << 40)
        }
        None => 0,
    }
}

/// Copies a fixed 16-byte array (a UUID / key half) at `off`, zero-filled if the
/// range is out of bounds.
#[must_use]
pub fn bytes16(data: &[u8], off: usize) -> [u8; 16] {
    let mut out = [0u8; 16];
    if let Some(s) = off.checked_add(16).and_then(|end| data.get(off..end)) {
        out.copy_from_slice(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_readers_decode_in_range() {
        let d = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(le_u16(&d, 0), 0x0201);
        assert_eq!(le_u32(&d, 0), 0x0403_0201);
        assert_eq!(le_u64(&d, 0), 0x0807_0605_0403_0201);
        assert_eq!(le_u48(&d, 0), 0x0605_0403_0201);
    }

    #[test]
    fn le_readers_return_zero_out_of_range() {
        let d = [0xffu8; 3];
        assert_eq!(le_u16(&d, 2), 0);
        assert_eq!(le_u32(&d, 0), 0);
        assert_eq!(le_u64(&d, 0), 0);
        assert_eq!(le_u48(&d, 0), 0);
        assert_eq!(le_u16(&[], 0), 0);
    }

    #[test]
    fn bytes16_copies_or_zero_fills() {
        let mut d = [0u8; 20];
        for (i, b) in d.iter_mut().enumerate() {
            *b = i as u8;
        }
        assert_eq!(bytes16(&d, 0)[15], 15);
        assert_eq!(bytes16(&d, 4)[0], 4);
        // Out of range -> zeroed.
        assert_eq!(bytes16(&d, 10), [0u8; 16]);
        assert_eq!(bytes16(&[], 0), [0u8; 16]);
    }
}
