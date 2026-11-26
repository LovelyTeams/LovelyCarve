//! Positioned, read-only Red Hat GFS1 volume access.
#![forbid(unsafe_code)]

mod allocation;
mod directory;
mod format;
mod journal;
mod stream;

use std::collections::VecDeque;
use std::sync::Arc;

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

use allocation::Allocation;
use directory::{Directory, Entry};
pub use format::{Header, SUPERBLOCK_BYTES, SUPERBLOCK_OFFSET};
use format::{Inode, JINDEX_FORMAT, Number, RINDEX_FORMAT, require};
use stream::Stream;

pub struct Gfs1<S> {
    source: S,
    header: Header,
    allocation: Allocation,
    inodes: VecDeque<(NodeId, Arc<Inode>)>,
    directory: Option<(NodeId, Directory)>,
    stream: Option<(NodeId, Stream)>,
}

fn read_index(
    source: &mut impl BlockSource,
    header: &Header,
    allocation: Option<&mut Allocation>,
    number: Number,
    format: u32,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut allocation = allocation;
    if let Some(allocation) = allocation.as_deref_mut() {
        allocation.check(source, header, number.block, true)?;
    }
    let inode = Inode::read(source, header, number.block)?;
    require(
        inode.number == number
            && inode.kind == FileKind::Regular
            && inode.payload_format == format
            && inode.size <= limit as u64,
        "invalid GFS1 system index inode",
    )?;
    let stream = Stream::parse(source, header, &inode, allocation)?;
    require(stream.complete(), "GFS1 system index contains a hole")?;
    let mut raw = vec![0; inode.size as usize];
    require(
        stream.read(source, header, 0, &mut raw)? == raw.len(),
        "short GFS1 system index read",
    )?;
    Ok(raw)
}

impl<S: BlockSource> Gfs1<S> {
    fn inode(&mut self, id: NodeId) -> Result<Arc<Inode>> {
        if let Some((_, inode)) = self.inodes.iter().find(|(cached, _)| id == *cached) {
            return Ok(Arc::clone(inode));
        }
        self.allocation
            .check(&mut self.source, &self.header, id, true)?;
        let inode = Arc::new(Inode::read(&mut self.source, &self.header, id)?);
        if self.inodes.len() == 64 {
            self.inodes.pop_front();
        }
        self.inodes.push_back((id, Arc::clone(&inode)));
        Ok(inode)
    }

    fn directory(&mut self, node: NodeId) -> Result<&Directory> {
        if self
            .directory
            .as_ref()
            .is_none_or(|(owner, _)| node != *owner)
        {
            let inode = self.inode(node)?;
            let content =
                directory::read(&mut self.source, &self.header, &mut self.allocation, &inode)?;
            self.directory = Some((node, content));
        }
        Ok(&self.directory.as_ref().expect("directory loaded above").1)
    }

    fn validate_entry(&mut self, entry: &Entry) -> Result<()> {
        let inode = self.inode(entry.number.block)?;
        require(
            inode.number == entry.number && inode.raw_kind == entry.raw_kind,
            "GFS1 directory reference disagrees with its inode",
        )
    }

    fn read_content(&mut self, inode: &Inode, offset: u64, output: &mut [u8]) -> Result<usize> {
        if self
            .stream
            .as_ref()
            .is_none_or(|(owner, _)| inode.number.block != *owner)
        {
            let stream = Stream::parse(
                &mut self.source,
                &self.header,
                inode,
                Some(&mut self.allocation),
            )?;
            if inode.kind == FileKind::Symlink {
                require(stream.complete(), "GFS1 symbolic link contains a hole")?;
            }
            self.stream = Some((inode.number.block, stream));
        }
        self.stream.as_ref().expect("stream loaded above").1.read(
            &mut self.source,
            &self.header,
            offset,
            output,
        )
    }
}

impl<S: BlockSource> FoldFrontend<S> for Gfs1<S> {
    const TAG: &'static str = "gfs1";

    fn probe(source: &mut S) -> Result<bool> {
        if source.len() < SUPERBLOCK_OFFSET + SUPERBLOCK_BYTES as u64 {
            return Ok(false);
        }
        let mut raw = [0; SUPERBLOCK_BYTES];
        source.read_at(SUPERBLOCK_OFFSET, &mut raw)?;
        Ok(Header::inspect_prefix(&raw, source.len()).is_ok())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::inspect(&mut source)?;
        let limit = allocation::INDEX_RECORD * allocation::INDEX_LIMIT;
        let resource_index = read_index(
            &mut source,
            &header,
            None,
            header.rindex,
            RINDEX_FORMAT,
            limit,
        )?;
        let mut allocation = Allocation::parse(&mut source, &header, &resource_index)?;
        require(
            resource_index
                == read_index(
                    &mut source,
                    &header,
                    Some(&mut allocation),
                    header.rindex,
                    RINDEX_FORMAT,
                    limit,
                )?,
            "inconsistent GFS1 resource index bootstrap",
        )?;
        let journal_index = read_index(
            &mut source,
            &header,
            Some(&mut allocation),
            header.jindex,
            JINDEX_FORMAT,
            journal::INDEX_RECORD * journal::INDEX_LIMIT,
        )?;
        journal::validate(&mut source, &header, &allocation, &journal_index)?;
        let mut fs = Self {
            source,
            header,
            allocation,
            inodes: VecDeque::new(),
            directory: None,
            stream: None,
        };
        let root = fs.inode(fs.header.root.block)?;
        require(
            root.number == fs.header.root && root.kind == FileKind::Directory,
            "invalid GFS1 root inode",
        )?;
        Ok(fs)
    }

    fn root(&self) -> NodeId {
        self.header.root.block
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        let Some(entry) = self.directory(dir)?.get(name).cloned() else {
            return Ok(None);
        };
        self.validate_entry(&entry)?;
        Ok(Some(entry.number.block))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let entries = self.directory(dir)?.clone();
        entries
            .into_iter()
            .map(|(name, entry)| {
                self.validate_entry(&entry)?;
                Ok(DirEntry {
                    name,
                    node: entry.number.block,
                    kind: entry.kind,
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
            mode: inode.mode,
        })
    }

    fn inode_number(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<u64> {
        Ok(self.inode(node)?.number.formal)
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.inode(node)?.times)
    }

    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.kind != FileKind::Symlink {
            return Ok(None);
        }
        require(
            (1..=65536).contains(&inode.size),
            "invalid GFS1 symbolic link length",
        )?;
        let mut raw = vec![0; inode.size as usize];
        require(
            self.read_content(&inode, 0, &mut raw)? == raw.len() && !raw.contains(&0),
            "invalid GFS1 symbolic link target",
        )?;
        Ok(Some(raw))
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        match inode.kind {
            FileKind::Directory => Err(FoldError::IsDirectory),
            FileKind::Regular | FileKind::Symlink => self.read_content(&inode, offset, bytes),
            _ => Err(FoldError::Unsupported("GFS1 special file content")),
        }
    }
}

#[cfg(test)]
mod tests;
