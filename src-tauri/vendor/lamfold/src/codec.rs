//! The shared decompression-codec registry (L2).
//!
//! One entry point — [`decode`] — for every compressor a read-only media format
//! uses, so each decoder is bound and optimized once and shared across the
//! flock (squashfs/erofs/zisofs/cramfs). Every codec is a permissive `no_std`
//! crate (see `the lamfold design spec` §3.1/§6); each is behind a cargo feature so a
//! bootloader build pulls only what its media needs.
//!
//! The registry wires + tests **deflate** (raw + zlib, `miniz_oxide`) and **lz4** (block,
//! `lz4_flex`) — the two most broadly-used (squashfs-gzip/cramfs/zisofs/
//! erofs-deflate, and squashfs-lz4/erofs-lz4). **zstd/xz/lzo** are declared and
//! are provided by (`ruzstd`/`lzma-rust2`/`lzokay`), where each gets a real
//! decode arm + round-trip fixtures.

use alloc::vec::Vec;

use crate::error::{FoldError, Result};
use crate::read_cap::checked_block_len;

/// The compressors a Family-A read-only format can use, matched to the crate
/// that decodes each (`the lamfold design spec` §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Raw DEFLATE (no header) — erofs `deflate`.
    Deflate,
    /// zlib-wrapped DEFLATE — squashfs `gzip`, cramfs, zisofs.
    Zlib,
    /// LZ4 **block** format (size known from FS metadata) — squashfs/erofs lz4.
    Lz4,
    /// XZ / LZMA / MicroLZMA — squashfs `xz`, erofs lzma. (`lzma-rust2`)
    Xz,
    /// Zstandard — squashfs/erofs zstd. (`ruzstd`)
    Zstd,
    /// LZO — squashfs `lzo`. (`lzokay`)
    Lzo,
}

/// Decompress `input` into a fresh buffer. `expected_len` is the decompressed
/// size the filesystem recorded for this block; it bounds the output (and is
/// required by the size-less block codecs, lz4/lzo). The declared size is capped
/// via [`checked_block_len`] *before* any allocation — the decompression-bomb
/// guard.
pub fn decode(codec: Codec, input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    // Bomb guard: refuse an absurd declared output before allocating for it.
    let _ = checked_block_len(expected_len as u64)?;
    match codec {
        Codec::Deflate => inflate_raw(input, expected_len),
        Codec::Zlib => inflate_zlib(input, expected_len),
        Codec::Lz4 => lz4_block(input, expected_len),
        Codec::Xz => xz_decode(input, expected_len),
        Codec::Zstd => zstd_decode(input, expected_len),
        Codec::Lzo => lzo_decode(input, expected_len),
    }
}

/// LZ4 **block** decode with an external dictionary — the prior decompressed
/// output that EROFS reaches into through its sliding window (`lz4_max_distance`,
/// ≤ 64 KiB). Unlike [`decode`], the dictionary is the bytes logically preceding
/// this pcluster's output, and the produced length is the pcluster's exact
/// decompressed size (`expected_len`). Used by `lamfold-erofs`'s compressed path.
#[cfg(feature = "codec-lz4")]
pub fn lz4_block_with_dict(input: &[u8], expected_len: usize, dict: &[u8]) -> Result<Vec<u8>> {
    // Same bomb guard as `decode`: bound the declared output before allocating.
    let _ = checked_block_len(expected_len as u64)?;
    let mut out = alloc::vec![0u8; expected_len];
    let n = lz4_flex::block::decompress_into_with_dict(input, &mut out, dict)
        .map_err(|_| FoldError::Decompress("erofs lz4 dict decompress failed"))?;
    if n != expected_len {
        return Err(FoldError::Decompress(
            "erofs lz4 dict decompress: short output",
        ));
    }
    Ok(out)
}

#[cfg(not(feature = "codec-lz4"))]
pub fn lz4_block_with_dict(_: &[u8], _: usize, _: &[u8]) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-lz4 feature disabled"))
}

