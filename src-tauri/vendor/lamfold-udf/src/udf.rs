//! Volume descriptor chain and the lamfold frontend; based on upstream udf.rs.
use crate::{descriptors::*, inode::UdfInode};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec,
    vec::Vec,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

/// Mounted UDF physical partition. Node handles are partition-relative ICB LBAs,
/// so bounded cache eviction cannot change identity or hard-link relationships.
pub struct Udf<S: BlockSource> {
    pub(crate) src: S,
    pub(crate) block_size: u32,
    pub(crate) partition_start: u32,
    pub(crate) partition_length: u32,
    root: u32,
    nodes: BTreeMap<u32, UdfInode>,
    cached_bytes: usize,
}

impl<S: BlockSource> Udf<S> {
    pub(crate) fn read_block(&mut self, block: u32) -> Result<Vec<u8>> {
        let mut bytes = vec![0; self.block_size as usize];
        exact(
            &mut self.src,
            u64::from(block) * u64::from(self.block_size),
            &mut bytes,
        )?;
        Ok(bytes)
    }
    pub(crate) fn physical(&self, block: u32, length: u64) -> Result<u64> {
        let start = u64::from(block) * u64::from(self.block_size);
        require(
            start.checked_add(length).is_some_and(|end| {
                end <= u64::from(self.partition_length) * u64::from(self.block_size)
            }),
            "udf: extent outside physical partition",
        )?;
        Ok(u64::from(self.partition_start) * u64::from(self.block_size) + start)
    }
    pub(crate) fn partition_block(&mut self, block: u32) -> Result<Vec<u8>> {
        let offset = self.physical(block, u64::from(self.block_size))?;
        let mut bytes = vec![0; self.block_size as usize];
        exact(&mut self.src, offset, &mut bytes)?;
        Ok(bytes)
    }
    pub(crate) fn icb(&self, bytes: &[u8]) -> Result<u32> {
        require(
            u16_at(bytes, 8)? == 0,
            "udf: invalid ICB partition reference",
        )?;
        let length = u32_at(bytes, 0)?;
        require(
            length == self.block_size,
            "udf: unsupported ICB extent length/type",
        )?;
        let block = u32_at(bytes, 4)?;
        self.physical(block, u64::from(length))?;
        Ok(block)
    }
    pub(crate) fn inode(&mut self, node: NodeId) -> Result<UdfInode> {
        let block = u32::try_from(node).map_err(|_| FoldError::NotFound)?;
        if let Some(inode) = self.nodes.get(&block) {
            return Ok(inode.clone());
        }
        let inode = self.read_fe(block)?;
        let cost = inode.cache_cost();
        if self.nodes.len() >= 4096 || self.cached_bytes + cost > 16 * 1024 * 1024 {
            self.nodes.clear();
            self.cached_bytes = 0;
        }
        self.cached_bytes += cost;
        self.nodes.insert(block, inode.clone());
        Ok(inode)
    }
    fn volume_sequence(&mut self, location: u32, length: u32) -> Result<()> {
        require(
            length > 0
                && length.is_multiple_of(self.block_size)
                && length / self.block_size <= 1024,
            "udf: volume descriptor sequence length/limit",
        )?;
        let mut partition: Option<(u32, Vec<u8>)> = None;
        let mut logical: Option<(u32, Vec<u8>)> = None;
        let mut terminated = false;
        for index in 0..length / self.block_size {
            let block = location
                .checked_add(index)
                .ok_or(FoldError::Corrupt("udf: VDS address overflow"))?;
            let bytes = self.read_block(block)?;
            match descriptor(&bytes, block, 16)? {
                5 => {
                    descriptor(&bytes, block, 512)?;
                    let sequence = u32_at(&bytes, 16)?;
                    if let Some((_, previous)) = &partition {
                        require(
                            u16_at(previous, 22)? == u16_at(&bytes, 22)?,
                            "udf: multiple physical partitions",
                        )?;
                    }
                    if partition.as_ref().is_none_or(|(old, _)| sequence >= *old) {
                        partition = Some((sequence, bytes));
                    }
                }
                6 => {
                    descriptor(&bytes, block, 440)?;
                    let sequence = u32_at(&bytes, 16)?;
                    if logical.as_ref().is_none_or(|(old, _)| sequence >= *old) {
                        logical = Some((sequence, bytes));
                    }
                }
                8 => {
                    terminated = true;
                    break;
                }
                3 => {
                    return Err(FoldError::Unsupported(
                        "udf: chained volume descriptor sequences",
                    ))
                }
                1 | 4 | 7 => {}
                _ => return Err(FoldError::Unsupported("udf: unknown volume descriptor")),
            }
        }
        require(terminated, "udf: unterminated volume descriptor sequence")?;
        let (_, partition) =
            partition.ok_or(FoldError::Corrupt("udf: missing partition descriptor"))?;
        let (_, logical) =
            logical.ok_or(FoldError::Corrupt("udf: missing logical volume descriptor"))?;
        require(
            u16_at(&partition, 20)? & 1 != 0
                && matches!(partition.get(25..31), Some(b"+NSR02") | Some(b"+NSR03")),
            "udf: partition flags/contents",
        )?;
        require(
            u32_at(&logical, 212)? == self.block_size,
            "udf: logical/physical block sizes disagree",
        )?;
        domain(&logical[216..248])?;
        if u32_at(&logical, 264)? != 6
            || u32_at(&logical, 268)? != 1
            || logical.get(440..442) != Some(&[1, 6])
        {
            return Err(FoldError::Unsupported("udf: only a single Type-1 physical partition map is supported (no VAT/sparable/metadata maps)"));
        }
        // Include the complete partition-map table in CRC coverage.
        descriptor(&logical, u32_at(&logical, 12)?, 446)?;
        require(
            u16_at(&logical, 442)? == 1 && u16_at(&logical, 444)? == u16_at(&partition, 22)?,
            "udf: physical partition map identity",
        )?;
        self.partition_start = u32_at(&partition, 188)?;
        self.partition_length = u32_at(&partition, 192)?;
        require(
            self.partition_length > 0
                && (u64::from(self.partition_start) + u64::from(self.partition_length))
                    * u64::from(self.block_size)
                    <= self.src.len(),
            "udf: physical partition exceeds source",
        )?;
        let fsd_block = self.icb(&logical[248..264])?;
        let fsd = self.partition_block(fsd_block)?;
        require(
            descriptor(&fsd, fsd_block, 512)? == 256,
            "udf: expected file set descriptor",
        )?;
        domain(&fsd[416..448])?;
        if u32_at(&fsd, 448)? != 0 {
            return Err(FoldError::Unsupported("udf: multiple file sets"));
        }
        self.root = self.icb(&fsd[400..416])?;
        require(
            self.inode(u64::from(self.root))?.metadata.kind == FileKind::Directory,
            "udf: root is not a directory",
        )
    }
    fn mount_anchor(&mut self, block_size: u32, anchor: u32) -> Result<()> {
        self.block_size = block_size;
        let bytes = self.read_block(anchor)?;
        require(valid_anchor(&bytes, anchor), "udf: invalid anchor")?;
        let mut failure = FoldError::Corrupt("udf: no usable volume descriptor sequence");
        for offset in [16, 24] {
            self.nodes.clear();
            self.cached_bytes = 0;
            let length = u32_at(&bytes, offset)?;
            if length == 0 {
                continue;
            }
            match self.volume_sequence(u32_at(&bytes, offset + 4)?, length) {
                Ok(()) => return Ok(()),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
    fn directory(&mut self, node: NodeId) -> Result<Vec<DirEntry>> {
        let inode = self.inode(node)?;
        if inode.metadata.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        require(
            inode.metadata.size <= 32 * 1024 * 1024,
            "udf: directory byte limit",
        )?;
        let mut data = vec![0; inode.metadata.size as usize];
        self.content(&inode, 0, &mut data)?;
        let mut position = 0;
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        while position < data.len() {
            let tail = &data[position..];
            // A directory's information length excludes sector padding. Invalid
            // records must not silently terminate enumeration of later entries.
            require(
                tail.len() >= 38,
                "udf: truncated file identifier descriptor",
            )?;
            let length = 38 + usize::from(u16_at(tail, 36)?) + usize::from(tail[19]);
            let padded = (length + 3) & !3;
            let record = tail
                .get(..padded)
                .ok_or(FoldError::Corrupt("udf: FID length/padding"))?;
            let location = inode.data.location(position as u64, self.block_size)?;
            require(
                descriptor(record, location, length)? == 257,
                "udf: invalid file identifier descriptor",
            )?;
            if record[18] & 0x0c == 0 {
                require(entries.len() < 262144, "udf: directory entry limit")?;
                require(
                    record[18] & !0x1f == 0 && u16_at(record, 16)? == 1,
                    "udf: FID version/flags",
                )?;
                let name = decode_name(&record[length - usize::from(record[19])..length])?;
                safe_component(&name)?;
                require(names.insert(name.clone()), "udf: duplicate directory name")?;
                let child = self.icb(&record[20..36])?;
                let metadata = self.inode(u64::from(child))?.metadata;
                require(
                    (record[18] & 2 != 0) == (metadata.kind == FileKind::Directory),
                    "udf: FID and inode types disagree",
                )?;
                entries.push(DirEntry {
                    name,
                    node: u64::from(child),
                    kind: metadata.kind,
                });
            }
            position += padded;
        }
        Ok(entries)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Udf<S> {
    const TAG: &'static str = "udf";
    fn probe(source: &mut S) -> Result<bool> {
        for block_size in BLOCK_SIZES {
            for location in anchor_locations(source.len(), block_size) {
                let Ok(location) = u32::try_from(location) else {
                    continue;
                };
                let offset = u64::from(location) * u64::from(block_size);
                if offset + 512 > source.len() {
                    continue;
                }
                let mut bytes = [0; 512];
                exact(source, offset, &mut bytes)?;
                if valid_anchor(&bytes, location) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn open(source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut reader = Self {
            src: source,
            block_size: 2048,
            partition_start: 0,
            partition_length: 0,
            root: 0,
            nodes: BTreeMap::new(),
            cached_bytes: 0,
        };
        let mut failure = FoldError::Corrupt("udf: no valid anchor");
        for block_size in BLOCK_SIZES {
            for location in anchor_locations(reader.src.len(), block_size) {
                let Ok(location) = u32::try_from(location) else {
                    continue;
                };
                let offset = u64::from(location) * u64::from(block_size);
                if offset + 512 > reader.src.len() {
                    continue;
                }
                let mut bytes = [0; 512];
                exact(&mut reader.src, offset, &mut bytes)?;
                if valid_anchor(&bytes, location) {
                    match reader.mount_anchor(block_size, location) {
                        Ok(()) => return Ok(reader),
                        Err(error) => failure = error,
                    }
                }
            }
        }
        Err(failure)
    }
    fn root(&self) -> NodeId {
        u64::from(self.root)
    }
    fn lookup(
        &mut self,
        directory: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .directory(directory)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(node)
    }
    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        Ok(self.inode(node)?.metadata)
    }
    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.inode(node)?.times)
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        require(
            matches!(inode.metadata.kind, FileKind::Regular | FileKind::Symlink),
            "udf: inode has no file content",
        )?;
        self.content(&inode, offset, bytes)
    }
    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.metadata.kind != FileKind::Symlink {
            return Ok(None);
        }
        let mut bytes = vec![0; inode.metadata.size as usize];
        self.content(&inode, 0, &mut bytes)?;
        Ok(Some(bytes))
    }
}
