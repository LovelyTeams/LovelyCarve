//! Digler-compatible MP3, WAV, AU, and WMA scanners.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

pub(crate) fn scan_mp3<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let mut size = skip_id3(reader)?;
    let mut frames = 0usize;
    while reader.remaining() >= 4 {
        let header = reader.read_vec(4)?;
        let Some(frame_size) = parse_mp3_frame_size(&header) else {
            break;
        };
        if !(100..=1500).contains(&frame_size) {
            bail!("invalid MP3 frame size");
        }
        reader.discard_exact((frame_size - 4) as u64)?;
        size += frame_size as u64;
        frames += 1;
    }
    if frames < 2 {
        bail!("MP3 stream contains fewer than two frames");
    }
    Ok(Some(FormatScanResult::sized(size)))
}

fn skip_id3<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<u64> {
    let header = reader.peek_vec(10)?;
    if &header[..3] != b"ID3" {
        return Ok(0);
    }
    let tag_size = (u32::from(header[6] & 0x7f) << 21)
        | (u32::from(header[7] & 0x7f) << 14)
        | (u32::from(header[8] & 0x7f) << 7)
        | u32::from(header[9] & 0x7f);
    let total = 10 + u64::from(tag_size);
    reader.discard_exact(total)?;
    Ok(total)
}

fn parse_mp3_frame_size(bytes: &[u8]) -> Option<usize> {
    let header = u32::from_be_bytes(bytes.try_into().ok()?);
    if header & 0xffe0_0000 != 0xffe0_0000 {
        return None;
    }
    let version_bits = ((header >> 19) & 3) as usize;
    let version = match version_bits {
        3 => 1,
        2 => 2,
        0 => 25,
        _ => return None,
    };
    if (header >> 17) & 3 != 1 {
        return None;
    }
    let bitrate_index = ((header >> 12) & 0x0f) as usize;
    if bitrate_index == 0 || bitrate_index == 15 {
        return None;
    }
    const MPEG1: [usize; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const MPEG2: [usize; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    const SAMPLE_RATES: [[usize; 4]; 4] = [
        [11_025, 12_000, 8_000, 0],
        [0, 0, 0, 0],
        [22_050, 24_000, 16_000, 0],
        [44_100, 48_000, 32_000, 0],
    ];
    let sample_index = ((header >> 10) & 3) as usize;
    let sample_rate = SAMPLE_RATES[version_bits][sample_index];
    if sample_index == 3 || sample_rate == 0 {
        return None;
    }
    let bitrate = if version == 1 { MPEG1 } else { MPEG2 }[bitrate_index];
    let padding = usize::from((header >> 9) & 1 != 0);
    let frame_size = (1152 * bitrate * 1000 / sample_rate) / 8 + padding;
    (frame_size > 4).then_some(frame_size)
}

pub(crate) fn scan_au<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_vec(24)?;
    if u32::from_be_bytes(header[..4].try_into()?) != 0x2e73_6e64 {
        bail!("invalid AU signature");
    }
    let header_size = u32::from_be_bytes(header[4..8].try_into()?);
    let data_size = u32::from_be_bytes(header[8..12].try_into()?);
    if header_size < 24 || data_size == u32::MAX {
        bail!("invalid or unknown AU size");
    }
    let mut bytes_read = 24u64;
    if header_size > 24 {
        let wanted = u64::from(header_size - 24);
        if reader.discard_up_to(wanted)? != wanted {
            bail!("truncated AU header");
        }
        bytes_read += wanted;
    }
    let total = u64::from(header_size) + u64::from(data_size);
    let remaining = total.saturating_sub(bytes_read);
    let discarded = reader.discard_up_to(remaining)?;
    if discarded < remaining {
        return Ok(Some(FormatScanResult::sized(bytes_read + discarded)));
    }
    Ok(Some(FormatScanResult::sized(total)))
}

/// RIFF / RIFX / RF64 WAVE files. The container length (which counts the pad
/// byte of an odd-sized chunk and any chunks after the audio) is the file size
/// when the chunks after `data` walk cleanly up to it; otherwise the file ends
/// where they stop. Sizes past the carve bound are cut at the bound.
pub(crate) fn scan_wav<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_array::<12>()?;
    let big_endian = match &header[..4] {
        b"RIFF" | b"RF64" => false,
        b"RIFX" => true,
        _ => bail!("invalid WAV RIFF signature"),
    };
    if &header[8..12] != b"WAVE" {
        bail!("missing WAVE identifier");
    }
    let rf64 = &header[..4] == b"RF64";
    let mut declared_end = u64::from(riff_u32(&header[4..8], big_endian)) + 8;
    let mut rf64_data_size = None;
    let mut found_format = false;
    let (data_end, data_pad) = loop {
        // Streaming writers may leave a zero length in the header; otherwise
        // the audio must start inside the declared container.
        if declared_end > 20 && reader.bytes_read() + 8 > declared_end {
            bail!("missing WAV data chunk");
        }
        let chunk = reader.read_array::<8>()?;
        if !is_fourcc(&chunk[..4]) {
            bail!("invalid RIFF chunk id");
        }
        let mut size = u64::from(riff_u32(&chunk[4..8], big_endian));
        match &chunk[..4] {
            b"ds64" if rf64 && size >= 16 => {
                let sizes = reader.read_array::<16>()?;
                declared_end = u64::from_le_bytes(sizes[..8].try_into()?).saturating_add(8);
                rf64_data_size = Some(u64::from_le_bytes(sizes[8..].try_into()?));
                size -= 16;
            }
            b"fmt " if size < 16 => bail!("WAV fmt chunk is too short"),
            b"fmt " => found_format = true,
            b"data" if !found_format => bail!("WAV data chunk precedes fmt"),
            b"data" => {
                if size == u64::from(u32::MAX) {
                    size = rf64_data_size.unwrap_or(size);
                }
                break (reader.bytes_read().saturating_add(size), size & 1);
            }
            _ => {}
        }
        reader.discard_exact(size + (size & 1))?;
    };
    let bound = reader.remaining_from(0);
    if data_end >= bound {
        return Ok(Some(FormatScanResult::sized(bound)));
    }
    if declared_end < data_end {
        return Ok(Some(FormatScanResult::sized(data_end)));
    }
    // Chunks after the audio (LIST/INFO, id3, cue...) up to the declared end.
    let mut position = data_end + data_pad;
    let end = declared_end.min(bound);
    while position + 8 <= end {
        let mut chunk = [0u8; 8];
        reader.peek_at(position, &mut chunk)?;
        let size = u64::from(riff_u32(&chunk[4..8], big_endian));
        let next = position + 8 + size + (size & 1);
        if !is_fourcc(&chunk[..4]) || next > end {
            break;
        }
        position = next;
    }
    Ok(Some(FormatScanResult::sized(if position >= end {
        end
    } else {
        data_end
    })))
}

