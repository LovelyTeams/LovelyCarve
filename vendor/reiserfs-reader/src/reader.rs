use crate::{
    exact,
    objects::{self, Content, Directory, Segment, Stat},
    require,
    tree::{Item, Tree},
    Header,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::VecDeque;

/// Immutable volume with bounded bitmap, tree-page and inode caches. The last
/// directory and file map are retained; unrelated objects are never scanned.
pub struct ReiserFs<S> {
    source: S,
    header: Header,
    tree: Tree,
    stats: VecDeque<(u64, Stat)>,
    directory: Option<(u64, Directory)>,
    file: Option<(u64, Vec<Segment>)>,
}
impl<S: BlockSource> ReiserFs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    fn object(&mut self, node: u64) -> Result<(Stat, Vec<Item>)> {
        let items = self.tree.object(&mut self.source, &self.header, node)?;
        let stat = Stat::parse(&items)?;
        if !self.stats.iter().any(|(cached, _)| *cached == node) {
            if self.stats.len() == 128 {
                self.stats.pop_front();
            }
            self.stats.push_back((node, stat.clone()));
        }
        Ok((stat, items))
    }
    fn stat(&mut self, node: u64) -> Result<Stat> {
        if let Some((_, stat)) = self.stats.iter().find(|(cached, _)| *cached == node) {
            return Ok(stat.clone());
        }
        let items = self.tree.stat(&mut self.source, &self.header, node)?;
        let stat = Stat::parse(&items)?;
        if self.stats.len() == 128 {
            self.stats.pop_front();
        }
        self.stats.push_back((node, stat.clone()));
        Ok(stat)
    }
    fn ensure_directory(&mut self, node: u64) -> Result<()> {
        if self
            .directory
            .as_ref()
            .is_some_and(|(cached, _)| *cached == node)
        {
            return Ok(());
        }
        let (stat, items) = self.object(node)?;
        let directory = objects::directory(node, &stat, &items)?;
        self.directory = Some((node, directory));
        Ok(())
    }
    fn directory(&self) -> Result<&Directory> {
        self.directory
            .as_ref()
            .map(|(_, directory)| directory)
            .ok_or(FoldError::Corrupt("reiserfs: missing directory cache"))
    }
    fn ensure_file(&mut self, node: u64) -> Result<Stat> {
        if self
            .file
            .as_ref()
            .is_some_and(|(cached, _)| *cached == node)
        {
            return self.stat(node);
        }
        let (stat, items) = self.object(node)?;
        let segments = objects::content(&stat, &items, self.header.block_size)?;
        for segment in &segments {
            if let Content::Block(block) = segment.content {
                if block != 0 {
                    let count = segment.length.div_ceil(u64::from(self.header.block_size));
                    require(
                        u64::from(block) + count <= u64::from(self.header.blocks),
                        "reiserfs: content run beyond volume",
                    )?;
                    for position in u64::from(block)..u64::from(block) + count {
                        self.tree
                            .allocated(&mut self.source, &self.header, position as u32)?;
                    }
                }
            }
        }
        self.file = Some((node, segments));
        Ok(stat)
    }
}
impl<S: BlockSource> FoldFrontend<S> for ReiserFs<S> {
    const TAG: &'static str = "reiserfs";
    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source).is_ok())
    }
    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?;
        let mut reader = Self {
            source,
            header,
            tree: Tree::new(),
            stats: VecDeque::new(),
            directory: None,
            file: None,
        };
        reader.ensure_directory(reader.root())?;
        require(
            reader.directory()?.parent == 1,
            "reiserfs: root parent identity",
        )?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        (1u64 << 32) | 2
    }
    fn lookup(
        &mut self,
        node: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        self.ensure_directory(node)?;
        Ok(self
            .directory()?
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.ensure_directory(node)?;
        let mut entries = self.directory()?.entries.clone();
        for entry in &mut entries {
            entry.kind = self.stat(entry.node)?.kind();
            if entry.kind == FileKind::Directory {
                let (stat, items) = self.object(entry.node)?;
                let child = objects::directory(entry.node, &stat, &items)?;
                require(
                    child.parent == node,
                    "reiserfs: directory parent/cycle mismatch",
                )?;
            }
        }
        Ok(entries)
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let stat = self.stat(node)?;
        Ok(Metadata {
            kind: stat.kind(),
            size: stat.size,
            mode: stat.mode,
        })
    }
    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.stat(node)?.times)
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let stat = self.ensure_file(node)?;
        let count = stat.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let (_, segments) = self
            .file
            .as_ref()
            .ok_or(FoldError::Corrupt("reiserfs: missing file map"))?;
        let end = offset + count as u64;
        let first = segments.partition_point(|segment| segment.start + segment.length <= offset);
        let mut written = 0;
        for segment in &segments[first..] {
            if segment.start >= end {
                break;
            }
            let start = offset.max(segment.start);
            let stop = end.min(segment.start + segment.length);
            let length = (stop - start) as usize;
            let target = &mut bytes[(start - offset) as usize..(stop - offset) as usize];
            match &segment.content {
                Content::Direct(body) => target.copy_from_slice(
                    &body[(start - segment.start) as usize..(stop - segment.start) as usize],
                ),
                Content::Block(0) => target.fill(0),
                Content::Block(block) => exact(
                    &mut self.source,
                    u64::from(*block) * u64::from(self.header.block_size) + start - segment.start,
                    target,
                )?,
            }
            written += length;
        }
        require(written == count, "reiserfs: short logical read")?;
        Ok(count)
    }
    fn read_link(
        &mut self,
        node: NodeId,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let stat = self.stat(node)?;
        if stat.kind() != FileKind::Symlink {
            return Ok(None);
        }
        require(
            stat.size > 0 && stat.size <= u64::from(self.header.block_size),
            "reiserfs: symlink size",
        )?;
        let mut bytes = vec![0; stat.size as usize];
        let count = self.read_at(node, 0, &mut bytes, context)?;
        require(
            count == bytes.len() && !bytes.contains(&0),
            "reiserfs: short/invalid symlink target",
        )?;
        Ok(Some(bytes))
    }
}
