use no_std_io2::io::Read;

use flate2::read::ZlibDecoder;

pub use crate::traits::CompressionAction;
pub use crate::traits::types::Compressor;

/// Empty filesystem compressor
#[derive(Debug, Copy, Clone, Default)]
pub struct FilesystemCompressor;

/// Default compressor for SquashFS v3 (gzip only)
#[derive(Copy, Clone)]
pub struct DefaultCompressor;

impl DefaultCompressor {
    pub(crate) fn decompress_bounded(
        &self,
        bytes: &[u8],
        out: &mut Vec<u8>,
        maximum: usize,
    ) -> Result<(), crate::BackhandError> {
        let decoder = ZlibDecoder::new(bytes);
        let mut decoded = Vec::new();
        decoder.take(maximum as u64 + 1).read_to_end(&mut decoded)?;
        if decoded.len() > maximum {
            return Err(crate::BackhandError::CorruptedOrInvalidSquashfs);
        }
        out.extend_from_slice(&decoded);
        Ok(())
    }
}

impl CompressionAction for DefaultCompressor {
    type Error = crate::error::BackhandError;
    type Compressor = Option<Compressor>;
    type FilesystemCompressor = FilesystemCompressor;
    type SuperBlock = super::squashfs::SuperBlock;

    /// Decompress bytes using gzip (the only compression algorithm supported in v3)
    fn decompress(
        &self,
        bytes: &[u8],
        out: &mut Vec<u8>,
        _compressor: Self::Compressor,
    ) -> Result<(), Self::Error> {
        trace!("v3 decompress");
        self.decompress_bounded(bytes, out, 1 << 20)
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
