//! Read-only F2FS, using the latest valid committed checkpoint (no roll-forward).
//! Metadata decoders adapted from fstool are isolated in sibling modules.
#[allow(dead_code)]
mod checkpoint;
#[allow(dead_code)]
mod constants;
mod directory;
mod inode;
#[allow(dead_code)]
mod nat;
#[allow(dead_code)]
mod superblock;
#[cfg(test)]
mod tests;

use checkpoint::Checkpoint;
pub use lamfold::Result;
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId,
    SubstrateCtx,
};
use superblock::Superblock;

fn invalid(message: String) -> FoldError {
    FoldError::Source(message)
}
fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(FoldError::Corrupt("F2FS short integer"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("F2FS integer"))?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(FoldError::Corrupt("F2FS short integer"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("F2FS integer"))?,
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(FoldError::Corrupt("F2FS short integer"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("F2FS integer"))?,
    ))
}
/// F2FS's seeded reflected CRC32; unlike zlib CRC this has no final complement.
pub fn crc32_raw(mut seed: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        seed ^= *byte as u32;
        for _ in 0..8 {
            seed = (seed >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(seed & 1));
        }
    }
    seed
}

mod block {
    use super::*;
    /// Minimal read-only seam for the upstream metadata decoders. All reads
    /// are exact and volume-relative; writable fstool APIs are not included.
    pub trait BlockDevice {
        fn total_size(&self) -> u64;
        fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()>;
    }
    impl<S: BlockSource> BlockDevice for S {
        fn total_size(&self) -> u64 {
            self.len()
        }
        fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
            require(
                offset
                    .checked_add(buffer.len() as u64)
                    .is_some_and(|end| end <= self.len()),
                "F2FS source read bounds",
            )?;
            BlockSource::read_at(self, offset, buffer)
        }
    }
}

pub struct F2fs<S: BlockSource> {
    source: S,
    sb: Superblock,
    cp: Checkpoint,
    feature: u32,
    uuid: [u8; 16],
}

impl<S: BlockSource> F2fs<S> {
    fn node_block(&mut self, nid: u32, owner: Option<u32>) -> Result<[u8; 4096]> {
        let address = nat::lookup_node(&mut self.source, &self.sb, &self.cp, nid)?;
        require(
            owner.is_none_or(|owner| address.ino == owner),
            "F2FS NAT owner mismatch",
        )?;
        let bytes = self.data_block(address.block)?;
        require(
            u32_at(&bytes, 4072)? == nid && u32_at(&bytes, 4076)? == address.ino,
            "F2FS node footer identity mismatch",
        )?;
        Ok(bytes)
    }
    fn data_block(&mut self, address: u32) -> Result<[u8; 4096]> {
        let end = self.sb.main_blkaddr as u64
            + self.sb.segment_count_main as u64 * self.sb.blocks_per_seg() as u64;
        require(
            address >= self.sb.main_blkaddr
                && (address as u64) < end
                && (address as u64) < self.sb.block_count,
            "F2FS allocated block outside main area",
        )?;
        let mut bytes = [0; 4096];
        self.source.read_at(address as u64 * 4096, &mut bytes)?;
        Ok(bytes)
    }
}

