//! Whole-stream decompression for data whose decompressed size isn't recorded
//! anywhere, such as a compressed initramfs member.
//!
//! [`decode`](crate::decode) decodes one filesystem block whose size the
//! filesystem recorded. A compressed initramfs member is one stream of unknown
//! size followed by more data, so [`decode_stream`] decodes until the stream's
//! own end, caps the output, and reports how much input the stream used so the
//! caller can carry on after it.

use alloc::vec::Vec;

use crate::error::{FoldError, Result};

/// A self-delimiting compressed stream, recognised by its magic bytes
/// ([`StreamCodec::sniff`]), as Linux recognises a compressed initramfs member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamCodec {
    /// gzip (RFC 1952): A header, raw DEFLATE, then CRC-32 and size.
    Gzip,
    /// Zstandard frames, skippable frames allowed between them.
    Zstd,
    /// One `.xz` stream.
    Xz,
    /// The legacy `.lzma` ("LZMA alone") format.
    Lzma,
    /// LZ4's legacy frame format (magic `0x184C2102`), which `lz4 -l` writes
    /// and the kernel reads.
    Lz4Legacy,
    /// bzip2: Recognised, not decoded.
    Bzip2,
    /// lzop: Recognised, not decoded.
    Lzo,
}

impl StreamCodec {
    /// The stream format `input` starts with, if any.
    pub fn sniff(input: &[u8]) -> Option<StreamCodec> {
        const MAGICS: &[(&[u8], StreamCodec)] = &[
            (&[0x1f, 0x8b], StreamCodec::Gzip),
            (&[0x28, 0xb5, 0x2f, 0xfd], StreamCodec::Zstd),
            (&[0xfd, b'7', b'z', b'X', b'Z', 0x00], StreamCodec::Xz),
            (&[0x5d, 0x00, 0x00], StreamCodec::Lzma),
            (&LZ4_LEGACY_MAGIC, StreamCodec::Lz4Legacy),
            (b"BZh", StreamCodec::Bzip2),
            (&[0x89, b'L', b'Z', b'O'], StreamCodec::Lzo),
        ];
        MAGICS
            .iter()
            .find(|(magic, _)| input.starts_with(magic))
            .map(|&(_, codec)| codec)
    }
}

/// Decode the `codec` stream at the start of `input`, producing at most
/// `max_out` bytes. Returns the output and the number of input bytes the
/// stream occupied. Output beyond `max_out` is refused as
/// [`FoldError::FileTooLarge`] before it's allocated.
pub fn decode_stream(codec: StreamCodec, input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    match codec {
        StreamCodec::Gzip => gzip(input, max_out),
        StreamCodec::Zstd => zstd(input, max_out),
        StreamCodec::Xz => xz(input, max_out),
        StreamCodec::Lzma => lzma(input, max_out),
        StreamCodec::Lz4Legacy => lz4_legacy(input, max_out),
        StreamCodec::Bzip2 => Err(FoldError::Unsupported("stream_bzip2")),
        StreamCodec::Lzo => Err(FoldError::Unsupported("stream_lzo")),
    }
}

/// Bytes decoded per step; the output grows by at most this much past what's
/// already checked against the cap.
#[cfg(any(
    feature = "codec-deflate",
    feature = "codec-zstd",
    feature = "codec-xz"
))]
const CHUNK: usize = 64 * 1024;

/// Append `chunk` to `out`, refusing to grow past `max_out`.
#[cfg(any(
    feature = "codec-deflate",
    feature = "codec-zstd",
    feature = "codec-xz"
))]
fn push_capped(out: &mut Vec<u8>, chunk: &[u8], max_out: usize) -> Result<()> {
    let want = out.len() + chunk.len();
    if want > max_out {
        return Err(FoldError::FileTooLarge {
            size: want as u64,
            max: max_out as u64,
        });
    }
    out.try_reserve(chunk.len())
        .map_err(|_| FoldError::Decompress("stream output allocation failed"))?;
    out.extend_from_slice(chunk);
    Ok(())
}

