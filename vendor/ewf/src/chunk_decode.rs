//! LovelyCarve compatibility patch: bounded decoding, checksums and EWF2 flags.
use std::io::Read;

use crate::error::{EwfError, Result};
use crate::ewf2::CompressionMethod;
use crate::sections::{adler32, Chunk};
use crate::segment_source::SegmentSource;

pub(crate) fn decode(
    source: &SegmentSource,
    chunk: &Chunk,
    method: CompressionMethod,
    chunk_size: u64,
    required: u64,
) -> Result<Vec<u8>> {
    let flags = chunk.v2_flags();
    let pattern = flags.is_some_and(|value| value & 5 == 5);
    let checksum = flags.map_or(!chunk.compressed(), |value| value & 2 != 0);
    let stored_size = chunk.size();
    // Includes a little headroom for incompressible zlib/bzip2 streams.
    if stored_size > chunk_size.saturating_add(1024 * 1024) || chunk_size > 128 * 1024 * 1024 {
        return Err(EwfError::Parse(
            "EWF chunk exceeds bounded decoder size".into(),
        ));
    }
    let mut stored = vec![0; stored_size as usize];
    let count = source.read_at(&mut stored, chunk.offset())?;
    stored.truncate(count);
    let mut page = if pattern {
        if stored.len() != 8 {
            return Err(EwfError::Parse(
                "EWF2 pattern fill must contain 8 bytes".into(),
            ));
        }
        (0..chunk_size as usize)
            .map(|index| stored[index % 8])
            .collect()
    } else if chunk.compressed() {
        let decoder: Box<dyn Read + '_> = match method {
            CompressionMethod::Zlib => Box::new(flate2::read::ZlibDecoder::new(stored.as_slice())),
            CompressionMethod::Bzip2 => Box::new(bzip2::read::BzDecoder::new(stored.as_slice())),
            CompressionMethod::None => {
                return Err(EwfError::Parse(
                    "compressed EWF chunk without compression method".into(),
                ))
            }
        };
        let mut output = Vec::new();
        decoder
            .take(chunk_size + 1)
            .read_to_end(&mut output)
            .map_err(|error| EwfError::Decompression(error.to_string()))?;
        output
    } else {
        if count != stored_size as usize {
            return Err(EwfError::Parse("truncated uncompressed EWF chunk".into()));
        }
        if checksum {
            let payload_size = stored
                .len()
                .checked_sub(4)
                .ok_or_else(|| EwfError::Parse("missing EWF chunk checksum".into()))?;
            let checksum_bytes: [u8; 4] = stored[payload_size..]
                .try_into()
                .map_err(|_| EwfError::Parse("invalid EWF checksum size".into()))?;
            if adler32(&stored[..payload_size]) != u32::from_le_bytes(checksum_bytes) {
                return Err(EwfError::Parse(
                    "EWF chunk Adler-32 checksum mismatch".into(),
                ));
            }
            stored.truncate(payload_size);
        }
        stored
    };
    if (page.len() as u64) < required || (page.len() as u64) > chunk_size {
        return Err(EwfError::Parse(format!(
            "decoded EWF chunk length {} outside {required}..={chunk_size}",
            page.len()
        )));
    }
    // Only the final partial chunk may be shorter than the nominal chunk size.
    page.resize(chunk_size as usize, 0);
    Ok(page)
}
