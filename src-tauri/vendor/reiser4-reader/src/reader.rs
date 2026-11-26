use crate::{
    cluster::{self, CCREG},
    format::BLOCK_SIZE,
    object::{file_segments, Content, Directory, FilePlugins, Segment, Stat},
    require,
    tree::{Item, Key, Tree, COMPRESSED_TAIL},
    Header,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::{collections::VecDeque, sync::Arc};

pub struct Reiser4<S> {
    source: S,
    header: Header,
    tree: Tree,
    root: NodeId,
    root_plugins: FilePlugins,
    stats: VecDeque<(NodeId, Arc<Stat>)>,
    directory: Option<(NodeId, Arc<Directory>)>,
    file: Option<(NodeId, Vec<Segment>)>,
}

impl<S: BlockSource> Reiser4<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }

    fn remember_stat(&mut self, item: &Item) -> Result<(NodeId, Arc<Stat>)> {
        if let Some((_, stat)) = self.stats.iter().find(|(node, _)| *node == item.id) {
            return Ok((item.id, Arc::clone(stat)));
        }
        let stat = Arc::new(Stat::parse(item)?);
        if self.stats.len() == 256 {
            self.stats.pop_front();
        }
        self.stats.push_back((item.id, Arc::clone(&stat)));
        Ok((item.id, stat))
    }

    fn stat(&mut self, node: NodeId) -> Result<Arc<Stat>> {
        if let Some((_, stat)) = self.stats.iter().find(|(id, _)| *id == node) {
            return Ok(Arc::clone(stat));
        }
        let item = self.tree.stat_at(&mut self.source, &self.header, node)?;
        let (reachable, stat) = self.resolve(item.key)?;
        require(
            reachable == node,
            "reiser4: stat-data item is not reachable from the root",
        )?;
        Ok(stat)
    }

    fn resolve(&mut self, key: Key) -> Result<(NodeId, Arc<Stat>)> {
        let items = self.tree.items(&mut self.source, &self.header, key..=key)?;
        match items.as_slice() {
            [item] => self.remember_stat(item),
            [] => Err(FoldError::NotFound),
            _ => Err(FoldError::Corrupt("reiser4: duplicate stat-data key")),
        }
    }

    fn directory(&mut self, node: NodeId) -> Result<Arc<Directory>> {
        if let Some((cached, directory)) = &self.directory {
            if *cached == node {
                return Ok(Arc::clone(directory));
            }
        }
        let stat = self.stat(node)?;
        let directory = Arc::new(self.directory_for(&stat)?);
        self.directory = Some((node, Arc::clone(&directory)));
        Ok(directory)
    }

    fn directory_for(&mut self, stat: &Stat) -> Result<Directory> {
        require(
            stat.metadata.kind == FileKind::Directory,
            "reiser4: node is not a directory",
        )?;
        let items = self
            .tree
            .items(&mut self.source, &self.header, stat.key.directory())?;
        Directory::parse(stat.key, self.header.large_keys, &items)
    }

    fn resolve_child(&mut self, parent: Key, key: Key) -> Result<(NodeId, FileKind)> {
        let (node, stat) = self.resolve(key).map_err(|error| match error {
            FoldError::NotFound => FoldError::Corrupt("reiser4: directory target is missing"),
            error => error,
        })?;
        if stat.metadata.kind == FileKind::Directory {
            require(
                key != parent && self.directory_for(&stat)?.parent == parent,
                "reiser4: directory parent mismatch or cycle",
            )?;
        }
        Ok((node, stat.metadata.kind))
    }

    fn ensure_file(&mut self, node: NodeId, stat: &Stat) -> Result<()> {
        if self
            .file
            .as_ref()
            .is_some_and(|(cached, _)| *cached == node)
        {
            return Ok(());
        }
        if stat.encrypted {
            return Err(FoldError::Unsupported("reiser4: encrypted file"));
        }
        require(
            stat.metadata.kind == FileKind::Regular,
            "reiser4: node has no regular file data",
        )?;
        let items = self
            .tree
            .items(&mut self.source, &self.header, stat.key.file_body())?;
        let has_clusters = items.iter().any(|item| item.plugin == COMPRESSED_TAIL);
        let clustered = stat.plugins.object == Some(CCREG) || has_clusters;
        let segments = if clustered {
            if stat.plugins.cipher.or(self.root_plugins.cipher).unwrap_or(0) != 0 {
                return Err(FoldError::Unsupported("reiser4: encrypted file"));
            }
            require(
                stat.plugins.object.is_none_or(|object| object == CCREG),
                "reiser4: cluster items on a non-clustered file",
            )?;
            cluster::file_segments(items, stat.metadata.size, stat.plugins, self.root_plugins)?
        } else {
            file_segments(items, stat.metadata.size)?
        };
        let mut physical = Vec::new();
        for part in &segments {
            if let Content::Blocks(block) = part.content {
                let blocks = (part.end - part.start).div_ceil(BLOCK_SIZE as u64);
                self.tree
                    .allocation
                    .check(&mut self.source, &self.header, block, blocks)?;
                physical.push((block, block + blocks));
            }
        }
        physical.sort_unstable();
        require(
            physical.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "reiser4: overlapping physical extents",
        )?;
        self.file = Some((node, segments));
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Reiser4<S> {
    const TAG: &'static str = "reiser4";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source).is_ok())
    }

    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?;
        header.check_clean(&mut source)?;
        let mut volume = Self {
            source,
            header,
            tree: Tree::default(),
            root: 0,
            root_plugins: FilePlugins::default(),
            stats: VecDeque::new(),
            directory: None,
            file: None,
        };
        let (root, stat) = volume.resolve(Key::ROOT)?;
        volume.root = root;
        volume.root_plugins = stat.plugins;
        require(
            volume.directory(root)?.parent == Key::ROOT,
            "reiser4: root parent key",
        )?;
        Ok(volume)
    }

    fn root(&self) -> NodeId {
        self.root
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        let stat = self.stat(dir)?;
        let entries = self.directory(dir)?;
        entries
            .entries
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, key)| self.resolve_child(stat.key, *key).map(|(node, _)| node))
            .transpose()
    }

    fn read_dir(&mut self, dir: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let stat = self.stat(dir)?;
        let directory = self.directory(dir)?;
        directory
            .entries
            .iter()
            .map(|(name, key)| {
                let (node, kind) = self.resolve_child(stat.key, *key)?;
                Ok(DirEntry {
                    name: name.clone(),
                    node,
                    kind,
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        Ok(self.stat(node)?.metadata.clone())
    }

    fn inode_number(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<u64> {
        Ok(self.stat(node)?.key.object_number())
    }

    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.stat(node)?.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        output: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let stat = self.stat(node)?;
        let len = stat
            .metadata
            .size
            .saturating_sub(offset)
            .min(output.len() as u64) as usize;
        if let Some(link) = &stat.target {
            if len != 0 {
                output[..len].copy_from_slice(&link[offset as usize..offset as usize + len]);
            }
            return Ok(len);
        }
        self.ensure_file(node, &stat)?;
        if len == 0 {
            return Ok(0);
        }
        let (_, segments) = self
            .file
            .as_ref()
            .ok_or(FoldError::Corrupt("reiser4: missing file ranges"))?;
        let end = offset + len as u64;
        let first = segments.partition_point(|part| part.end <= offset);
        let mut written = 0;
        for part in &segments[first..] {
            if part.start >= end {
                break;
            }
            let start = offset.max(part.start);
            let stop = end.min(part.end);
            let target = &mut output[(start - offset) as usize..(stop - offset) as usize];
            let local = start - part.start;
            match &part.content {
                Content::Hole => target.fill(0),
                Content::Inline(bytes) => {
                    target.copy_from_slice(&bytes[local as usize..local as usize + target.len()])
                }
                Content::Compressed(codec, payload) => {
                    let bytes = codec.decode(payload, (part.end - part.start) as usize)?;
                    target.copy_from_slice(&bytes[local as usize..local as usize + target.len()]);
                }
                Content::Blocks(block) => self
                    .source
                    .read_at(block * BLOCK_SIZE as u64 + local, target)?,
            }
            written += target.len();
        }
        require(written == len, "reiser4: short logical read")?;
        Ok(written)
    }

    fn read_link(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        Ok(self.stat(node)?.target.clone())
    }
}
