//! Native BeFS attributes reuse the existing named-stream IPC contract.
use super::*;
use crate::filesystem::volumes::ntfs::NtfsDataStream;

pub type BeFsVolume = RomVolume<befs_reader::BeFs<RomImageSource>>;
pub fn parse_befs_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("befs:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    valid_lookup_path(&path).then_some((partition.parse().ok()?, path))
}
pub fn open_befs_source(image: &Arc<DiskImage>, partition: u8) -> Result<BeFsVolume> {
    BeFsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
impl BeFsVolume {
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
                    size: attribute.size,
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
        let attribute = reader
            .attributes(node)?
            .into_iter()
            .find(|attribute| attribute.name == name)
            .context("unknown BeFS attribute")?;
        let count = lamfold::checked_full_read_len(attribute.size.min(max_bytes as u64))?;
        let mut bytes = vec![0; count];
        ensure!(
            reader.read_attribute_at(node, name, 0, &mut bytes)? == count,
            "short BeFS attribute read"
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
        let mut reader = self.reader.borrow_mut();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let (node, _) = Self::resolve(&mut reader, path, &mut context)?;
        let size = reader
            .attributes(node)?
            .into_iter()
            .find(|attribute| attribute.name == name)
            .context("unknown BeFS attribute")?
            .size;
        let mut bytes = vec![0; chunk_size.clamp(4096, 1024 * 1024)];
        let mut offset = 0;
        while offset < size {
            let count = (size - offset).min(bytes.len() as u64) as usize;
            ensure!(
                reader.read_attribute_at(node, name, offset, &mut bytes[..count])? == count,
                "short BeFS attribute stream"
            );
            visitor(&bytes[..count])?;
            offset += count as u64;
        }
        ensure!(offset == size, "incomplete BeFS attribute stream");
        Ok(offset)
    }
}
