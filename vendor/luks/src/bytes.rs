//! Bounds-checked big-endian readers. LUKS stores all multi-byte integers
//! big-endian; every read yields 0 out of range (never panics) so a truncated
//! or lying header degrades gracefully rather than crashing.

/// Read a big-endian `u16` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u16(data: &[u8], off: usize) -> u16 {
    let mut b = [0u8; 2];
    if let Some(s) = data.get(off..off + 2) {
        b.copy_from_slice(s);
    }
    u16::from_be_bytes(b)
}

/// Read a big-endian `u32` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u32(data: &[u8], off: usize) -> u32 {
    let mut b = [0u8; 4];
    if let Some(s) = data.get(off..off + 4) {
        b.copy_from_slice(s);
    }
    u32::from_be_bytes(b)
}

/// Read a big-endian `u64` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u64(data: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    if let Some(s) = data.get(off..off + 8) {
        b.copy_from_slice(s);
    }
    u64::from_be_bytes(b)
}

/// Read a fixed-size byte array at `off`, zero-filled if out of range.
#[must_use]
pub fn bytes_n<const N: usize>(data: &[u8], off: usize) -> [u8; N] {
    let mut b = [0u8; N];
    if let Some(s) = data.get(off..off + N) {
        b.copy_from_slice(s);
    }
    b
}

/// Read a null-terminated ASCII string from a fixed `len`-byte field at `off`.
/// Trailing NULs and anything after the first NUL are dropped; invalid UTF-8 is
/// replaced. Returns an empty string if the field is out of range.
#[must_use]
pub fn cstr(data: &[u8], off: usize, len: usize) -> String {
    let field = data.get(off..off + len).unwrap_or(&[]);
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn be_readers_bounds_check() {
        let d = [0x00, 0x01, 0x00, 0x00, 0x00, 0x2a];
        assert_eq!(be_u16(&d, 0), 1);
        assert_eq!(be_u32(&d, 2), 42);
        // out of range -> 0, no panic
        assert_eq!(be_u16(&d, 100), 0);
        assert_eq!(be_u32(&d, 100), 0);
    }

    #[test]
    fn bytes_n_and_cstr() {
        let d = b"aes\0\0\0\0\0rest";
        assert_eq!(cstr(d, 0, 8), "aes");
        assert_eq!(bytes_n::<3>(d, 0), *b"aes");
        // out of range -> zero-filled / empty
        assert_eq!(bytes_n::<4>(d, 100), [0u8; 4]);
        assert_eq!(cstr(d, 100, 8), "");
    }
}
