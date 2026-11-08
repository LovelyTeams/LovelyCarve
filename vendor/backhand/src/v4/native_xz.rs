//! Validate XZ framing before constructing a potentially large decoder history.
//! All supported BCJ/Delta chains are length preserving; LZMA2 chunk headers
//! independently give the compressed extent and uncompressed length of a block.
use crate::BackhandError;
use std::io::{Cursor, Read};

const MAX_OUTPUT: usize = 4 * super::squashfs::MAX_BLOCK_SIZE as usize;
const MAX_DICTIONARY: u64 = 8 * 1024 * 1024;

fn invalid() -> BackhandError {
    BackhandError::CorruptedOrInvalidSquashfs
}

fn field(bytes: &[u8], position: usize, length: usize) -> Result<&[u8], BackhandError> {
    bytes
        .get(position..position.checked_add(length).ok_or_else(invalid)?)
        .ok_or_else(invalid)
}

fn crc_record(bytes: &[u8]) -> Result<&[u8], BackhandError> {
    let end = bytes.len().checked_sub(4).ok_or_else(invalid)?;
    let expected = u32::from_le_bytes(field(bytes, end, 4)?.try_into().map_err(|_| invalid())?);
    if crc32fast::hash(&bytes[..end]) != expected {
        return Err(invalid());
    }
    Ok(&bytes[..end])
}

fn integer(bytes: &[u8], position: &mut usize) -> Result<u64, BackhandError> {
    let mut result = 0u64;
    for index in 0..9 {
        let byte = *bytes.get(*position).ok_or_else(invalid)?;
        *position += 1;
        result |= u64::from(byte & 127) << (7 * index);
        if byte & 128 == 0 {
            if index > 0 && byte == 0 {
                return Err(invalid());
            }
            return Ok(result);
        }
    }
    Err(invalid())
}

fn block_header(header: &[u8], compressed: usize, uncompressed: u64) -> Result<(), BackhandError> {
    let body = crc_record(header)?;
    let flags = *body.get(1).ok_or_else(invalid)?;
    if flags & 0x3c != 0 {
        return Err(invalid());
    }
    let mut position = 2;
    if flags & 0x40 != 0 && integer(body, &mut position)? != compressed as u64 {
        return Err(invalid());
    }
    if flags & 0x80 != 0 && integer(body, &mut position)? != uncompressed {
        return Err(invalid());
    }
    let count = (flags & 3) + 1;
    for index in 0..count {
        let identifier = integer(body, &mut position)?;
        let length = usize::try_from(integer(body, &mut position)?).map_err(|_| invalid())?;
        let properties = field(body, position, length)?;
        position += length;
        if index + 1 == count {
            if identifier != 0x21 || properties.len() != 1 || properties[0] > 40 {
                return Err(invalid());
            }
            let property = properties[0];
            let dictionary = if property == 40 {
                u64::from(u32::MAX)
            } else {
                u64::from(2 | (property & 1)) << (property / 2 + 11)
            };
            if dictionary > MAX_DICTIONARY {
                return Err(invalid());
            }
        } else if !matches!(identifier, 3..=11) {
            return Err(invalid());
        }
    }
    if body[position..].iter().any(|byte| *byte != 0) {
        return Err(invalid());
    }
    Ok(())
}

fn lzma2_length(bytes: &[u8]) -> Result<u64, BackhandError> {
    let mut position = 0usize;
    let mut output = 0u64;
    loop {
        let control = *bytes.get(position).ok_or_else(invalid)?;
        position += 1;
        if control == 0 {
            return if position == bytes.len() {
                Ok(output)
            } else {
                Err(invalid())
            };
        }
        let sizes = field(bytes, position, if control >= 128 { 4 } else { 2 })?;
        if !matches!(control, 1 | 2 | 128..=255) {
            return Err(invalid());
        }
        let uncompressed = u64::from(u16::from_be_bytes([sizes[0], sizes[1]]))
            + 1
            + if control >= 128 {
                u64::from(control & 31) << 16
            } else {
                0
            };
        let compressed = if control >= 128 {
            usize::from(u16::from_be_bytes([sizes[2], sizes[3]])) + 1
        } else {
            uncompressed as usize
        };
        position += sizes.len() + usize::from(control >= 192);
        field(bytes, position, compressed)?;
        position += compressed;
        output = output.checked_add(uncompressed).ok_or_else(invalid)?;
        if output > MAX_OUTPUT as u64 {
            return Err(invalid());
        }
    }
}

