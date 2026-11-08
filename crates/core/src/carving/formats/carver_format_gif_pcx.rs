//! Digler-compatible GIF and PCX scanners.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

pub(crate) fn scan_gif<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_vec(13)?;
    if &header[..6] != b"GIF87a" && &header[..6] != b"GIF89a" {
        bail!("invalid GIF signature");
    }
    let width = usize::from(u16::from_le_bytes([header[6], header[7]]));
    let height = usize::from(u16::from_le_bytes([header[8], header[9]]));
    let global_fields = header[10];
    let has_global_color_table = global_fields & 0x80 != 0;
    if has_global_color_table {
        skip_gif_color_table(reader, global_fields)?;
    }
    let mut data_parsed = false;
    loop {
        match reader.read_byte()? {
            0x21 => read_gif_extension(reader)?,
            0x2c => {
                let descriptor = reader.read_vec(9)?;
                let left = usize::from(u16::from_le_bytes([descriptor[0], descriptor[1]]));
                let top = usize::from(u16::from_le_bytes([descriptor[2], descriptor[3]]));
                let frame_width = usize::from(u16::from_le_bytes([descriptor[4], descriptor[5]]));
                let frame_height = usize::from(u16::from_le_bytes([descriptor[6], descriptor[7]]));
                if left + frame_width > width || top + frame_height > height {
                    bail!("GIF frame exceeds screen bounds");
                }
                let fields = descriptor[8];
                if fields & 0x80 != 0 {
                    skip_gif_color_table(reader, fields)?;
                } else if !has_global_color_table {
                    bail!("GIF has no color table");
                }
                let literal_width = reader.read_byte()?;
                if !(2..=8).contains(&literal_width) {
                    bail!("GIF LZW code size is out of range");
                }
                loop {
                    let size = reader.read_byte()?;
                    if size == 0 {
                        break;
                    }
                    reader.discard_exact(u64::from(size))?;
                }
                data_parsed = true;
            }
            0x3b if data_parsed => {
                return Ok(Some(FormatScanResult::sized(reader.bytes_read())));
            }
            0x3b => bail!("GIF is missing image data"),
            other => bail!("unknown GIF block type {other:#x}"),
        }
    }
}

fn skip_gif_color_table<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    fields: u8,
) -> anyhow::Result<()> {
    let colors = 1u64 << (1 + u32::from(fields & 7));
    reader.discard_exact(3 * colors)
}

fn read_gif_extension<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<()> {
    let extension = reader.read_byte()?;
    match extension {
        0xf9 => {
            let control = reader.read_vec(6)?;
            if control[0] != 4 || control[5] != 0 {
                bail!("invalid GIF graphic control extension");
            }
            return Ok(());
        }
        0x01 => reader.discard_exact(13)?,
        0xfe => {}
        0xff => {
            let size = reader.read_byte()?;
            let application = reader.read_vec(usize::from(size))?;
            if application == b"NETSCAPE2.0" {
                let block_size = reader.read_byte()?;
                if block_size == 0 {
                    return Ok(());
                }
                reader.discard_exact(u64::from(block_size))?;
            }
        }
        _ => bail!("unknown GIF extension"),
    }
    loop {
        let size = reader.read_byte()?;
        if size == 0 {
            return Ok(());
        }
        reader.discard_exact(u64::from(size))?;
    }
}

pub(crate) fn scan_pcx<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_vec(128)?;
    let manufacturer = header[0];
    let version = header[1];
    let encoding = header[2];
    let bits_per_pixel = header[3];
    let x_min = le_u16(&header[4..6]);
    let y_min = le_u16(&header[6..8]);
    let x_max = le_u16(&header[8..10]);
    let y_max = le_u16(&header[10..12]);
    let number_of_planes = header[65];
    let bytes_per_line = le_u16(&header[66..68]);
    if manufacturer != 0x0a
        || !matches!(encoding, 0 | 1)
        || !matches!(version, 0 | 2 | 3 | 4 | 5)
        || !matches!(bits_per_pixel, 1 | 2 | 4 | 8)
        || number_of_planes == 0
        || number_of_planes > 4
        || x_max < x_min
        || y_max < y_min
    {
        bail!("invalid PCX header");
    }
    let width = u32::from(x_max) - u32::from(x_min) + 1;
    let height = u32::from(y_max) - u32::from(y_min) + 1;
    let mut minimum_line = (width * u32::from(bits_per_pixel)).div_ceil(8);
    if minimum_line % 2 != 0 {
        minimum_line += 1;
    }
    if u32::from(bytes_per_line) < minimum_line {
        bail!("PCX bytes-per-line is too small");
    }

    let mut total = 128u64;
    if encoding == 0 {
        let expected = u64::from(bytes_per_line) * u64::from(number_of_planes) * u64::from(height);
        if reader.discard_up_to(expected)? != expected {
            bail!("truncated PCX image data");
        }
        total += expected;
    } else {
        for _ in 0..height {
            for _ in 0..number_of_planes {
                total += read_pcx_rle_line(reader, bytes_per_line)?;
            }
        }
    }
    if version == 5 && bits_per_pixel == 8 && reader.remaining() > 0 {
        let marker = reader.read_byte()?;
        total += 1;
        if marker == 0x0c {
            if reader.discard_up_to(256)? != 256 {
                bail!("truncated PCX palette");
            }
            total += 256;
        }
    }
    Ok(Some(FormatScanResult::sized(total)))
}

fn read_pcx_rle_line<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    expected: u16,
) -> anyhow::Result<u64> {
    let mut consumed = 0u64;
    let mut decoded = 0usize;
    while decoded < usize::from(expected) {
        let byte = reader.read_byte()?;
        consumed += 1;
        if byte & 0xc0 == 0xc0 {
            let run = usize::from(byte & 0x3f);
            if run == 0 {
                bail!("invalid PCX RLE run");
            }
            reader.read_byte()?;
            consumed += 1;
            decoded += run;
        } else {
            decoded += 1;
        }
    }
    Ok(consumed)
}

fn le_u16(bytes: &[u8]) -> u16 {
    let mut value = [0u8; 2];
    value.copy_from_slice(bytes);
    u16::from_le_bytes(value)
}
