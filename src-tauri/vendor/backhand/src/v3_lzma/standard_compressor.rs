use crate::lzma::MAX_BLOCK_SIZE;

pub use crate::traits::CompressionAction;
pub use crate::traits::types::Compressor;

#[derive(Copy, Clone)]
pub struct LzmaStandardCompressor;

impl CompressionAction for LzmaStandardCompressor {
    type Error = crate::error::BackhandError;
    type Compressor = Option<Compressor>;
    type FilesystemCompressor = crate::v3::compressor::FilesystemCompressor;
    type SuperBlock = crate::v3::squashfs::SuperBlock;

    /// Decompress bytes using standard LZMA for v3 filesystems
    ///
    /// A v3 image holds either a full .lzma stream or a bare 5-byte header.
    /// This tries the first form, then builds the missing header for the
    /// second.
    fn decompress(
        &self,
        bytes: &[u8],
        out: &mut Vec<u8>,
        _compressor: Self::Compressor,
    ) -> Result<(), Self::Error> {
        crate::lzma::decompress_standard(bytes, out, MAX_BLOCK_SIZE)
    }

    /// Using the current compressor from the superblock, compress bytes
    fn compress(
        &self,
        _bytes: &[u8],
        _fc: Self::FilesystemCompressor,
        _block_size: u32,
    ) -> Result<Vec<u8>, Self::Error> {
        unimplemented!();
    }
}
