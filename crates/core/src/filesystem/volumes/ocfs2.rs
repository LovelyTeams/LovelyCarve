//! OCFS2 names retain source path identity; native EAs reuse named streams.
use super::*;
use crate::filesystem::volumes::ntfs::NtfsDataStream;

pub type Ocfs2Volume = RomVolume<ocfs2_reader::Ocfs2<RomImageSource>>;
pub fn parse_ocfs2_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("ocfs2:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    valid_lookup_path(&path).then_some((partition.parse().ok()?, path))
}
pub fn open_ocfs2_source(image: &Arc<DiskImage>, partition: u8) -> Result<Ocfs2Volume> {
    Ocfs2Volume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
impl Ocfs2Volume {
    pub fn list_streams(&self, path: &str) -> Result<Vec<NtfsDataStream>> {
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let metadata = reader.metadata(node, &mut context)?;
        let mut output = Vec::new();
        if matches!(metadata.kind, FileKind::Regular | FileKind::Symlink) {
            output.push(NtfsDataStream {
                name: String::new(),
                size: metadata.size,
                resident: false,
            });
        }
        output.extend(
            reader
                .attributes(node)?
                .into_iter()
                .map(|attribute| NtfsDataStream {
                    name: attribute.name,
                    size: attribute.bytes.len() as u64,
                    resident: attribute.resident,
                }),
        );
        Ok(output)
    }
    pub fn read_named_stream(&self, path: &str, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let mut bytes = reader
            .attributes(node)?
            .into_iter()
            .find(|attribute| attribute.name == name)
            .context("unknown OCFS2 attribute")?
            .bytes;
        bytes.truncate(max_bytes);
        Ok(bytes)
    }
    pub fn stream_named_stream(
        &self,
        path: &str,
        name: &str,
        chunk_size: usize,
        mut visitor: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let bytes = self.read_named_stream(path, name, 65536)?;
        for chunk in bytes.chunks(chunk_size.clamp(4096, 1024 * 1024)) {
            visitor(chunk)?;
        }
        Ok(bytes.len() as u64)
    }
}
