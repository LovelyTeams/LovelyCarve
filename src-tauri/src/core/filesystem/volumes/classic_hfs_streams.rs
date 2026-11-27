//! Classic Macintosh resource forks through the existing named-stream contract.
use super::*;
use crate::core::common::error::Failure;
use crate::core::filesystem::volumes::ntfs::NtfsDataStream;
use classic_hfs::ForkKind;

const RESOURCE: &str = "com.apple.ResourceFork";

pub trait AppleForks: FoldFrontend<RomImageSource> {
    fn fork_size(&mut self, node: lamfold::NodeId, kind: ForkKind) -> lamfold::Result<u64>;
    fn read_fork_at(
        &mut self,
        node: lamfold::NodeId,
        kind: ForkKind,
        offset: u64,
        bytes: &mut [u8],
    ) -> lamfold::Result<usize>;
}

impl AppleForks for classic_hfs::Hfs<RomImageSource> {
    fn fork_size(&mut self, node: lamfold::NodeId, kind: ForkKind) -> lamfold::Result<u64> {
        Self::fork_size(self, node, kind)
    }

    fn read_fork_at(
        &mut self,
        node: lamfold::NodeId,
        kind: ForkKind,
        offset: u64,
        bytes: &mut [u8],
    ) -> lamfold::Result<usize> {
        Self::read_fork_at(self, node, kind, offset, bytes)
    }
}

impl AppleForks for mfs_reader::Mfs<RomImageSource> {
    fn fork_size(&mut self, node: lamfold::NodeId, kind: ForkKind) -> lamfold::Result<u64> {
        mfs_reader::Mfs::fork_size(self, node, kind)
    }

    fn read_fork_at(
        &mut self,
        node: lamfold::NodeId,
        kind: ForkKind,
        offset: u64,
        bytes: &mut [u8],
    ) -> lamfold::Result<usize> {
        Self::read_fork_at(self, node, kind, offset, bytes)
    }
}

impl<F: AppleForks> RomVolume<F> {
    pub fn list_streams(&self, path: &str) -> Result<Vec<NtfsDataStream>> {
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        if reader.metadata(node, &mut context)?.kind == FileKind::Directory {
            return Ok(Vec::new());
        }
        Ok(vec![
            NtfsDataStream {
                name: String::new(),
                size: reader.fork_size(node, ForkKind::Data)?,
                resident: false,
            },
            NtfsDataStream {
                name: RESOURCE.into(),
                size: reader.fork_size(node, ForkKind::Resource)?,
                resident: false,
            },
        ])
    }
    pub fn read_named_stream(&self, path: &str, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
        ensure!(name == RESOURCE, "unknown Macintosh named fork: {name}");
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let size = reader
            .fork_size(node, ForkKind::Resource)?
            .min(max_bytes as u64);
        let mut bytes = vec![0; lamfold::checked_full_read_len(size)?];
        ensure!(
            reader.read_fork_at(node, ForkKind::Resource, 0, &mut bytes)? == bytes.len(),
            Failure::corrupt("short Macintosh resource fork read")
        );
        Ok(bytes)
    }
    pub fn stream_named_stream(
        &self,
        path: &str,
        name: &str,
        chunk_size: usize,
        mut visitor: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        ensure!(name == RESOURCE, "unknown Macintosh named fork: {name}");
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let size = reader.fork_size(node, ForkKind::Resource)?;
        let mut offset = 0;
        let mut bytes = vec![0; chunk_size.clamp(4096, 1024 * 1024)];
        while offset < size {
            let count = (size - offset).min(bytes.len() as u64) as usize;
            ensure!(
                reader.read_fork_at(node, ForkKind::Resource, offset, &mut bytes[..count])?
                    == count,
                Failure::corrupt("short Macintosh resource fork stream")
            );
            visitor(&bytes[..count])?;
            offset += count as u64;
        }
        ensure!(
            offset == size,
            Failure::corrupt("incomplete Macintosh resource fork stream")
        );
        Ok(offset)
    }
}
