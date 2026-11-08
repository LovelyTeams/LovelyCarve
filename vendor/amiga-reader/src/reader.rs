use crate::{allocation::Allocation, convert, require, Geometry, Hints, Source};
use amiga_ffs::{BlockSource as _, DateStamp, Entry, EntryKind, FileChain, Volume};
use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, LocalFileTimes, Metadata, NodeId,
    Result, SubstrateCtx,
};
use std::collections::{HashMap, HashSet, VecDeque};

/// Immutable OFS/FFS. Metadata and name-entry caches are bounded; the last
/// directory and file chain are retained for repeated range requests.
pub struct Amiga<S: BlockSource> {
    volume: Volume<Source<S>>,
    geometry: Geometry,
    allocation: Allocation,
    entries: VecDeque<Entry>,
    directory: Option<(u64, Vec<DirEntry>)>,
    file: Option<FileChain>,
}
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}
fn local_time(date: DateStamp) -> Result<(i64, u32)> {
    require(
        date.mins < 1440 && date.ticks < 3000,
        "amiga: invalid DateStamp minutes/ticks",
    )?;
    Ok((
        252460800
            + i64::from(date.days) * 86400
            + i64::from(date.mins) * 60
            + i64::from(date.ticks / 50),
        (date.ticks % 50) * 20_000_000,
    ))
}
impl<S: BlockSource> Amiga<S> {
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn open_with_hints(mut source: S, hints: Option<Hints>) -> Result<Self> {
        let geometry = Geometry::inspect(&mut source, hints)?;
        let mut volume = geometry.open(source)?;
        let allocation = Allocation::open(&mut volume, &geometry)?;
        let mut reader = Self {
            volume,
            geometry,
            allocation,
            entries: VecDeque::new(),
            directory: None,
            file: None,
        };
        reader.ensure_directory(geometry.root)?;
        Ok(reader)
    }
    fn entry(&mut self, node: u64) -> Result<Entry> {
        if let Some(entry) = self.entries.iter().find(|entry| entry.lba == node) {
            return Ok(entry.clone());
        }
        self.volume.source_mut().reset();
        self.allocation
            .data(&mut self.volume, &self.geometry, node)?;
        let entry = convert(self.volume.entry_at(node))?;
        require(
            u64::from(entry.own_key) == node,
            "amiga: entry own-key mismatch",
        )?;
        let mut raw = vec![0; self.geometry.block_size as usize];
        self.volume.source_mut().read_block(node, &mut raw)?;
        let block_size = raw.len();
        let length_offset = if self.geometry.variant.has_long_names() {
            block_size - 184
        } else {
            block_size - 80
        };
        let name_length = usize::from(raw[length_offset]);
        require(
            name_length > 0
                && name_length <= self.geometry.variant.max_name_len()
                && entry.name.len() == name_length
                && !entry.name.iter().any(|byte| matches!(*byte, 0 | b'/')),
            "amiga: invalid/lossy name encoding",
        )?;
        if self.geometry.variant.has_long_names() {
            let comment_offset = length_offset + 1 + name_length;
            require(
                comment_offset < length_offset + 112
                    && usize::from(raw[comment_offset]) <= 79
                    && comment_offset + 1 + usize::from(raw[comment_offset]) <= length_offset + 112,
                "amiga: long-name/comment bounds",
            )?;
        } else {
            require(raw[block_size - 184] <= 79, "amiga: comment length")?;
        }
        if self.entries.len() == 128 {
            self.entries.pop_front();
        }
        self.entries.push_back(entry.clone());
        Ok(entry)
    }
    fn canonical(&mut self, node: u64) -> Result<Entry> {
        let mut entry = self.entry(node)?;
        let wanted = match entry.kind {
            EntryKind::LinkFile => Some(EntryKind::File),
            EntryKind::LinkDir => Some(EntryKind::Directory),
            _ => None,
        };
        let mut seen = HashSet::new();
        loop {
            require(seen.insert(entry.lba), "amiga: hard-link cycle")?;
            if seen.len() > 256 {
                return Err(FoldError::Unsupported("amiga: hard-link depth limit"));
            }
            match entry.kind {
                EntryKind::LinkFile | EntryKind::LinkDir => {
                    require(entry.real_entry != 0, "amiga: missing hard-link target")?;
                    entry = self.entry(u64::from(entry.real_entry))?;
                }
                EntryKind::File | EntryKind::Directory | EntryKind::SoftLink => {
                    require(
                        wanted.is_none_or(|wanted| wanted == entry.kind),
                        "amiga: hard-link target type mismatch",
                    )?;
                    return Ok(entry);
                }
            }
        }
    }
    fn ensure_directory(&mut self, node: u64) -> Result<()> {
        let directory = if node == self.geometry.root {
            node
        } else {
            let entry = self.canonical(node)?;
            if entry.kind != EntryKind::Directory {
                return Err(FoldError::NotDirectory);
            }
            entry.lba
        };
        if self
            .directory
            .as_ref()
            .is_some_and(|(cached, _)| *cached == directory)
        {
            return Ok(());
        }
        self.volume.source_mut().reset();
        let entries = convert(self.volume.read_dir(directory))?;
        if entries.len() > 131072 {
            return Err(FoldError::Unsupported("amiga: directory entry limit"));
        }
        let table = convert(self.volume.hash_table(directory))?;
        let by_block: HashMap<_, _> = entries.iter().map(|entry| (entry.lba, entry)).collect();
        let fold = self.geometry.variant.fold();
        for (slot, first) in table.into_iter().enumerate() {
            let mut next = first;
            while next != 0 {
                let entry = by_block
                    .get(&u64::from(next))
                    .ok_or(FoldError::Corrupt("amiga: directory chain target missing"))?;
                let expected_slot = amiga_ffs::name_hash(
                    &entry.name,
                    fold,
                    amiga_ffs::hash_table_size(self.geometry.block_size as usize),
                ) as usize;
                if u64::from(entry.parent) != directory || expected_slot != slot {
                    return Err(FoldError::Source(format!("amiga: corrupt directory {directory}, entry {} {:?}, parent {}, slot {slot}, expected slot {expected_slot}",entry.lba,latin1(&entry.name),entry.parent)));
                }
                next = entry.hash_chain;
            }
        }
        let mut names = HashSet::new();
        let mut output = Vec::with_capacity(entries.len());
        for raw_entry in entries {
            let entry = self.entry(raw_entry.lba)?;
            require(
                names.insert(entry.name.iter().copied().map(fold).collect::<Vec<_>>()),
                "amiga: duplicate folded name",
            )?;
            let kind = match entry.kind {
                EntryKind::Directory | EntryKind::LinkDir => FileKind::Directory,
                EntryKind::File | EntryKind::LinkFile => FileKind::Regular,
                EntryKind::SoftLink => FileKind::Symlink,
            };
            output.push(DirEntry {
                name: latin1(&entry.name),
                node: entry.lba,
                kind,
            });
        }
        self.directory = Some((directory, output));
        Ok(())
    }
    fn soft_link(&mut self, node: u64) -> Result<Vec<u8>> {
        self.volume.source_mut().reset();
        let mut raw = vec![0; self.geometry.block_size as usize];
        self.volume.source_mut().read_block(node, &mut raw)?;
        require(
            raw[24..raw.len() - 200].contains(&0),
            "amiga: unterminated soft-link target",
        )?;
        let bytes = convert(self.volume.read_softlink(node))?;
        require(!bytes.is_empty(), "amiga: empty soft-link target")?;
        Ok(bytes)
    }
    fn ensure_file(&mut self, node: u64) -> Result<()> {
        if self
            .file
            .as_ref()
            .is_some_and(|chain| chain.header_lba == node)
        {
            return Ok(());
        }
        self.volume.source_mut().reset();
        let chain = convert(self.volume.file_chain(node))?;
        let mut used = chain.blocks.clone();
        used.extend_from_slice(&chain.extensions);
        used.push(
            u32::try_from(node).map_err(|_| FoldError::Corrupt("amiga: file header overflow"))?,
        );
        used.sort_unstable();
        require(
            used.windows(2).all(|pair| pair[0] != pair[1]),
            "amiga: repeated file data/header/extension block",
        )?;
        for block in used {
            self.allocation
                .data(&mut self.volume, &self.geometry, u64::from(block))?;
        }
        self.file = Some(chain);
        Ok(())
    }
}
impl<S: BlockSource> FoldFrontend<S> for Amiga<S> {
    const TAG: &'static str = "amiga";
    fn probe(source: &mut S) -> Result<bool> {
        Ok(Geometry::inspect(source, None).is_ok())
    }
    fn open(source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        Self::open_with_hints(source, None)
    }
    fn root(&self) -> NodeId {
        self.geometry.root
    }
    fn inode_identity(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<NodeId> {
        if node == self.geometry.root {
            Ok(node)
        } else {
            Ok(self.canonical(node)?.lba)
        }
    }
    fn lookup(
        &mut self,
        node: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        self.ensure_directory(node)?;
        let Some(bytes) = name
            .chars()
            .map(|character| u8::try_from(character as u32).ok())
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(None);
        };
        let name: Vec<_> = bytes
            .into_iter()
            .map(self.geometry.variant.fold())
            .collect();
        let (_, entries) = self
            .directory
            .as_ref()
            .ok_or(FoldError::Corrupt("amiga: missing directory cache"))?;
        Ok(entries
            .iter()
            .find(|entry| {
                entry
                    .name
                    .chars()
                    .map(|character| (self.geometry.variant.fold())(character as u8))
                    .eq(name.iter().copied())
            })
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.ensure_directory(node)?;
        self.directory
            .as_ref()
            .map(|(_, entries)| entries.clone())
            .ok_or(FoldError::Corrupt("amiga: missing directory cache"))
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        if node == self.geometry.root {
            return Ok(Metadata {
                kind: FileKind::Directory,
                size: 0,
                mode: 0,
            });
        }
        let entry = self.canonical(node)?;
        let (kind, size) = match entry.kind {
            EntryKind::File => (FileKind::Regular, u64::from(entry.byte_size)),
            EntryKind::Directory => (FileKind::Directory, 0),
            EntryKind::SoftLink => (FileKind::Symlink, self.soft_link(entry.lba)?.len() as u64),
            EntryKind::LinkFile | EntryKind::LinkDir => {
                return Err(FoldError::Corrupt("amiga: unresolved hard link"))
            }
        };
        Ok(Metadata {
            kind,
            size,
            mode: 0,
        })
    }
    fn local_timestamps(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<LocalFileTimes> {
        if node == self.geometry.root {
            return Ok(LocalFileTimes {
                modified: Some(local_time(self.volume.root().dir_altered)?),
                created: Some(local_time(self.volume.root().disk_made)?),
                ..LocalFileTimes::default()
            });
        }
        Ok(LocalFileTimes {
            modified: Some(local_time(self.entry(node)?.date)?),
            ..LocalFileTimes::default()
        })
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let entry = self.canonical(node)?;
        if entry.kind == EntryKind::SoftLink {
            let raw = self.soft_link(entry.lba)?;
            let start = offset.min(raw.len() as u64) as usize;
            let count = (raw.len() - start).min(bytes.len());
            bytes[..count].copy_from_slice(&raw[start..start + count]);
            return Ok(count);
        }
        if entry.kind != EntryKind::File {
            return Err(FoldError::IsDirectory);
        }
        self.ensure_file(entry.lba)?;
        self.volume.source_mut().reset();
        let chain = self
            .file
            .as_ref()
            .ok_or(FoldError::Corrupt("amiga: missing file map"))?;
        let expected = u64::from(chain.byte_size)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        let count = if self.geometry.variant.is_ffs() {
            crate::ranges::read_ffs(&mut self.volume, &self.geometry, chain, offset, bytes)?
        } else {
            convert(self.volume.read_range(chain, offset, bytes))?
        };
        require(count == expected, "amiga: short logical read")?;
        Ok(count)
    }
    fn read_link(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        if node == self.geometry.root {
            return Ok(None);
        }
        let entry = self.canonical(node)?;
        if entry.kind == EntryKind::SoftLink {
            self.soft_link(entry.lba).map(Some)
        } else {
            Ok(None)
        }
    }
    fn read_link_text(
        &mut self,
        node: NodeId,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<String>> {
        Ok(self.read_link(node, context)?.map(|bytes| latin1(&bytes)))
    }
}
