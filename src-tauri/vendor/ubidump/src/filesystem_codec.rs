//! Bounded UBIFS data-node codecs. Linux crypto "deflate" uses raw DEFLATE.
use anyhow::{Result, ensure};
use std::io::Read;

pub fn decode(bytes: &[u8], compression: u16, size: usize) -> Result<Vec<u8>> {
    ensure!(
        (1..=4096).contains(&size),
        "UBIFS data size exceeds logical block"
    );
    let mut out = vec![0; size + 1];
    match compression {
        0 => {
            ensure!(bytes.len() == size, "UBIFS stored data length mismatch");
            return Ok(bytes.to_vec());
        }
        1 => {
            let count = lzokay::decompress::decompress(bytes, &mut out)
                .map_err(|error| anyhow::anyhow!("UBIFS LZO: {error:?}"))?;
            ensure!(count == size, "UBIFS LZO output size mismatch");
        }
        2 => {
            let mut decoder = flate2::Decompress::new(false);
            let status = decoder.decompress(bytes, &mut out, flate2::FlushDecompress::Finish)?;
            ensure!(
                status == flate2::Status::StreamEnd
                    && decoder.total_in() == bytes.len() as u64
                    && decoder.total_out() == size as u64,
                "UBIFS raw DEFLATE length/termination mismatch"
            );
        }
        3 => {
            let mut decoder = ms_compress_ruzstd::decoding::FrameDecoder::new();
            decoder.set_max_window_size(1024 * 1024);
            let mut reader =
                ms_compress_ruzstd::decoding::StreamingDecoder::new_with_decoder(bytes, decoder)?;
            let mut written = 0usize;
            loop {
                let count = reader.read(&mut out[written..])?;
                if count == 0 {
                    break;
                }
                written += count;
                ensure!(
                    written <= size,
                    "UBIFS Zstandard output exceeds logical block"
                );
            }
            if let Some(expected) = reader.decoder.get_checksum_from_data() {
                ensure!(
                    reader.decoder.get_calculated_checksum() == Some(expected),
                    "UBIFS Zstandard checksum mismatch"
                );
            }
            ensure!(
                written == size && reader.decoder.is_finished() && reader.into_inner().is_empty(),
                "UBIFS Zstandard length/termination mismatch"
            );
        }
        _ => anyhow::bail!("unsupported UBIFS compression {compression}"),
    }
    out.truncate(size);
    Ok(out)
}
