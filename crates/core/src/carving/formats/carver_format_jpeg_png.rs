//! Digler-compatible JPEG and PNG scanners.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

pub(crate) fn scan_jpeg<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let mut pair = [0u8; 2];
    reader.read_exact(&mut pair)?;
    if pair != [0xff, 0xd8] {
        bail!("missing JPEG SOI marker");
    }
    loop {
        // Entropy-coded data runs until the next 0xFF; the byte after it is
        // the marker (or a 0x00 stuffing byte).
        reader.skip_past_byte(0xff)?;
        let mut marker = reader.read_byte()?;
        if marker == 0 {
            continue;
        }
        while marker == 0xff {
            marker = reader.read_byte()?;
        }
        if marker == 0xd9 {
            return Ok(Some(FormatScanResult::sized(reader.bytes_read())));
        }
        if (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        reader.read_exact(&mut pair)?;
        let segment_length = usize::from(u16::from_be_bytes(pair));
        if segment_length < 2 {
            bail!("short JPEG segment length");
        }
        let payload = (segment_length - 2) as u64;
        let known = matches!(
            marker,
            0xc0 | 0xc1 | 0xc2 | 0xc4 | 0xda | 0xdb | 0xdd | 0xe0 | 0xee
        ) || (0xe0..=0xef).contains(&marker)
            || marker == 0xfe;
        if !known {
            bail!("unknown JPEG marker");
        }
        reader.discard_exact(payload)?;
    }
}

const PNG_HEADER: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub(crate) fn scan_png<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    if &reader.read_array::<8>()? != PNG_HEADER {
        bail!("not a PNG file");
    }
    let mut stage = 0u8;
    while stage != 5 {
        let header = reader.read_array::<8>()?;
        let length = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let chunk_type = &header[4..8];
        let next_stage = match chunk_type {
            b"IHDR" if stage == 0 => Some(1),
            b"PLTE" if stage == 1 => Some(2),
            b"tRNS" => Some(3),
            b"IDAT" if (1..=4).contains(&stage) => Some(4),
            b"IEND" if stage == 4 => Some(5),
            b"IHDR" | b"PLTE" | b"IDAT" | b"IEND" => bail!("invalid PNG chunk order"),
            _ => None,
        };
        if length > i32::MAX as u64 {
            bail!("bad PNG chunk length");
        }
        let mut crc = flate2::Crc::new();
        crc.update(chunk_type);
        reader.consume_with(length, |bytes| crc.update(bytes))?;
        let expected = u32::from_be_bytes(reader.read_array()?);
        if expected != crc.sum() {
            bail!("invalid PNG checksum");
        }
        if let Some(next_stage) = next_stage {
            stage = next_stage;
        }
    }
    Ok(Some(FormatScanResult::sized(reader.bytes_read())))
}
