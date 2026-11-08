//! Microsoft mixed-endian GUID formatting.
//!
//! A Windows GUID stores its first three fields little-endian and its final
//! eight bytes big-endian. Formatting is pure string work (no crypto), so it is
//! hand-written here rather than pulling in a dependency — it renders identically
//! to the canonical `8-4-4-4-12` form `libbde`/`pybde` print.

use std::fmt::Write;

use crate::bytes::{le_u16, le_u32};

/// Render a 16-byte Microsoft GUID in canonical `8-4-4-4-12` lowercase form.
#[must_use]
pub fn format_guid(raw: &[u8; 16]) -> String {
    let d1 = le_u32(raw, 0);
    let d2 = le_u16(raw, 4);
    let d3 = le_u16(raw, 6);
    let mut tail = String::with_capacity(20);
    for (i, b) in raw[8..16].iter().enumerate() {
        if i == 2 {
            tail.push('-');
        }
        // `write!` to a String is infallible.
        let _ = write!(tail, "{b:02x}");
    }
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_mixed_endian_guid() {
        // On-disk bytes of the BitLocker To Go identifier as seen in bdetogo.raw
        // (offset 424): 3bd66749 292e d84a 8399 f6a339e3d001.
        let raw = [
            0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3,
            0xd0, 0x01,
        ];
        assert_eq!(format_guid(&raw), "4967d63b-2e29-4ad8-8399-f6a339e3d001");
    }

    #[test]
    fn formats_zero_guid() {
        assert_eq!(
            format_guid(&[0u8; 16]),
            "00000000-0000-0000-0000-000000000000"
        );
    }
}