fn validated_length(bytes: &[u8]) -> Result<usize, BackhandError> {
    let header = field(bytes, 0, 12)?;
    if &header[..6] != b"\xfd7zXZ\0" || header[6] != 0 {
        return Err(invalid());
    }
    crc_record(&header[6..])?;
    let check_length = match header[7] {
        0 => 0,
        1 => 4,
        4 => 8,
        10 => 32,
        _ => return Err(invalid()),
    };
    let footer_start = bytes.len().checked_sub(12).ok_or_else(invalid)?;
    let footer = field(bytes, footer_start, 12)?;
    if footer[8..10] != header[6..8]
        || &footer[10..] != b"YZ"
        || crc32fast::hash(&footer[4..10])
            != u32::from_le_bytes(footer[..4].try_into().map_err(|_| invalid())?)
    {
        return Err(invalid());
    }
    let index_size = (u64::from(u32::from_le_bytes(
        footer[4..8].try_into().map_err(|_| invalid())?,
    )) + 1)
        * 4;
    let index_start = footer_start
        .checked_sub(usize::try_from(index_size).map_err(|_| invalid())?)
        .ok_or_else(invalid)?;
    if index_start < 12 {
        return Err(invalid());
    }
    let index = crc_record(&bytes[index_start..footer_start])?;
    if index.first() != Some(&0) {
        return Err(invalid());
    }
    let mut position = 1;
    let count = integer(index, &mut position)?;
    if count > (index.len() / 2) as u64 {
        return Err(invalid());
    }
    let mut block_start = 12usize;
    let mut total = 0u64;
    for _ in 0..count {
        let unpadded = usize::try_from(integer(index, &mut position)?).map_err(|_| invalid())?;
        let uncompressed = integer(index, &mut position)?;
        let block_length = unpadded.checked_add(3).ok_or_else(invalid)? & !3;
        let block = field(&bytes[..index_start], block_start, block_length)?;
        let header_size = (usize::from(*block.first().ok_or_else(invalid)?) + 1) * 4;
        let compressed = unpadded
            .checked_sub(header_size + check_length)
            .ok_or_else(invalid)?;
        block_header(field(block, 0, header_size)?, compressed, uncompressed)?;
        let body = field(block, header_size, compressed)?;
        if lzma2_length(body)? != uncompressed {
            return Err(invalid());
        }
        let padding = (4 - compressed % 4) % 4;
        if field(block, header_size + compressed, padding)?
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(invalid());
        }
        block_start += block_length;
        total = total.checked_add(uncompressed).ok_or_else(invalid)?;
        if total > MAX_OUTPUT as u64 {
            return Err(invalid());
        }
    }
    if block_start != index_start
        || index[position..].len() > 3
        || index[position..].iter().any(|byte| *byte != 0)
    {
        return Err(invalid());
    }
    Ok(total as usize)
}

#[inline(never)]
pub(super) fn decode(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), BackhandError> {
    let expected = validated_length(bytes)?;
    let mut reader = lzma_rust2::XzReader::new(Cursor::new(bytes), false);
    let mut decoded = Vec::new();
    (&mut reader)
        .take(expected as u64 + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() != expected || reader.into_inner().position() != bytes.len() as u64 {
        return Err(invalid());
    }
    out.extend_from_slice(&decoded);
    Ok(())
}
