//! Classic Macintosh resource forks through the existing named-stream contract.
use super::*;
use crate::filesystem::volumes::ntfs::NtfsDataStream;
use classic_hfs::ForkKind;

const RESOURCE: &str = "com.apple.ResourceFork";
impl HfsVolume {
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
        ensure!(name == RESOURCE, "unknown classic HFS named fork: {name}");
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
            "short HFS resource fork read"
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
        ensure!(name == RESOURCE, "unknown classic HFS named fork: {name}");
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
                "short HFS resource fork stream"
            );
            visitor(&bytes[..count])?;
            offset += count as u64;
        }
        ensure!(offset == size, "incomplete HFS resource fork stream");
        Ok(offset)
    }
}
