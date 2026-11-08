use crate::{corrupt, Result};

pub(crate) fn slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| corrupt("byte range overflow"))?;
    bytes.get(offset..end).ok_or_else(|| {
        corrupt(format!(
            "truncated structure: range {offset}..{end}, length {}",
            bytes.len()
        ))
    })
}

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = slice(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = slice(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = slice(bytes, offset, 8)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

pub(crate) fn utf16(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(corrupt("odd UTF-16 filename length"));
    }
    let words = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&words).map_err(|error| corrupt(format!("invalid UTF-16 filename: {error}")))
}
