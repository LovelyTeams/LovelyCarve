//! Little-endian decode helpers — one home for the byte-parsing patterns that
//! otherwise repeat across the format readers.

/// Little-endian `u32` from the first 4 bytes of `b`; `0` when `b` is shorter.
///
/// Adapts [`safe_read::le_u32`]'s `(bytes, offset)` shape to the `Fn(&[u8]) -> u32`
/// that [`le_u32_table`]'s `map` needs, so the read stays bounds-checked (ADR-0012)
/// rather than slicing blind.
#[inline]
pub(crate) fn le_u32(b: &[u8]) -> u32 {
    safe_read::le_u32(b, 0)
}

/// Decode a packed table of little-endian `u32` entries (grain directory / table).
#[inline]
pub(crate) fn le_u32_table(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(le_u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_u32_short_slice_reads_zero_instead_of_panicking() {
        assert_eq!(le_u32(&[1, 2, 3]), 0);
    }

    #[test]
    fn le_u32_table_decodes_whole_entries_only() {
        assert_eq!(le_u32_table(&[1, 0, 0, 0, 2, 0, 0, 0, 0xff]), vec![1, 2]);
    }
}
