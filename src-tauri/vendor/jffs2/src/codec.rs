//! Pure Rust data decoding; bounded RTIME adapted from jffs2-rs.
use crate::nodes::MAX_DATA;
use anyhow::{Context, Result, bail, ensure};

pub(crate) fn decode(bytes: &[u8], kind: u8, size: usize) -> Result<Vec<u8>> {
    ensure!(size <= MAX_DATA, "JFFS2 decompression exceeds limit");
    match kind {
        0 | 4 => {
            ensure!(bytes.len() == size, "JFFS2 stored length mismatch");
            Ok(bytes.to_vec())
        }
        1 => {
            ensure!(bytes.is_empty(), "JFFS2 zero node contains data");
            Ok(vec![0; size])
        }
        2 => {
            let mut positions = [0u16; 256];
            let mut output = Vec::with_capacity(size);
            let mut input = 0;
            while output.len() < size {
                let pair = bytes
                    .get(input..input + 2)
                    .context("JFFS2 truncated RTIME pair")?;
                input += 2;
                let value = pair[0];
                let repeat = usize::from(pair[1]);
                ensure!(
                    output.len() + 1 + repeat <= size,
                    "JFFS2 RTIME output overflow"
                );
                output.push(value);
                let previous = usize::from(positions[value as usize]);
                // RTIME's history positions wrap at 16 bits, including large nodes.
                positions[value as usize] = output.len() as u16;
                for displacement in 0..repeat {
                    let value = *output
                        .get(previous + displacement)
                        .context("JFFS2 invalid RTIME reference")?;
                    output.push(value);
                }
            }
            ensure!(input == bytes.len(), "JFFS2 trailing RTIME bytes");
            Ok(output)
        }
        6 => {
            let mut output = vec![0; size + 1];
            let mut decoder = flate2::Decompress::new(true);
            let status = decoder.decompress(bytes, &mut output, flate2::FlushDecompress::Finish)?;
            ensure!(
                status == flate2::Status::StreamEnd
                    && decoder.total_in() == bytes.len() as u64
                    && decoder.total_out() == size as u64,
                "JFFS2 zlib length/termination mismatch"
            );
            output.truncate(size);
            Ok(output)
        }
        7 => {
            let mut output = vec![0; size + 1];
            let written = lzokay::decompress::decompress(bytes, &mut output)
                .map_err(|error| anyhow::anyhow!("JFFS2 LZO: {error:?}"))?;
            ensure!(written == size, "JFFS2 LZO output size mismatch");
            output.truncate(size);
            Ok(output)
        }
        _ => bail!("unsupported JFFS2 compression {kind}"),
    }
}
