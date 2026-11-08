//! Bounds-checked little-endian readers over untrusted volume bytes.
//!
//! Every multi-byte read goes through these helpers: an out-of-range offset
//! yields a zero value or all-zero bytes rather than panicking or reading out of
//! bounds. This is the Paranoid-Gatekeeper front door — the reader parses
//! attacker-controllable BitLocker volumes and must never trust a length or
//! offset. BitLocker structures are little-endian throughout.

/// Read a little-endian `u16` at `off`, yielding 0 when out of range.
pub(crate) fn le_u16(b: &[u8], off: usize) -> u16 {
    let mut a = [0u8; 2];
    if let Some(s) = b.get(off..off.saturating_add(2)) {
        a.copy_from_slice(s);
    }
    u16::from_le_bytes(a)
}

/// Read a little-endian `u32` at `off`, yielding 0 when out of range.
pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(off..off.saturating_add(4)) {
        a.copy_from_slice(s);
    }
    u32::from_le_bytes(a)
}

/// Read a little-endian `u64` at `off`, yielding 0 when out of range.
pub(crate) fn le_u64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    if let Some(s) = b.get(off..off.saturating_add(8)) {
        a.copy_from_slice(s);
    }
    u64::from_le_bytes(a)
}

/// Read a 16-byte GUID at `off`, yielding all-zero bytes when out of range.
pub(crate) fn read_guid(b: &[u8], off: usize) -> [u8; 16] {
    let mut g = [0u8; 16];
    if let Some(s) = b.get(off..off.saturating_add(16)) {
        g.copy_from_slice(s);
    }
    g
}

/// Copy an owned byte range `[off, off+len)`, truncated to what is present.
/// Never panics: an out-of-range start yields an empty vector.
pub(crate) fn slice_owned(b: &[u8], off: usize, len: usize) -> Vec<u8> {
    let end = off.saturating_add(len).min(b.len());
    b.get(off..end).map(<[u8]>::to_vec).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_yield_zero_out_of_range() {
        let b = [1u8, 2, 3];
        assert_eq!(le_u16(&b, 0), 0x0201);
        assert_eq!(le_u16(&b, 10), 0);
        assert_eq!(le_u32(&b, 0), 0); // only 3 bytes -> out of range -> 0
        assert_eq!(le_u64(&b, 0), 0);
        assert_eq!(read_guid(&b, 0), [0u8; 16]);
    }

    #[test]
    fn le_u32_and_u64_in_range() {
        let b = [1u8, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(le_u32(&b, 0), 1);
        assert_eq!(le_u64(&b, 0), 1);
    }

    #[test]
    fn read_guid_in_range() {
        let mut b = [0u8; 16];
        b[0] = 0xAA;
        assert_eq!(read_guid(&b, 0)[0], 0xAA);
    }

    #[test]
    fn slice_owned_truncates() {
        let b = [1u8, 2, 3, 4];
        assert_eq!(slice_owned(&b, 1, 2), vec![2, 3]);
        assert_eq!(slice_owned(&b, 2, 100), vec![3, 4]);
        assert!(slice_owned(&b, 100, 4).is_empty());
    }
}
