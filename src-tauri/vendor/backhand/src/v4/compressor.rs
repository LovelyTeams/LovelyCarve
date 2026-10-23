//! Types of supported compression algorithms

use no_std_io2::io::{Read, Write};
use std::io::Cursor;

use deku::prelude::*;
#[cfg(feature = "gzip")]
use flate2::read::ZlibEncoder;
#[cfg(feature = "gzip")]
use flate2::{Compress, Compression};
#[cfg(feature = "gzip")]
use flate2::{Decompress, FlushDecompress, Status};
#[cfg(feature = "xz")]
use lzma_rust2::{CheckType, FilterType, XzOptions, XzWriter};

use crate::error::BackhandError;
use crate::traits::CompressionAction;
use crate::v4::filesystem::writer::{CompressionExtra, FilesystemCompressor};
use crate::v4::metadata::MetadataWriter;
use crate::v4::squashfs::Flags;

#[derive(Copy, Clone, Debug, PartialEq, Eq, DekuRead, DekuWrite, DekuSize, Default)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
#[deku(id_type = "u16")]
#[repr(u16)]
#[rustfmt::skip]
pub enum Compressor {
    Uncompressed = 0,
    Gzip = 1,
    Lzma = 2,
    Lzo =  3,
    #[default]
    Xz =   4,
    Lz4 =  5,
    Zstd = 6,
}

#[derive(Debug, DekuRead, DekuWrite, PartialEq, Eq, Clone, Copy)]
#[deku(
    endian = "endian",
    ctx = "endian: deku::ctx::Endian, compressor: Compressor"
)]
#[deku(id = "compressor")]
pub enum CompressionOptions {
    #[deku(id = "Compressor::Gzip")]
    Gzip(Gzip),

    #[deku(id = "Compressor::Lzo")]
    Lzo(Lzo),

    #[deku(id = "Compressor::Xz")]
    Xz(Xz),

    #[deku(id = "Compressor::Lz4")]
    Lz4(Lz4),

    #[deku(id = "Compressor::Zstd")]
    Zstd(Zstd),

    #[deku(id = "Compressor::Lzma")]
    Lzma,
}

#[derive(Debug, DekuRead, DekuWrite, DekuSize, PartialEq, Eq, Clone, Copy)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct Gzip {
    pub compression_level: u32,
    // A range from 9 ..= 15
    pub window_size: u8,
    // TODO: enum
    pub strategies: u16,
}

#[derive(Debug, DekuRead, DekuWrite, DekuSize, PartialEq, Eq, Clone, Copy)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct Lzo {
    // TODO: enum
    pub algorithm: u32,
    pub compression_level: u32,
}

#[derive(Debug, DekuRead, DekuWrite, PartialEq, Eq, Clone, Copy)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct Xz {
    pub dictionary_size: u32,
    pub filters: XzFilter,

    // the rest of these fields are from OpenWRT. These are optional, as the kernel will ignore
    // these fields when seen. We follow the same behaviour and don't attempt to parse if the bytes
    // for these aren't found
    // TODO: both are currently unused in this library
    // TODO: in openwrt, git-hash:f97ad870e11ebe5f3dcf833dda6c83b9165b37cb shows that before
    // official squashfs-tools had xz support they had the dictionary_size field as the last field
    // in this struct. If we get test images, I guess we can support this in the future.
    #[deku(cond = "!deku::reader.end()")]
    pub bit_opts: Option<u16>,
    #[deku(cond = "!deku::reader.end()")]
    pub fb: Option<u16>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, DekuRead, DekuWrite, DekuSize)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct XzFilter(u32);

impl XzFilter {
    pub fn new(filter: u32) -> Self {
        Self(filter)
    }

    fn x86(&self) -> bool {
        self.0 & 0x0001 == 0x0001
    }

    fn powerpc(&self) -> bool {
        self.0 & 0x0002 == 0x0002
    }

    fn ia64(&self) -> bool {
        self.0 & 0x0004 == 0x0004
    }

    fn arm(&self) -> bool {
        self.0 & 0x0008 == 0x0008
    }

    fn armthumb(&self) -> bool {
        self.0 & 0x0010 == 0x0010
    }

    fn sparc(&self) -> bool {
        self.0 & 0x0020 == 0x0020
    }
}

#[derive(Debug, DekuRead, DekuWrite, DekuSize, PartialEq, Eq, Clone, Copy)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct Lz4 {
    pub version: u32,
    //TODO: enum
    pub flags: u32,
}

#[derive(Debug, DekuRead, DekuWrite, DekuSize, PartialEq, Eq, Clone, Copy)]
#[deku(endian = "endian", ctx = "endian: deku::ctx::Endian")]
pub struct Zstd {
    pub compression_level: u32,
}

/// Default compressor that handles the compression features that are enabled
#[derive(Copy, Clone)]
pub struct DefaultCompressor;

