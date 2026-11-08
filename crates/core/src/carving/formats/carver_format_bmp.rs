//! Digler-compatible BMP scanner.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

pub(crate) fn scan_bmp<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let file = reader.read_vec(14)?;
    if &file[..2] != b"BM" {
        bail!("invalid BMP signature");
    }
    let file_size = le_u32(&file[2..6]);
    let reserved_one = u16::from_le_bytes(file[6..8].try_into()?);
    let reserved_two = u16::from_le_bytes(file[8..10].try_into()?);
    let data_offset = le_u32(&file[10..14]);
    if reserved_one != 0 || reserved_two != 0 || file_size < 54 || data_offset < 14 {
        bail!("invalid BMP file header");
    }

    let header_size_bytes = reader.read_vec(4)?;
    let header_size = le_u32(&header_size_bytes);
    if !matches!(header_size, 12 | 40 | 64 | 108 | 124) {
        bail!("unsupported BMP DIB header");
    }
    let mut dib = header_size_bytes;
    dib.extend_from_slice(&reader.read_vec(header_size.saturating_sub(4) as usize)?);
    if dib.len() < 40 {
        bail!("incomplete BMP DIB header");
    }
    let width = i32::from_le_bytes(dib[4..8].try_into()?);
    let height = i32::from_le_bytes(dib[8..12].try_into()?);
    let planes = u16::from_le_bytes(dib[12..14].try_into()?);
    let bits_per_pixel = u16::from_le_bytes(dib[14..16].try_into()?);
    let compression = le_u32(&dib[16..20]);
    let image_size = le_u32(&dib[20..24]);
    let colors_used = le_u32(&dib[32..36]);
    if planes != 1
        || !matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32)
        || !matches!(compression, 0..=6 | 11..=13)
        || width <= 0
        || height == 0
    {
        bail!("invalid BMP DIB metadata");
    }

    let mut expected_min_data_offset = 14u32.wrapping_add(header_size);
    if bits_per_pixel <= 8 {
        let colors = if colors_used == 0 {
            1u32.wrapping_shl(u32::from(bits_per_pixel))
        } else {
            colors_used
        };
        expected_min_data_offset = expected_min_data_offset.wrapping_add(colors.wrapping_mul(4));
    }
    if data_offset < expected_min_data_offset {
        bail!("BMP pixel offset precedes palette");
    }

    let mut bytes_per_pixel = u32::from(bits_per_pixel / 8);
    if bits_per_pixel % 8 != 0 {
        bytes_per_pixel = 1;
    }
    let mut row_size = (width as u32).wrapping_mul(bytes_per_pixel);
    if bits_per_pixel == 1 {
        row_size = (width as u32).wrapping_add(7) / 8;
    } else if bits_per_pixel == 4 {
        row_size = (width as u32).wrapping_add(1) / 2;
    }
    let padded_row_size = row_size.wrapping_add(3) & !3;
    let expected_image_size = padded_row_size.wrapping_mul(height.unsigned_abs());
    if image_size != 0 && image_size < expected_image_size {
        bail!("BMP declared image size is too small");
    }
    let actual_image_size = if compression == 0 {
        expected_image_size
    } else {
        image_size
    };
    let expected_total_size = data_offset.wrapping_add(actual_image_size);
    if file_size < expected_total_size {
        bail!("BMP file size is inconsistent");
    }
    Ok(Some(FormatScanResult::sized(u64::from(file_size))))
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap_or_default())
}
