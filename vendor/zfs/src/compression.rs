//! ZFS LZ4 block decompression.
//!
//! On-disk ZFS LZ4 blocks are framed as: `BE u32 compressed_size` followed by
//! exactly `compressed_size` bytes of raw LZ4 block data. The uncompressed
//! size is *not* in the header — the SPA/DMU layers recover it from the
//! `BlockPtr.logical_size` field, but `decompress_lz4` is called without that
//! context. We therefore pass `lz4_flex::block::decompress` a generous upper
//! bound (16 MiB matches ZFS's largest supported recordsize) and let it grow
//! the output buffer as needed; allocations stop the moment the LZ4 stream
//! signals end-of-block.
//!
//! Replaces the upstream dependency on the unmaintained `lz4-compression`
//! crate, whose `decompress::decompress` happened to expose a
//! "decompress-until-block-end" entry point that newer LZ4 crates do not.

use nom::combinator::map_res;
use nom::multi::length_data;
use nom::number::complete::be_u32;
use nom::IResult;

const ZFS_LZ4_MAX_OUTPUT: usize = 16 * 1024 * 1024;

pub fn decompress_lz4(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
    map_res(length_data(be_u32), |compressed: &[u8]| {
        lz4_flex::block::decompress(compressed, ZFS_LZ4_MAX_OUTPUT)
    })(input)
}
