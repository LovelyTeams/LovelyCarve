//! Little-endian field readers. bcachefs is little-endian on disk.
//!
//! Every reader takes a slice and an offset the caller has bounds-checked;
//! they panic on a short slice, so parsers check lengths first.

pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().expect("8 bytes"))
}

pub(crate) fn uuid_at(b: &[u8], o: usize) -> [u8; 16] {
    b[o..o + 16].try_into().expect("16 bytes")
}

/// A buffer of `n` zero bytes, or an error naming `what` when memory will
/// not hold it. `vec![0; n]` answers an allocation failure by aborting the
/// process, which no caller and no C ABI guard can catch, and wherever `n`
/// comes from the disk a crafted image chooses it (#149).
pub(crate) fn zeroed(n: usize, what: impl std::fmt::Display) -> crate::Result<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| {
        crate::Error::Unsupported(format!("{what}: {n} bytes do not fit in memory"))
    })?;
    v.resize(n, 0);
    Ok(v)
}
