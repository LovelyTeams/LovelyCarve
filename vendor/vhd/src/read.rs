//! Panic-free big-endian integer readers over a byte slice.
//!
//! Out-of-range reads yield 0 rather than panicking — the Paranoid Gatekeeper
//! posture for parsing attacker-controllable images. Callers still range-check
//! lengths/offsets before trusting a value.

pub(crate) fn be_u32(buf: &[u8], off: usize) -> u32 {
    let mut b = [0u8; 4];
    if let Some(s) = buf.get(off..off + 4) {
        b.copy_from_slice(s);
    }
    u32::from_be_bytes(b)
}

pub(crate) fn be_u64(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    if let Some(s) = buf.get(off..off + 8) {
        b.copy_from_slice(s);
    }
    u64::from_be_bytes(b)
}
