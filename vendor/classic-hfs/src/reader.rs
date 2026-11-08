use crate::{
    be16, be32,
    btree::{self, TreeKind},
    catalog::Catalog,
    exact,
    fork::{Fork, Map, Overflow},
    metadata::{extents, Header},
    names, require,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, LocalFileTimes, Metadata, NodeId,
    Result, SubstrateCtx,
};
use std::collections::{BTreeMap, HashSet};

/// HFS files have independent data and resource forks. Finder aliases remain
/// ordinary files; neither fork is interpreted as a host link.
#[derive(Clone, Copy, Debug)]
pub enum ForkKind {
    Data,
    Resource,
}
impl ForkKind {
    fn byte(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Resource => 0xff,
        }
    }
}
pub struct Hfs<S> {
    source: S,
    header: Header,
    bitmap: Vec<u8>,
    catalog: Catalog,
    overflow: Overflow,
    // One recent fork only: walking a large volume cannot grow a file-map cache.
    cached: Option<(u32, u8, Map)>,
}
impl<S: BlockSource> Hfs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Preserve source name bytes, including names written under non-Roman
    /// Macintosh script systems (display and lookup assume MacRoman).
    pub fn raw_name(&self, node: NodeId) -> Result<&[u8]> {
        Ok(&self.catalog.get(node)?.name)
    }
    pub fn backup_local(&self, node: NodeId) -> Result<u32> {
        Ok(self.catalog.get(node)?.backup)
    }
    pub fn fork_size(&self, node: NodeId, kind: ForkKind) -> Result<u64> {
        Ok(u64::from(self.fork(node, kind)?.logical))
    }
    fn fork(&self, node: NodeId, kind: ForkKind) -> Result<&Fork> {
        let entry = self.catalog.get(node)?;
        match kind {
            ForkKind::Data => &entry.data,
            ForkKind::Resource => &entry.resource,
        }
        .as_ref()
        .ok_or(FoldError::Corrupt("hfs: directory has no fork"))
    }
    pub fn read_fork_at(
        &mut self,
        node: NodeId,
        kind: ForkKind,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let fork = self.fork(node, kind)?.clone();
        let id = u32::try_from(node).map_err(|_| FoldError::Corrupt("hfs: CNID overflow"))?;
        if self
            .cached
            .as_ref()
            .is_none_or(|(cached_id, cached_kind, _)| {
                *cached_id != id || *cached_kind != kind.byte()
            })
        {
            let map = Map::build(
                &self.header,
                &self.bitmap,
                &fork,
                id,
                kind.byte(),
                &self.overflow,
            )?;
            self.cached = Some((id, kind.byte(), map));
        }
        let count = u64::from(fork.logical)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count != 0 {
            let (_, _, map) = self
                .cached
                .as_ref()
                .ok_or(FoldError::Corrupt("hfs: missing cached fork"))?;
            map.read(&mut self.source, &self.header, offset, &mut bytes[..count])?;
        }
        Ok(count)
    }
}

