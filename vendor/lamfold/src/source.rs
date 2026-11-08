//! The byte source a frontend reads over.
//!
//! lamfold is a standalone crate, so it defines its own minimal random-access
//! source rather than depending on LamBoot's `BlockSource`. LamBoot adapts its
//! `BlockSource`/`SourceReader` to this trait at integration time; the two are
//! structurally identical (a bounded, byte-addressed, read-only medium), and a
//! source may itself be a view over another source — the recursion that lets an
//! `.iso` file inside a partition, or a squashfs inside that `.iso`, be read by
//! the same machinery.

use crate::error::Result;

/// A read-only, byte-addressed medium: a disc, a partition, an `.iso` file
/// region, or (recursively) a file inside one of those.
pub trait BlockSource {
    /// Total length of the source in bytes.
    fn len(&self) -> u64;

    /// Whether the source is empty (zero length).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill `buf` completely starting at byte `offset`. Returns
    /// [`crate::FoldError::Io`] on a short read or device error — frontends rely
    /// on a successful return meaning the whole buffer was filled.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

/// A `BlockSource` over an in-memory byte slice — the workhorse for host tests
/// and fixtures, and a legitimate source for an already-resident image.
pub struct SliceSource<'a> {
    data: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl BlockSource for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start =
            usize::try_from(offset).map_err(|_| crate::FoldError::Io("offset overflows usize"))?;
        let end = start
            .checked_add(buf.len())
            .ok_or(crate::FoldError::Io("offset+len overflow"))?;
        if end > self.data.len() {
            return Err(crate::FoldError::Io("read past end of source"));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_source_reads_and_bounds_check() {
        let bytes = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let mut s = SliceSource::new(&bytes);
        assert_eq!(s.len(), 8);
        assert!(!s.is_empty());
        let mut buf = [0u8; 3];
        s.read_at(2, &mut buf).unwrap();
        assert_eq!(buf, [2, 3, 4]);
        // past end is rejected, not truncated
        let mut over = [0u8; 4];
        assert!(s.read_at(6, &mut over).is_err());
    }
}
