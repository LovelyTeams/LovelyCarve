//! Bounded native Rust block codecs. Publish bytes only after complete validation.

use crate::BackhandError;

#[cfg(feature = "zstd")]
const MAX_OUTPUT: usize = 4 * super::squashfs::MAX_BLOCK_SIZE as usize;

#[cfg(feature = "xz")]
pub(super) fn decode_xz(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), BackhandError> {
    super::native_xz::decode(bytes, out)
}

#[cfg(feature = "zstd")]
pub(super) fn decode_zstd(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), BackhandError> {
    use ms_compress_ruzstd::decoding::{FrameDecoder, StreamingDecoder};
    use std::io::Read;

    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size(8 * 1024 * 1024);
    let mut reader = StreamingDecoder::new_with_decoder(bytes, decoder)
        .map_err(|_| BackhandError::CorruptedOrInvalidSquashfs)?;
    let mut decoded = Vec::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if decoded.len() + count > MAX_OUTPUT {
            return Err(BackhandError::CorruptedOrInvalidSquashfs);
        }
        decoded.extend_from_slice(&buffer[..count]);
    }
    if let Some(checksum) = reader.decoder.get_checksum_from_data()
        && reader.decoder.get_calculated_checksum() != Some(checksum)
    {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    if !reader.decoder.is_finished() || !reader.into_inner().is_empty() {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    out.extend_from_slice(&decoded);
    Ok(())
}
