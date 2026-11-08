//! Decompression of extent data.
//!
//! Framing found by hexdump of the lz4, zstd and gzip fixtures
//! (docs/clean-room.md, S4); the codecs themselves are public formats:
//!
//! * lz4: a bare LZ4 block (no frame), followed by zero padding to the
//!   sector. Decoded here, from the LZ4 block format description (S5),
//!   until the expected output length is reached, so the padding is never
//!   mistaken for a sequence.
//! * zstd: a little-endian u32 holding the compressed length, then one
//!   standard zstd frame (decoded by `ruzstd`, MIT).
//! * gzip: a raw deflate stream (decoded by `miniz_oxide`).

use crate::error::{Error, Result};
use crate::extent::compression;

/// Decompress `data` (whole sectors as stored) of compression type `t`
/// into exactly `out_len` bytes.
pub fn decompress(t: u8, data: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let out = match t {
        compression::LZ4 => lz4_block(data, out_len)?,
        compression::ZSTD => zstd(data, out_len)?,
        compression::GZIP => miniz_oxide::inflate::decompress_to_vec_with_limit(data, out_len)
            .map_err(|e| Error::Corrupt(format!("deflate: {e:?}")))?,
        t => return Err(Error::Unsupported(format!("compression type {t}"))),
    };
    if out.len() != out_len {
        return Err(Error::Corrupt(format!(
            "decompressed {} bytes, expected {out_len}",
            out.len()
        )));
    }
    Ok(out)
}

/// Compress `data` as compression type `t` stores it, the framing the
/// decompressor above reads (#105): lz4 a bare LZ4 block (`lz4_flex`, MIT),
/// zstd a little-endian u32 length then one zstd frame (`ruzstd`, MIT),
/// gzip a raw deflate stream (`miniz_oxide`). The caller pads to the block
/// and keeps the result only when it saves at least one block.
pub fn compress(t: u8, data: &[u8]) -> Result<Vec<u8>> {
    Ok(match t {
        compression::LZ4 => lz4_flex::block::compress(data),
        compression::ZSTD => {
            let frame = ruzstd::encoding::compress_to_vec(
                data,
                ruzstd::encoding::CompressionLevel::Fastest,
            );
            let mut out = (frame.len() as u32).to_le_bytes().to_vec();
            out.extend_from_slice(&frame);
            out
        }
        compression::GZIP => miniz_oxide::deflate::compress_to_vec(data, 6),
        t => return Err(Error::Unsupported(format!("compressing as type {t}"))),
    })
}

fn zstd(data: &[u8], out_len: usize) -> Result<Vec<u8>> {
    if data.len() < 4 {
        return Err(Error::Corrupt(
            "zstd extent shorter than its length prefix".into(),
        ));
    }
    let n = u32::from_le_bytes(data[..4].try_into().expect("4 bytes")) as usize;
    let frame = data
        .get(4..4 + n)
        .ok_or_else(|| Error::Corrupt(format!("zstd length {n} runs past the extent")))?;
    let mut out = vec![0u8; out_len];
    let got = ruzstd::decoding::FrameDecoder::new()
        .decode_all(frame, &mut out)
        .map_err(|e| Error::Corrupt(format!("zstd: {e}")))?;
    out.truncate(got);
    Ok(out)
}

/// Decode one LZ4 block until `out_len` bytes have been produced.
///
/// A sequence is a token (high nibble literal length, low nibble match
/// length minus 4; 15 in either means more length bytes follow, each added,
/// until one is not 255), the literals, then a little-endian u16 offset
/// back into the output and the match, which may overlap itself. The last
/// sequence has literals only.
pub fn lz4_block(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let bad = |m: &str| Error::Corrupt(format!("lz4: {m}"));
    let mut out = Vec::with_capacity(out_len);
    let mut i = 0usize;
    let ext = |i: &mut usize, mut n: usize| -> Result<usize> {
        if n == 15 {
            loop {
                let b = *src
                    .get(*i)
                    .ok_or_else(|| bad("length runs past the input"))?;
                *i += 1;
                n += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        Ok(n)
    };
    while out.len() < out_len {
        let token = *src.get(i).ok_or_else(|| bad("token past the input"))?;
        i += 1;
        let lit = ext(&mut i, (token >> 4) as usize)?;
        let lits = src
            .get(i..i + lit)
            .ok_or_else(|| bad("literals past the input"))?;
        if out.len() + lit > out_len {
            return Err(bad("literals overrun the output"));
        }
        out.extend_from_slice(lits);
        i += lit;
        if out.len() == out_len {
            break;
        }
        let off = src
            .get(i..i + 2)
            .ok_or_else(|| bad("offset past the input"))?;
        let off = u16::from_le_bytes([off[0], off[1]]) as usize;
        i += 2;
        if off == 0 || off > out.len() {
            return Err(bad("match offset out of range"));
        }
        let mlen = ext(&mut i, (token & 0xf) as usize)? + 4;
        if out.len() + mlen > out_len {
            return Err(bad("match overruns the output"));
        }
        let start = out.len() - off;
        for k in 0..mlen {
            let b = out[start + k];
            out.push(b);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sequence_of_a_real_lz4_extent_decodes() {
        // From the lz4 fixture: "0" then a 7-byte match at offset 1, then
        // 31 literals.
        let mut src = vec![0x13, b'0', 0x01, 0x00, 0xf0, 0x10];
        src.extend_from_slice(b" the quick brown fox jumps over");
        src.extend_from_slice(&[0u8; 32]); // sector padding
        let out = lz4_block(&src, 39).unwrap();
        assert_eq!(&out, b"00000000 the quick brown fox jumps over");
    }

    #[test]
    fn hostile_lz4_is_refused() {
        assert!(lz4_block(&[0x10], 1).is_err());
        assert!(lz4_block(&[0x01, 0x05, 0x00], 10).is_err()); // offset past output
        assert!(lz4_block(&[0x00, 0x00, 0x00], 4).is_err());
        assert!(lz4_block(&[0xf0, 0xff], 300).is_err());
    }

    #[test]
    fn short_zstd_is_refused() {
        assert!(zstd(&[1, 2], 10).is_err());
        assert!(zstd(&[100, 0, 0, 0, 0x28], 10).is_err());
    }
}
