//! Bounds-checked little-endian integer/array readers.
//!
//! These never panic: an out-of-range offset yields `0` (or an all-zero array)
//! rather than indexing past the end of the slice. Callers already range-check
//! before acting on the decoded values, so a zero on truncated input is a safe,
//! non-matching sentinel rather than silently wrong behaviour.

/// Read a little-endian `u16` at offset `o`; returns `0` if out of range.
#[inline]
pub(crate) fn le_u16(d: &[u8], o: usize) -> u16 {
    let mut b = [0u8; 2];
    if let Some(s) = d.get(o..o + 2) {
        b.copy_from_slice(s);
    }
    u16::from_le_bytes(b)
}

/// Read a little-endian `u32` at offset `o`; returns `0` if out of range.
#[inline]
pub(crate) fn le_u32(d: &[u8], o: usize) -> u32 {
    let mut b = [0u8; 4];
    if let Some(s) = d.get(o..o + 4) {
        b.copy_from_slice(s);
    }
    u32::from_le_bytes(b)
}

/// Read a little-endian `u64` at offset `o`; returns `0` if out of range.
#[inline]
pub(crate) fn le_u64(d: &[u8], o: usize) -> u64 {
    let mut b = [0u8; 8];
    if let Some(s) = d.get(o..o + 8) {
        b.copy_from_slice(s);
    }
    u64::from_le_bytes(b)
}

/// Copy 16 bytes at offset `o` into a GUID array; zero-filled if out of range.
#[inline]
pub(crate) fn le_arr16(d: &[u8], o: usize) -> [u8; 16] {
    let mut b = [0u8; 16];
    if let Some(s) = d.get(o..o + 16) {
        b.copy_from_slice(s);
    }
    b
}
