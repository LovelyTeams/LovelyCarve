//! Digler-compatible RAR scanner.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

pub(crate) fn scan_rar<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let signature = reader.read_vec(8)?;
    if &signature[..7] == b"Rar!\x1a\x07\x00" {
        reader.unread(1)?;
        return scan_rar_15(reader);
    }
    if signature == b"Rar!\x1a\x07\x01\x00" {
        return scan_rar_50(reader);
    }
    bail!("invalid RAR signature");
}

fn scan_rar_15<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let (header_type, flags) = read_rar_15_block(reader)?;
    if header_type != 0x73 || ((flags >> 8) & 0x0080) != 0 {
        bail!("invalid or encrypted RAR 1.5 header");
    }
    loop {
        let (header_type, _) = read_rar_15_block(reader)?;
        if header_type == 0x7b {
            break;
        }
    }
    Ok(Some(FormatScanResult::sized(reader.bytes_read())))
}

fn read_rar_15_block<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<(u8, u16)> {
    let header = reader.read_vec(7)?;
    let header_type = header[2];
    let flags = le_u16(&header[3..5]);
    if !(0x72..=0x7b).contains(&header_type) {
        bail!("invalid RAR 1.5 block type");
    }
    if header_type == 0x7b {
        return Ok((header_type, flags));
    }
    let mut payload_size = u32::from(le_u16(&header[5..7]));
    let mut bytes_read = 7u32;
    if matches!(header_type, 0x74 | 0x75 | 0x7a) && !(header_type == 0x75 && flags & 0x0008 == 0) {
        payload_size = payload_size.wrapping_add(le_u32(&reader.read_vec(4)?));
        bytes_read += 4;
    } else if header_type == 0x78 {
        let recovery = reader.read_vec(8)?;
        let blocks = le_u32(&recovery[..4]);
        let block_size = le_u32(&recovery[4..8]);
        payload_size = payload_size.wrapping_add(blocks.wrapping_mul(block_size));
        bytes_read += 8;
    }
    if payload_size <= bytes_read {
        bail!("invalid RAR 1.5 block size");
    }
    reader.discard_exact(u64::from(payload_size - bytes_read))?;
    Ok((header_type, flags))
}

fn scan_rar_50<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let (header_type, flags) = read_rar_5_block(reader)?;
    if header_type != 1 || ((flags >> 56) & 0x0080) != 0 {
        bail!("invalid or encrypted RAR 5 header");
    }
    loop {
        let (header_type, _) = read_rar_5_block(reader)?;
        if header_type == 5 {
            break;
        }
    }
    Ok(Some(FormatScanResult::sized(reader.bytes_read())))
}

fn read_rar_5_block<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<(u64, u64)> {
    reader.discard_exact(4)?;
    let (header_size, encoded_header_size) = read_rar_varint(reader)?;
    if encoded_header_size > 3 || header_size > 2 * 1024 * 1024 {
        bail!("invalid RAR 5 header size");
    }
    let (header_type, mut bytes_read) = read_rar_varint(reader)?;
    let (flags, flags_length) = read_rar_varint(reader)?;
    bytes_read += flags_length;
    let mut total_size = header_size;
    if flags & 0x0001 != 0 {
        let (_, length) = read_rar_varint(reader)?;
        bytes_read += length;
    }
    if flags & 0x0002 != 0 {
        let (data_size, length) = read_rar_varint(reader)?;
        bytes_read += length;
        total_size = total_size.saturating_add(data_size);
    }
    if total_size <= bytes_read as u64 {
        bail!("invalid RAR 5 block size");
    }
    reader.discard_exact(total_size - bytes_read as u64)?;
    Ok((header_type, flags))
}

fn read_rar_varint<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut length = 0usize;
    loop {
        let byte = reader.read_byte()?;
        if length >= 10 {
            bail!("RAR varint exceeds ten bytes");
        }
        value |= u64::from(byte & 0x7f) << shift;
        length += 1;
        if byte & 0x80 == 0 {
            return Ok((value, length));
        }
        shift += 7;
    }
}

fn le_u16(bytes: &[u8]) -> u16 {
    let mut value = [0u8; 2];
    value.copy_from_slice(bytes);
    u16::from_le_bytes(value)
}

fn le_u32(bytes: &[u8]) -> u32 {
    let mut value = [0u8; 4];
    value.copy_from_slice(bytes);
    u32::from_le_bytes(value)
}