/// RIFF chunk ids are four printable ASCII characters.
fn is_fourcc(id: &[u8]) -> bool {
    id.iter().all(|byte| (0x20..0x7f).contains(byte))
}

fn riff_u32(bytes: &[u8], big_endian: bool) -> u32 {
    let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if big_endian {
        u32::from_be_bytes(bytes)
    } else {
        u32::from_le_bytes(bytes)
    }
}

const ASF_HEADER: &[u8; 16] = b"\x30\x26\xb2\x75\x8e\x66\xcf\x11\xa6\xd9\x00\xaa\x00\x62\xce\x6c";
const ASF_FILE_PROPERTIES: &[u8; 16] =
    b"\xa1\xdc\xab\x8c\x47\xa9\xcf\x11\x8e\xe4\x00\xc0\x0c\x20\x53\x65";
const ASF_STREAM_PROPERTIES: &[u8; 16] =
    b"\x91\x07\xdc\xb7\xb7\xa9\xcf\x11\x8e\xe6\x00\xc0\x0c\x20\x53\x65";
const WMA_STREAM: &[u8; 16] = b"\x40\x9e\x69\xf8\x4d\x5b\xcf\x11\xa8\xfd\x00\x80\x5f\x5c\x44\x2b";

pub(crate) fn scan_wma<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_vec(30)?;
    if &header[..16] != ASF_HEADER {
        bail!("invalid ASF header");
    }
    let header_size = u64::from_le_bytes(header[16..24].try_into()?);
    let object_count = u32::from_le_bytes(header[24..28].try_into()?);
    if header_size < 30 || object_count < 4 {
        bail!("invalid ASF header metadata");
    }
    let mut bytes_read = 30u64;
    let mut total_file_size = 0u64;
    let mut wma_stream_found = false;
    for _ in 0..object_count {
        if bytes_read + 24 > header_size {
            bail!("ASF object exceeds header");
        }
        let object = reader.read_vec(24)?;
        let object_id = &object[..16];
        let object_size = u64::from_le_bytes(object[16..24].try_into()?);
        if !(24..=2_000 * 1024 * 1024).contains(&object_size)
            || bytes_read + object_size > header_size
        {
            bail!("invalid ASF object size");
        }
        if object_id == ASF_FILE_PROPERTIES {
            if object_size < 48 {
                bail!("invalid ASF file properties object");
            }
            let payload = reader.peek_vec(24)?;
            total_file_size = u64::from_le_bytes(payload[16..24].try_into()?);
            if total_file_size < header_size {
                bail!("invalid ASF total file size");
            }
        } else if object_id == ASF_STREAM_PROPERTIES {
            if object_size < 40 {
                bail!("invalid ASF stream properties object");
            }
            wma_stream_found = reader.peek_vec(16)?.as_slice() == WMA_STREAM;
        }
        reader.discard_exact(object_size - 24)?;
        bytes_read += object_size;
    }
    if total_file_size == 0 || total_file_size < bytes_read || !wma_stream_found {
        bail!("incomplete WMA structure");
    }
    Ok(Some(FormatScanResult::sized(total_file_size)))
}

#[cfg(test)]
#[path = "carver_format_audio_tests.rs"]
mod tests;
