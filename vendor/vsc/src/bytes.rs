//! Bounds-checked little-endian readers over untrusted volume bytes.
//!
//! Every multi-byte read goes through these helpers: an out-of-range offset
//! yields a zero value or an empty string rather than panicking or reading out
//! of bounds. This is the Paranoid-Gatekeeper front door — the reader parses
//! attacker-controllable disk images and must never trust a length or offset.

/// Read a little-endian `u16` at `off`, yielding 0 when out of range.
pub(crate) fn le_u16(b: &[u8], off: usize) -> u16 {
    let mut a = [0u8; 2];
    if let Some(s) = b.get(off..off + 2) {
        a.copy_from_slice(s);
    }
    u16::from_le_bytes(a)
}

/// Read a little-endian `u32` at `off`, yielding 0 when out of range.
pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(off..off + 4) {
        a.copy_from_slice(s);
    }
    u32::from_le_bytes(a)
}

/// Read a little-endian `u64` at `off`, yielding 0 when out of range.
pub(crate) fn le_u64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    if let Some(s) = b.get(off..off + 8) {
        a.copy_from_slice(s);
    }
    u64::from_le_bytes(a)
}

/// Read a 16-byte GUID at `off`, yielding all-zero bytes when out of range.
pub(crate) fn read_guid(b: &[u8], off: usize) -> [u8; 16] {
    let mut g = [0u8; 16];
    if let Some(s) = b.get(off..off + 16) {
        g.copy_from_slice(s);
    }
    g
}

/// Decode a UTF-16LE byte run into a `String`, substituting U+FFFD for any
/// unpaired surrogate. A trailing odd byte is ignored.
pub(crate) fn utf16le_string(b: &[u8]) -> String {
    let units = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair));
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}