impl<S: BlockSource> FoldFrontend<S> for F2fs<S> {
    const TAG: &'static str = "f2fs";
    fn probe(source: &mut S) -> Result<bool> {
        for offset in [1024, 5120] {
            if source.len() >= offset + 4 {
                let mut bytes = [0; 4];
                source.read_at(offset, &mut bytes)?;
                if bytes == 0xf2f52010u32.to_le_bytes() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut selected = None;
        for offset in [1024, 5120] {
            let candidate = (|| -> Result<_> {
                let mut bytes = [0; 3072];
                source.read_at(offset, &mut bytes)?;
                let sb = Superblock::decode(&bytes)
                    .ok_or(FoldError::Corrupt("F2FS invalid superblock"))?;
                require(
                    sb.block_count <= source.len() / 4096
                        && sb.log_blocks_per_seg == 9
                        && sb.segment_count_ckpt == 2,
                    "F2FS unsupported/truncated geometry",
                )?;
                let regions = [
                    (sb.cp_blkaddr, sb.segment_count_ckpt),
                    (sb.sit_blkaddr, sb.segment_count_sit),
                    (sb.nat_blkaddr, sb.segment_count_nat),
                    (sb.ssa_blkaddr, sb.segment_count_ssa),
                    (sb.main_blkaddr, sb.segment_count_main),
                ];
                require(
                    sb.cp_blkaddr == sb.segment0_blkaddr
                        && sb.segment_count_nat > 0
                        && sb.segment_count_nat % 2 == 0
                        && sb.segment_count_sit % 2 == 0,
                    "F2FS metadata geometry",
                )?;
                for (index, (base, count)) in regions.iter().enumerate() {
                    let end = *base as u64 + *count as u64 * 512;
                    require(
                        end <= sb.block_count
                            && regions
                                .get(index + 1)
                                .is_none_or(|(next, _)| end == *next as u64),
                        "F2FS overlapping/out-of-volume metadata",
                    )?;
                }
                let feature = u32_at(&bytes, 0x884)?;
                if feature & !0x3fff != 0 || feature & (0x2 | 0x4000 | 0x8000) != 0 {
                    return Err(FoldError::Unsupported("F2FS zoned/unknown volume features"));
                }
                require(
                    bytes[0x899..0xab9].iter().all(|byte| *byte == 0),
                    "F2FS multiple devices unsupported",
                )?;
                if feature & 0x800 != 0 {
                    require(
                        u32_at(&bytes, 0x20)? == 3068
                            && crc32_raw(0xf2f52010, &bytes[..3068]) == u32_at(&bytes, 3068)?,
                        "F2FS superblock CRC",
                    )?;
                }
                let uuid = bytes[0x6c..0x7c]
                    .try_into()
                    .map_err(|_| FoldError::Corrupt("F2FS UUID"))?;
                Ok((sb, feature, uuid))
            })();
            if let Ok(candidate) = candidate {
                selected = Some(candidate);
                break;
            }
        }
        let (sb, feature, uuid) = selected.ok_or(FoldError::Corrupt(
            "F2FS neither superblock validates (or unsupported features)",
        ))?;
        let cp = Checkpoint::load(&mut source, &sb)?;
        let mut reader = Self {
            source,
            sb,
            cp,
            feature,
            uuid,
        };
        let root = reader.inode(reader.sb.root_ino)?;
        require(
            root.kind == FileKind::Directory,
            "F2FS root is not a directory",
        )?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        self.sb.root_ino as u64
    }
    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(dir, cx)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(u32::try_from(node).map_err(|_| FoldError::NotFound)?)
    }
    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(u32::try_from(node).map_err(|_| FoldError::NotFound)?)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
            mode: inode.mode as u32,
        })
    }
    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self
            .inode(u32::try_from(node).map_err(|_| FoldError::NotFound)?)?
            .times)
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let nid = u32::try_from(node).map_err(|_| FoldError::NotFound)?;
        let inode = self.inode(nid)?;
        require(
            matches!(inode.kind, FileKind::Regular | FileKind::Symlink),
            "F2FS entry has no file content",
        )?;
        self.content(nid, &inode, offset, buffer)
    }
    fn read_link(&mut self, node: NodeId, cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        if self.metadata(node, cx)?.kind != FileKind::Symlink {
            return Ok(None);
        }
        let size = self.metadata(node, cx)?.size;
        require(size <= 4096, "F2FS symlink length limit")?;
        let mut bytes = vec![0; size as usize];
        require(
            self.read_at(node, 0, &mut bytes, cx)? == bytes.len(),
            "F2FS short symlink",
        )?;
        Ok(Some(bytes))
    }
}