/// Read metadata through inline + overflow extents. Extents-overflow bootstrap
/// follows only the header's linked leaves already covered by the known map;
/// an inaccessible self-describing record is an explicit error, never a scan.
fn tree(
    source: &mut impl BlockSource,
    header: &Header,
    bitmap: &[u8],
    size: u32,
    inline: [(u16, u16); 3],
    id: u32,
    overflow: &Overflow,
) -> Result<(Vec<u8>, Map)> {
    let mut map = Map::default();
    map.extend(header, bitmap, inline)?;
    let required = size.div_ceil(header.block_size);
    let mut prefix = Vec::new();
    let mut self_records = BTreeMap::new();
    let mut bootstrap_cursor = None;
    let mut seen = HashSet::new();
    while map.blocks < required {
        let start = u16::try_from(map.blocks)
            .map_err(|_| FoldError::Corrupt("hfs: tree overflow start"))?;
        let next = if id == 3 {
            let available = (u64::from(map.blocks) * u64::from(header.block_size))
                .min(u64::from(size)) as usize;
            let previous_length = prefix.len();
            prefix.resize(available, 0);
            map.read(
                source,
                header,
                previous_length as u64,
                &mut prefix[previous_length..],
            )?;
            let tree_header = btree::header(&prefix, TreeKind::Extents, size as usize)?;
            let (mut current, mut previous) = bootstrap_cursor.unwrap_or((tree_header.first, 0));
            while current != 0 && !self_records.contains_key(&start) {
                require(
                    current < tree_header.nodes && seen.insert(current),
                    "hfs: bootstrap leaf cycle/reference",
                )?;
                let leaf = btree::node(&prefix, current)?;
                require(
                    leaf.kind == 0xff && leaf.height == 1 && leaf.previous == previous,
                    "hfs: bootstrap leaf link/kind",
                )?;
                for record in leaf.records {
                    let (key, data) = btree::split(record, TreeKind::Extents)?;
                    if be32(key, 1)? == 3 && key[0] == 0 {
                        require(data.len() == 12, "hfs: short bootstrap extent")?;
                        require(
                            self_records
                                .insert(be16(key, 5)?, extents(data, 0)?)
                                .is_none(),
                            "hfs: duplicate bootstrap extent",
                        )?;
                    }
                }
                previous = current;
                current = leaf.next;
            }
            bootstrap_cursor = Some((current, previous));
            *self_records.get(&start).ok_or(FoldError::Corrupt(
                "hfs: unavailable self-overflow bootstrap record",
            ))?
        } else {
            *overflow
                .get(&(id, 0, start))
                .ok_or(FoldError::Corrupt("hfs: missing metadata overflow record"))?
        };
        map.extend(header, bitmap, next)?;
    }
    require(
        map.blocks == required,
        "hfs: metadata file extent count/length mismatch",
    )?;
    let previous_length = prefix.len();
    prefix.resize(size as usize, 0);
    map.read(
        source,
        header,
        previous_length as u64,
        &mut prefix[previous_length..],
    )?;
    Ok((prefix, map))
}
impl<S: BlockSource> FoldFrontend<S> for Hfs<S> {
    const TAG: &'static str = "hfs";
    fn probe(source: &mut S) -> Result<bool> {
        if source.len() < 1536 {
            return Ok(false);
        }
        let mut bytes = [0; 512];
        exact(source, 1024, &mut bytes)?;
        Ok(Header::parse(&bytes).is_ok_and(|header| header.validate_source(source.len()).is_ok()))
    }
    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut bytes = [0; 512];
        exact(&mut source, 1024, &mut bytes)?;
        let header = Header::parse(&bytes)?;
        header.validate_source(source.len())?;
        let mut bitmap = vec![0; usize::from(header.block_count).div_ceil(8)];
        exact(&mut source, header.bitmap_offset, &mut bitmap)?;
        let (extents_bytes, extents_map) = tree(
            &mut source,
            &header,
            &bitmap,
            header.extents_size,
            header.extents,
            3,
            &Overflow::new(),
        )?;
        let mut overflow = Overflow::new();
        for (key, value) in btree::leaves(&extents_bytes, TreeKind::Extents)? {
            require(value.len() == 12, "hfs: overflow record length")?;
            require(
                overflow
                    .insert((be32(key, 1)?, key[0], be16(key, 5)?), extents(value, 0)?)
                    .is_none(),
                "hfs: duplicate overflow record",
            )?;
        }
        let verified = Map::build(
            &header,
            &bitmap,
            &Fork {
                logical: header.extents_size,
                physical: header.extents_size.div_ceil(header.block_size) * header.block_size,
                extents: header.extents,
            },
            3,
            0,
            &overflow,
        )?;
        require(
            verified.extents == extents_map.extents,
            "hfs: bootstrap/rooted overflow map mismatch",
        )?;
        drop(extents_bytes);
        let (bytes, _) = tree(
            &mut source,
            &header,
            &bitmap,
            header.catalog_size,
            header.catalog,
            4,
            &overflow,
        )?;
        let catalog = Catalog::parse(&bytes)?;
        Ok(Self {
            source,
            header,
            bitmap,
            catalog,
            overflow,
            cached: None,
        })
    }
    fn root(&self) -> NodeId {
        2
    }
    fn lookup(
        &mut self,
        directory: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        require(
            self.catalog.get(directory)?.directory,
            "hfs: lookup on file",
        )?;
        let Some(raw) = names::encode(name) else {
            return Ok(None);
        };
        for id in self
            .catalog
            .children
            .get(&(directory as u32))
            .into_iter()
            .flatten()
        {
            if names::compare(&self.catalog.get(u64::from(*id))?.name, &raw).is_eq() {
                return Ok(Some(u64::from(*id)));
            }
        }
        Ok(None)
    }
    fn read_dir(
        &mut self,
        directory: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Vec<DirEntry>> {
        require(
            self.catalog.get(directory)?.directory,
            "hfs: listing on file",
        )?;
        self.catalog
            .children
            .get(&(directory as u32))
            .into_iter()
            .flatten()
            .map(|id| {
                let entry = self.catalog.get(u64::from(*id))?;
                Ok(DirEntry {
                    name: names::display(&entry.name),
                    node: u64::from(*id),
                    kind: if entry.directory {
                        FileKind::Directory
                    } else {
                        FileKind::Regular
                    },
                })
            })
            .collect()
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let entry = self.catalog.get(node)?;
        Ok(Metadata {
            kind: if entry.directory {
                FileKind::Directory
            } else {
                FileKind::Regular
            },
            size: entry
                .data
                .as_ref()
                .map_or(0, |fork| u64::from(fork.logical)),
            mode: 0,
        })
    }
    fn local_timestamps(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<LocalFileTimes> {
        let entry = self.catalog.get(node)?;
        let local = |raw| (raw != 0).then_some((i64::from(raw) - 2_082_844_800, 0));
        Ok(LocalFileTimes {
            modified: local(entry.modified),
            created: local(entry.created),
            ..Default::default()
        })
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.read_fork_at(node, ForkKind::Data, offset, bytes)
    }
}