/// Read a `Read` source to its end into a capped buffer.
#[cfg(any(feature = "codec-zstd", feature = "codec-xz"))]
macro_rules! read_all_capped {
    ($reader:expr, $max_out:expr, $err:literal) => {{
        let mut out = Vec::new();
        let mut chunk = alloc::vec![0u8; CHUNK];
        loop {
            let n = $reader
                .read(&mut chunk)
                .map_err(|_| FoldError::Decompress($err))?;
            if n == 0 {
                break;
            }
            push_capped(&mut out, &chunk[..n], $max_out)?;
        }
        out
    }};
}

// --- gzip --------------------------------------------------------------------

/// CRC-32 (IEEE 802.3, reflected), the checksum in a gzip trailer.
#[cfg(feature = "codec-deflate")]
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

#[cfg(feature = "codec-deflate")]
fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| {
        CRC32_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

/// gzip header flags (RFC 1952 §2.3.1).
#[cfg(feature = "codec-deflate")]
const FHCRC: u8 = 0x02;
#[cfg(feature = "codec-deflate")]
const FEXTRA: u8 = 0x04;
#[cfg(feature = "codec-deflate")]
const FNAME: u8 = 0x08;
#[cfg(feature = "codec-deflate")]
const FCOMMENT: u8 = 0x10;

#[cfg(feature = "codec-deflate")]
fn gzip(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    let bad = FoldError::Corrupt("gzip_header");
    // ID1 ID2, CM = 8 (deflate), FLG, MTIME(4), XFL, OS.
    if input.len() < 10 || input[..3] != [0x1f, 0x8b, 8] || input[3] & 0xe0 != 0 {
        return Err(bad);
    }
    let flg = input[3];
    let mut pos = 10;
    if flg & FEXTRA != 0 {
        let xlen = input
            .get(pos..pos + 2)
            .ok_or(FoldError::Corrupt("gzip_header"))?;
        pos += 2 + usize::from(u16::from_le_bytes([xlen[0], xlen[1]]));
    }
    for flag in [FNAME, FCOMMENT] {
        if flg & flag != 0 {
            let nul = input
                .get(pos..)
                .and_then(|r| r.iter().position(|&b| b == 0))
                .ok_or(FoldError::Corrupt("gzip_header"))?;
            pos += nul + 1;
        }
    }
    if flg & FHCRC != 0 {
        pos += 2;
    }
    let body = input.get(pos..).ok_or(FoldError::Corrupt("gzip_header"))?;
    let (out, used) = inflate_raw_stream(body, max_out)?;
    pos += used;
    let trailer = input
        .get(pos..pos + 8)
        .ok_or(FoldError::Corrupt("gzip_truncated"))?;
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if crc != crc32(&out) || isize != out.len() as u32 {
        return Err(FoldError::Corrupt("gzip_checksum"));
    }
    Ok((out, pos + 8))
}

/// Inflate one raw DEFLATE stream; returns the output and the input it used.
#[cfg(feature = "codec-deflate")]
fn inflate_raw_stream(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut out = Vec::new();
    let mut chunk = alloc::vec![0u8; CHUNK];
    let mut pos = 0;
    loop {
        let r = inflate(&mut state, &input[pos..], &mut chunk, MZFlush::None);
        pos += r.bytes_consumed;
        push_capped(&mut out, &chunk[..r.bytes_written], max_out)?;
        match r.status {
            Ok(MZStatus::StreamEnd) => return Ok((out, pos)),
            Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
            _ => return Err(FoldError::Corrupt("deflate_truncated")),
        }
    }
}

#[cfg(not(feature = "codec-deflate"))]
fn gzip(_: &[u8], _: usize) -> Result<(Vec<u8>, usize)> {
    Err(FoldError::Unsupported("codec-deflate feature disabled"))
}

// --- zstd --------------------------------------------------------------------

#[cfg(feature = "codec-zstd")]
fn zstd(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    use ruzstd::io::Read as _;
    const FRAME_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

    let mut rest: &[u8] = input;
    let mut out = Vec::new();
    let mut frames = 0usize;
    loop {
        // A skippable frame: Magic 0x184D2A50..5F, a 4-byte size, then data.
        if rest.len() >= 8 && rest[0] & 0xf0 == 0x50 && rest[1..4] == [0x2a, 0x4d, 0x18] {
            let size = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
            rest = rest
                .get(8 + size..)
                .ok_or(FoldError::Corrupt("zstd_truncated"))?;
            continue;
        }
        if !rest.starts_with(&FRAME_MAGIC) {
            break;
        }
        let mut dec = ruzstd::decoding::StreamingDecoder::new(&mut rest)
            .map_err(|_| FoldError::Decompress("zstd frame init failed"))?;
        let frame = read_all_capped!(dec, max_out - out.len(), "zstd read failed");
        drop(dec);
        push_capped(&mut out, &frame, max_out)?;
        frames += 1;
    }
    if frames == 0 {
        return Err(FoldError::Corrupt("zstd_no_frame"));
    }
    Ok((out, input.len() - rest.len()))
}

#[cfg(not(feature = "codec-zstd"))]
fn zstd(_: &[u8], _: usize) -> Result<(Vec<u8>, usize)> {
    Err(FoldError::Unsupported("codec-zstd feature disabled"))
}

// --- xz and lzma -------------------------------------------------------------

#[cfg(feature = "codec-xz")]
fn xz(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    use lzma_rust2::Read as _;
    let mut rest: &[u8] = input;
    let mut rdr = lzma_rust2::XzReader::new(&mut rest, false);
    let out = read_all_capped!(rdr, max_out, "xz read failed");
    drop(rdr);
    Ok((out, input.len() - rest.len()))
}

#[cfg(feature = "codec-xz")]
fn lzma(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    use lzma_rust2::Read as _;
    let mut rest: &[u8] = input;
    // The 13-byte header carries the properties, the dictionary size and the
    // size (or "unknown", ended by a marker). No memory limit beyond the
    // dictionary the header names.
    let mut rdr = lzma_rust2::LzmaReader::new_mem_limit(&mut rest, u32::MAX, None)
        .map_err(|_| FoldError::Corrupt("lzma_header"))?;
    let out = read_all_capped!(rdr, max_out, "lzma read failed");
    drop(rdr);
    Ok((out, input.len() - rest.len()))
}

#[cfg(not(feature = "codec-xz"))]
fn xz(_: &[u8], _: usize) -> Result<(Vec<u8>, usize)> {
    Err(FoldError::Unsupported("codec-xz feature disabled"))
}

#[cfg(not(feature = "codec-xz"))]
fn lzma(_: &[u8], _: usize) -> Result<(Vec<u8>, usize)> {
    Err(FoldError::Unsupported("codec-xz feature disabled"))
}

// --- lz4 legacy --------------------------------------------------------------

/// `0x184C2102`, little-endian.
const LZ4_LEGACY_MAGIC: [u8; 4] = [0x02, 0x21, 0x4c, 0x18];
/// A legacy block decompresses to at most 8 MiB.
#[cfg(feature = "codec-lz4")]
const LZ4_LEGACY_BLOCK: usize = 8 << 20;
/// LZ4's worst-case compressed size for an 8 MiB block.
#[cfg(feature = "codec-lz4")]
const LZ4_LEGACY_MAX_COMPRESSED: usize = LZ4_LEGACY_BLOCK + LZ4_LEGACY_BLOCK / 255 + 16;

/// The legacy format is the magic, then blocks of a 4-byte little-endian
/// compressed size and an LZ4 block, with no end marker: The stream ends where
/// the next size can't be a block (zero, past the input, or over the bound).
/// A repeated magic starts another legacy stream, as `lz4 -l` concatenates.
#[cfg(feature = "codec-lz4")]
fn lz4_legacy(input: &[u8], max_out: usize) -> Result<(Vec<u8>, usize)> {
    if !input.starts_with(&LZ4_LEGACY_MAGIC) {
        return Err(FoldError::Corrupt("lz4_legacy_magic"));
    }
    let mut out = Vec::new();
    let mut pos = 4;
    while let Some(head) = input.get(pos..pos + 4) {
        if head == LZ4_LEGACY_MAGIC {
            pos += 4;
            continue;
        }
        let size = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if size == 0 || size > LZ4_LEGACY_MAX_COMPRESSED || size > input.len() - pos - 4 {
            break;
        }
        let block = &input[pos + 4..pos + 4 + size];
        let start = out.len();
        let room = core::cmp::min(LZ4_LEGACY_BLOCK, max_out - start);
        out.try_reserve(room)
            .map_err(|_| FoldError::Decompress("stream output allocation failed"))?;
        out.resize(start + room, 0);
        match lz4_flex::block::decompress_into(block, &mut out[start..]) {
            Ok(n) => out.truncate(start + n),
            // Out of room under the cap: The block is bigger than allowed.
            Err(_) if room < LZ4_LEGACY_BLOCK => {
                return Err(FoldError::FileTooLarge {
                    size: (start + room + 1) as u64,
                    max: max_out as u64,
                })
            }
            Err(_) => return Err(FoldError::Decompress("lz4 legacy block failed")),
        }
        pos += 4 + size;
    }
    Ok((out, pos))
}

#[cfg(not(feature = "codec-lz4"))]
fn lz4_legacy(_: &[u8], _: usize) -> Result<(Vec<u8>, usize)> {
    Err(FoldError::Unsupported("codec-lz4 feature disabled"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "codec-deflate")]
    #[test]
    fn crc32_known_answer() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn sniff_recognises_each_magic() {
        assert_eq!(
            StreamCodec::sniff(&[0x1f, 0x8b, 8]),
            Some(StreamCodec::Gzip)
        );
        assert_eq!(
            StreamCodec::sniff(&[0x02, 0x21, 0x4c, 0x18, 0]),
            Some(StreamCodec::Lz4Legacy)
        );
        assert_eq!(StreamCodec::sniff(b"BZh9"), Some(StreamCodec::Bzip2));
        assert_eq!(StreamCodec::sniff(b"070701"), None);
    }

    #[cfg(feature = "codec-deflate")]
    #[test]
    fn gzip_member_then_trailing_data() {
        // Python's gzip.compress(b"hello\n", mtime=0), then two bytes that
        // aren't gzip.
        let mut gz: Vec<u8> = alloc::vec![
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xcb, 0x48, 0xcd, 0xc9,
            0xc9, 0xe7, 0x02, 0x00, 0x20, 0x30, 0x3a, 0x36, 0x06, 0x00, 0x00, 0x00,
        ];
        let len = gz.len();
        gz.extend_from_slice(b"XY");
        let (out, used) = decode_stream(StreamCodec::Gzip, &gz, 1024).unwrap();
        assert_eq!(out, b"hello\n");
        assert_eq!(used, len);
        assert!(matches!(
            decode_stream(StreamCodec::Gzip, &gz, 3),
            Err(FoldError::FileTooLarge { .. })
        ));
        gz[len - 5] ^= 1;
        assert!(matches!(
            decode_stream(StreamCodec::Gzip, &gz, 1024),
            Err(FoldError::Corrupt("gzip_checksum"))
        ));
    }

    #[test]
    fn bzip2_and_lzo_are_refused() {
        assert!(matches!(
            decode_stream(StreamCodec::Bzip2, b"BZh9", 10),
            Err(FoldError::Unsupported("stream_bzip2"))
        ));
        assert!(matches!(
            decode_stream(StreamCodec::Lzo, b"\x89LZO", 10),
            Err(FoldError::Unsupported("stream_lzo"))
        ));
    }
}
