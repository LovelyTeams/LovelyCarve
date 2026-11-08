use crate::bytes::{slice, u16_at, u32_at, utf16};
use crate::{corrupt, Result};

/// Decode the user-visible link target, matching refsprogs's node_symlink event.
pub(crate) fn target(value: &[u8]) -> Result<Option<String>> {
    let tag = u32_at(value, 12)?;
    let length = u16_at(value, 16)? as usize;
    let data = slice(value, 20, length)?;
    match tag {
        0xA0000003 | 0xA000000C => {
            let offset = u16_at(data, 4)? as usize;
            let length = u16_at(data, 6)? as usize;
            let base = if tag == 0xA000000C { 12 } else { 8 };
            let offset = offset
                .checked_add(base)
                .ok_or_else(|| corrupt("reparse string offset overflow"))?;
            Ok(Some(utf16(slice(data, offset, length)?)?))
        }
        0xA000001D => {
            let length = data
                .len()
                .checked_sub(4)
                .ok_or_else(|| corrupt("truncated WSL symlink"))?;
            let bytes = slice(data, 4, length)?;
            Ok(Some(String::from_utf8(bytes.to_vec()).map_err(
                |error| corrupt(format!("invalid WSL link target: {error}")),
            )?))
        }
        _ => Ok(None),
    }
}