/// Decode an EROFS **MicroLZMA** pcluster — a raw LZMA1 range-coded stream with
/// no `.xz`/`.lzma` container, no 13-byte header, and no end-of-stream marker
/// (the decompressed length is known). [`decode`]`(Codec::Xz, ..)` is the wrong
/// path here: `XzReader` requires the `.xz` magic and rejects MicroLZMA.
///
/// `span` is the on-disk stream whose **first byte is a discard slot** occupying
/// LZMA1's mandatory leading `0x00`; this swaps it for `0x00` and feeds the rest
/// verbatim. `props` is the LZMA properties byte (mkfs writes the constant
/// `0x5d` = lc3/lp0/pb2). `dict_size` is the volume's configured window; it is a
/// floor, not the literal window — on a barely-compressible multi-block pcluster
/// the encoder's match distances reach across the whole decompressed output, so
/// the window is widened to `max(dict_size, expected_len)` (a larger window never
/// breaks a stream that used shorter distances). Mirrors `lz4_block_with_dict`'s
/// role as the EROFS-specific escape hatch outside the uniform [`decode`] shape.
#[cfg(feature = "codec-xz")]
pub fn microlzma_block_decode(
    span: &[u8],
    props: u8,
    dict_size: u32,
    expected_len: usize,
) -> Result<Vec<u8>> {
    use lzma_rust2::Read as _;
    let _ = checked_block_len(expected_len as u64)?;
    let (&discard, body) = span
        .split_first()
        .ok_or(FoldError::Decompress("erofs microlzma: empty span"))?;
    let _ = discard; // the slot value (mkfs writes 0xA2) is overwritten below
    let mut stream = Vec::with_capacity(span.len());
    stream.push(0x00u8);
    stream.extend_from_slice(body);
    let window = core::cmp::max(dict_size, expected_len as u32);
    let mut rdr = lzma_rust2::LzmaReader::new_with_props(
        stream.as_slice(),
        expected_len as u64,
        props,
        window,
        None,
    )
    .map_err(|_| FoldError::Decompress("erofs microlzma: init"))?;
    let mut out = alloc::vec![0u8; expected_len];
    let mut filled = 0;
    while filled < expected_len {
        let n = rdr
            .read(&mut out[filled..])
            .map_err(|_| FoldError::Decompress("erofs microlzma: read"))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    if filled != expected_len {
        return Err(FoldError::Decompress("erofs microlzma: short output"));
    }
    Ok(out)
}

#[cfg(not(feature = "codec-xz"))]
pub fn microlzma_block_decode(_: &[u8], _: u8, _: u32, _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-xz feature disabled"))
}

#[cfg(feature = "codec-deflate")]
fn inflate_zlib(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(input, expected_len)
        .map_err(|_| FoldError::Decompress("zlib inflate failed"))
}

#[cfg(feature = "codec-deflate")]
fn inflate_raw(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(input, expected_len)
        .map_err(|_| FoldError::Decompress("raw inflate failed"))
}

#[cfg(not(feature = "codec-deflate"))]
fn inflate_zlib(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-deflate feature disabled"))
}

#[cfg(not(feature = "codec-deflate"))]
fn inflate_raw(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-deflate feature disabled"))
}

#[cfg(feature = "codec-lz4")]
fn lz4_block(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    lz4_flex::block::decompress(input, expected_len)
        .map_err(|_| FoldError::Decompress("lz4 block decompress failed"))
}

#[cfg(not(feature = "codec-lz4"))]
fn lz4_block(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-lz4 feature disabled"))
}

#[cfg(feature = "codec-zstd")]
fn zstd_decode(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    use ruzstd::io::Read as _;
    let mut dec = ruzstd::decoding::StreamingDecoder::new(input)
        .map_err(|_| FoldError::Decompress("zstd frame init failed"))?;
    let mut out = alloc::vec![0u8; expected_len];
    let mut filled = 0;
    while filled < expected_len {
        let n = dec
            .read(&mut out[filled..])
            .map_err(|_| FoldError::Decompress("zstd read failed"))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    out.truncate(filled);
    Ok(out)
}

#[cfg(not(feature = "codec-zstd"))]
fn zstd_decode(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-zstd feature disabled"))
}

#[cfg(feature = "codec-xz")]
fn xz_decode(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    use lzma_rust2::Read as _;
    // `false` = a single xz stream (the per-block shape squashfs/erofs emit).
    let mut rdr = lzma_rust2::XzReader::new(input, false);
    let mut out = alloc::vec![0u8; expected_len];
    let mut filled = 0;
    while filled < expected_len {
        let n = rdr
            .read(&mut out[filled..])
            .map_err(|_| FoldError::Decompress("xz read failed"))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    out.truncate(filled);
    Ok(out)
}

#[cfg(not(feature = "codec-xz"))]
fn xz_decode(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-xz feature disabled"))
}

#[cfg(feature = "codec-lzo")]
fn lzo_decode(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    // LZO stores no length, so the caller's `expected_len` (from FS metadata)
    // sizes the output buffer; lzokay writes into it and returns the real length.
    let mut out = alloc::vec![0u8; expected_len];
    let n = lzokay::decompress::decompress(input, &mut out)
        .map_err(|_| FoldError::Decompress("lzo decompress failed"))?;
    out.truncate(n);
    Ok(out)
}

#[cfg(not(feature = "codec-lzo"))]
fn lzo_decode(_: &[u8], _: usize) -> Result<Vec<u8>> {
    Err(FoldError::Unsupported("codec-lzo feature disabled"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_cap::MAX_DECOMPRESSED_BLOCK_BYTES;

    #[cfg(feature = "codec-deflate")]
    #[test]
    fn zlib_roundtrip() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(200);
        let comp = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        let out = decode(Codec::Zlib, &comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[cfg(feature = "codec-deflate")]
    #[test]
    fn raw_deflate_roundtrip() {
        let data = b"raw deflate payload ".repeat(150);
        let comp = miniz_oxide::deflate::compress_to_vec(&data, 6);
        let out = decode(Codec::Deflate, &comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[cfg(feature = "codec-lz4")]
    #[test]
    fn lz4_block_roundtrip() {
        let data = b"lz4 block format payload, repeated. ".repeat(120);
        let comp = lz4_flex::block::compress(&data);
        let out = decode(Codec::Lz4, &comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[cfg(feature = "codec-zstd")]
    #[test]
    fn zstd_roundtrip() {
        let data = b"zstd payload that should round-trip cleanly. ".repeat(100);
        let comp = ruzstd::encoding::compress_to_vec(
            &data[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let out = decode(Codec::Zstd, &comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[cfg(feature = "codec-lzo")]
    #[test]
    fn lzo_roundtrip() {
        let data = b"lzo payload repeated for compressibility. ".repeat(90);
        let comp = lzokay::compress::compress(&data).unwrap();
        let out = decode(Codec::Lzo, &comp, data.len()).unwrap();
        assert_eq!(out, data);
    }

    #[cfg(feature = "codec-xz")]
    #[test]
    fn xz_fixture_decodes() {
        // A real `.xz` stream produced by `xz -6` (system tool); the substrate
        // decodes it via XzReader. Plaintext is the repeated phrase below.
        const XZ_FIXTURE: &[u8] = &[
            0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 0x00, 0x04, 0xe6, 0xd6, 0xb4, 0x46, 0x04, 0xc0,
            0x40, 0xb2, 0x02, 0x21, 0x01, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xdb, 0x98, 0x31, 0xa1, 0xe0, 0x01, 0x31, 0x00, 0x38, 0x5d, 0x00, 0x2a, 0x1a, 0x08,
            0xa2, 0x02, 0xfb, 0xd2, 0xa4, 0xb1, 0x5a, 0xb1, 0x82, 0x1c, 0x0d, 0xd8, 0x01, 0xe7,
            0x98, 0xd2, 0x14, 0x7e, 0x5c, 0x59, 0x15, 0xdf, 0x26, 0x1e, 0x76, 0xab, 0xd2, 0xa6,
            0x0b, 0x13, 0x2a, 0x61, 0x94, 0x3b, 0xa8, 0x28, 0xab, 0x00, 0x72, 0x1b, 0x5b, 0x80,
            0x31, 0xfa, 0xbd, 0x77, 0x99, 0x75, 0x87, 0xf0, 0x91, 0x00, 0x00, 0x00, 0x2a, 0x71,
            0x2f, 0xcc, 0xc0, 0xdb, 0x5c, 0x41, 0x00, 0x01, 0x5c, 0xb2, 0x02, 0x00, 0x00, 0x00,
            0xae, 0x8f, 0x17, 0x78, 0xb1, 0xc4, 0x67, 0xfb, 0x02, 0x00, 0x00, 0x00, 0x00, 0x04,
            0x59, 0x5a,
        ];
        let expected = "The lamfold xz codec round-trips through XzReader. ".repeat(6);
        let out = decode(Codec::Xz, XZ_FIXTURE, expected.len()).unwrap();
        assert_eq!(out, expected.as_bytes());
    }

    #[test]
    fn garbage_input_never_panics() {
        // Hostile input must return (Ok or Err) without panicking — for every
        // codec, whether its feature is on (real decode) or off (Unsupported).
        let garbage = [0xFFu8, 0x00, 0x13, 0x37, 0x42, 0x99, 0x01, 0x80];
        for c in [
            Codec::Deflate,
            Codec::Zlib,
            Codec::Lz4,
            Codec::Xz,
            Codec::Zstd,
            Codec::Lzo,
        ] {
            let _ = decode(c, &garbage, 64);
        }
    }

    #[test]
    fn declared_output_over_block_cap_is_rejected_before_work() {
        // Independent of which codec features are on: the bomb guard fires first.
        let e = decode(
            Codec::Zlib,
            &[0u8; 8],
            (MAX_DECOMPRESSED_BLOCK_BYTES + 1) as usize,
        );
        assert!(matches!(e, Err(FoldError::FileTooLarge { .. })));
    }
}