/// The maximum size of one decompressed block.
#[cfg(feature = "gzip")]
const MAX_DECOMPRESSED_SIZE: usize = 4 * super::squashfs::MAX_BLOCK_SIZE as usize;

/// Decompress a complete zlib stream in one call. The data is added to `out`.
#[cfg(feature = "gzip")]
fn gzip_decode_buffer(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), BackhandError> {
    let start = out.len();
    let mut spare = out.capacity() - start;
    loop {
        if spare < 8 * 1024 {
            out.reserve(8 * 1024);
            spare = out.capacity() - start;
        }

        out.truncate(start);
        match Decompress::new(true).decompress_vec(bytes, out, FlushDecompress::Finish) {
            Ok(Status::StreamEnd) => return Ok(()),
            // The output did not fit. Increase the buffer and decode again.
            Ok(_) => {
                if spare >= MAX_DECOMPRESSED_SIZE {
                    return Err(BackhandError::CorruptedOrInvalidSquashfs);
                }
                out.reserve(spare * 2);
                spare = out.capacity() - start;
            }
            Err(_) => return Err(BackhandError::CorruptedOrInvalidSquashfs),
        }
    }
}

impl CompressionAction for DefaultCompressor {
    type Compressor = Compressor;
    type FilesystemCompressor = FilesystemCompressor;
    type SuperBlock = super::squashfs::SuperBlock;
    type Error = crate::BackhandError;
    /// Using the current compressor from the superblock, decompress bytes
    #[inline(never)]
    fn decompress(
        &self,
        bytes: &[u8],
        out: &mut Vec<u8>,
        compressor: Self::Compressor,
    ) -> Result<(), Self::Error> {
        match compressor {
            Compressor::Uncompressed => out.extend_from_slice(bytes),
            #[cfg(feature = "gzip")]
            Compressor::Gzip => gzip_decode_buffer(bytes, out)?,
            #[cfg(feature = "xz")]
            Compressor::Xz => super::native_codecs::decode_xz(bytes, out)?,
            #[cfg(feature = "lzo")]
            Compressor::Lzo => {
                out.resize(out.capacity(), 0);
                let (out_size, error) = rust_lzo::LZOContext::decompress_to_slice(bytes, out);
                let out_size = out_size.len();
                out.truncate(out_size);
                if error != rust_lzo::LZOError::OK {
                    return Err(BackhandError::CorruptedOrInvalidSquashfs);
                }
            }
            #[cfg(feature = "zstd")]
            Compressor::Zstd => super::native_codecs::decode_zstd(bytes, out)?,
            #[cfg(feature = "lz4")]
            Compressor::Lz4 => {
                out.resize(out.capacity(), 0u8);
                let out_size =
                    lz4_flex::decompress_into(bytes, out.as_mut_slice()).map_err(|e| {
                        BackhandError::CompressionInit(err_text!("lz4 decompression: {}", e))
                    })?;
                out.truncate(out_size);
            }
            _ => {
                return Err(BackhandError::UnsupportedCompression(err_text!(
                    "{:?}", compressor
                )));
            }
        }
        Ok(())
    }

    /// Using the current compressor from the superblock, compress bytes
    fn compress(
        &self,
        bytes: &[u8],
        fc: Self::FilesystemCompressor,
        block_size: u32,
    ) -> Result<Vec<u8>, Self::Error> {
        match (fc.id, fc.options, fc.extra) {
            (Compressor::Uncompressed, None, _) => Ok(bytes.to_vec()),
            #[cfg(feature = "xz")]
            (Compressor::Xz, option @ (Some(CompressionOptions::Xz(_)) | None), extra) => {
                let dict_size = match option {
                    None => block_size,
                    Some(CompressionOptions::Xz(option)) => option.dictionary_size,
                    Some(_) => unreachable!(),
                };
                let default_level = 6; // LZMA_DEFAULT
                let level = match extra {
                    None => default_level,
                    Some(CompressionExtra::Xz(xz)) => {
                        if let Some(level) = xz.level {
                            level
                        } else {
                            default_level
                        }
                    }
                };
                if level > 9 || !(4096..=8 * 1024 * 1024).contains(&dict_size) {
                    return Err(BackhandError::CorruptedOrInvalidSquashfs);
                }
                let mut options = XzOptions::with_preset(level);
                options.set_check_sum_type(CheckType::Crc32);
                options.lzma_options.dict_size = dict_size;
                // SquashFS's bit mask names alternative BCJ encodings, not a
                // chain of unrelated machine-code filters. Choose the shortest.
                let filters = if let Some(CompressionOptions::Xz(xz)) = option {
                    vec![
                        (xz.filters.x86(), FilterType::BcjX86),
                        (xz.filters.powerpc(), FilterType::BcjPpc),
                        (xz.filters.ia64(), FilterType::BcjIa64),
                        (xz.filters.arm(), FilterType::BcjArm),
                        (xz.filters.armthumb(), FilterType::BcjArmThumb),
                        (xz.filters.sparc(), FilterType::BcjSparc),
                    ]
                } else {
                    Vec::new()
                };
                let mut candidates = vec![None];
                candidates.extend(
                    filters
                        .into_iter()
                        .filter_map(|(enabled, filter)| enabled.then_some(Some(filter))),
                );
                let mut best: Option<Vec<u8>> = None;
                for filter in candidates {
                    let mut options = options.clone();
                    if let Some(filter) = filter {
                        options.prepend_pre_filter(filter, 0);
                    }
                    let mut encoder = XzWriter::new(Vec::new(), options)?;
                    encoder.write_all(bytes)?;
                    let encoded = encoder.finish()?;
                    if best
                        .as_ref()
                        .is_none_or(|previous| encoded.len() < previous.len())
                    {
                        best = Some(encoded);
                    }
                }
                best.ok_or(BackhandError::CorruptedOrInvalidSquashfs)
            }
            #[cfg(feature = "gzip")]
            (Compressor::Gzip, option @ (Some(CompressionOptions::Gzip(_)) | None), _) => {
                let (compression_level, window_size) = match option {
                    None => (Compression::best(), 15), // 9 (best compression level)
                    Some(CompressionOptions::Gzip(option)) => (
                        Compression::new(option.compression_level),
                        option.window_size,
                    ),
                    Some(_) => unreachable!(),
                };
                let options = Compress::new_with_window_bits(compression_level, true, window_size);

                // TODO(#8): Use strategies

                let mut encoder = ZlibEncoder::new_with_compress(Cursor::new(bytes), options);
                let mut buf = vec![];
                encoder.read_to_end(&mut buf)?;
                Ok(buf)
            }
            #[cfg(feature = "lzo")]
            (Compressor::Lzo, _, _) => {
                let mut lzo = rust_lzo::LZOContext::new();
                let mut buf = vec![0; rust_lzo::worst_compress(bytes.len())];
                let error = lzo.compress(bytes, &mut buf);
                if error != rust_lzo::LZOError::OK {
                    return Err(BackhandError::CorruptedOrInvalidSquashfs);
                }
                Ok(buf)
            }
            #[cfg(feature = "zstd-write")]
            (Compressor::Zstd, option @ (Some(CompressionOptions::Zstd(_)) | None), _) => {
                let compression_level = match option {
                    None => 3,
                    Some(CompressionOptions::Zstd(option)) => option.compression_level,
                    Some(_) => unreachable!(),
                };
                let mut encoder = zstd::bulk::Compressor::new(compression_level as i32)?;
                let buffer_len = zstd_safe::compress_bound(bytes.len());
                let mut buf = Vec::with_capacity(buffer_len);
                encoder.compress_to_buffer(bytes, &mut buf)?;
                Ok(buf)
            }
            #[cfg(feature = "lz4")]
            (Compressor::Lz4, _option, _) => Ok(lz4_flex::compress(bytes)),
            _ => Err(BackhandError::UnsupportedCompression(err_text!(
                "{:?}", fc.id
            ))),
        }
    }

    /// Using the current compressor options, create compression options
    fn compression_options(
        &self,
        superblock: &mut Self::SuperBlock,
        kind: &crate::kinds::Kind,
        fs_compressor: Self::FilesystemCompressor,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut w = Cursor::new(vec![]);

        // Write compression options, if any
        if let Some(options) = &fs_compressor.options {
            trace!("writing compression options");
            superblock.flags |= Flags::CompressorOptionsArePresent as u16;
            let mut compression_opt_buf_out = Cursor::new(vec![]);
            let mut writer = Writer::new(&mut compression_opt_buf_out);
            match options {
                CompressionOptions::Gzip(gzip) => {
                    gzip.to_writer(&mut writer, kind.inner.type_endian)?
                }
                CompressionOptions::Lz4(lz4) => {
                    lz4.to_writer(&mut writer, kind.inner.type_endian)?
                }
                CompressionOptions::Zstd(zstd) => {
                    zstd.to_writer(&mut writer, kind.inner.type_endian)?
                }
                CompressionOptions::Xz(xz) => xz.to_writer(&mut writer, kind.inner.type_endian)?,
                CompressionOptions::Lzo(lzo) => {
                    lzo.to_writer(&mut writer, kind.inner.type_endian)?
                }
                CompressionOptions::Lzma => {}
            }
            let mut metadata = MetadataWriter::new(
                self,
                fs_compressor,
                superblock.block_size,
                kind.inner.data_endian,
            );
            metadata.write_all(compression_opt_buf_out.get_ref())?;
            metadata.finalize(&mut w)?;
        }

        Ok(Some(w.into_inner()))
    }
}
