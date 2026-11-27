//! LittleFS v2 inspection over a bounded, read-only evidence source.
#![forbid(unsafe_code)]

mod format;
mod geometry;
mod metadata;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

use format::{CtzStruct, NAME_MAX, TagType};
use geometry::{Geometry, read_pair};
pub use geometry::{Header, PROBE_BYTES};
use metadata::{decode_pair, slots};

type Pair = [u32; 2];
const ROOT: NodeId = 0;

enum Content {
    Directory(Pair),
    Inline(Vec<u8>),
    Ctz(CtzStruct),
}

impl Content {
    fn metadata(&self) -> Metadata {
        let (kind, size) = match self {
            Self::Directory(_) => (FileKind::Directory, 0),
            Self::Inline(bytes) => (FileKind::Regular, bytes.len() as u64),
            Self::Ctz(chain) => (FileKind::Regular, u64::from(chain.size)),
        };
        Metadata {
            size,
            kind,
            mode: 0,
        }
    }
}

pub struct LittleFs<S> {
    source: S,
    geometry: Geometry,
    nodes: HashMap<NodeId, Content>,
    directories: HashMap<NodeId, BTreeMap<String, NodeId>>,
    directory_pairs: HashSet<Pair>,
}

impl<S: BlockSource> LittleFs<S> {
    fn header(source: &mut S) -> Result<Option<Header>> {
        let length = source.len();
        let mut head = [0; PROBE_BYTES];
        let head = &mut head[..length.min(PROBE_BYTES as u64) as usize];
        source.read_at(0, head)?;
        Header::locate(head, length, |at, bytes| source.read_at(at, bytes))
    }

    fn node(&self, id: NodeId) -> Result<&Content> {
        self.nodes.get(&id).ok_or(FoldError::NotFound)
    }

    fn read_directory(&mut self, mut pair: Pair) -> Result<Vec<(NodeId, String, Content)>> {
        let sb = self.geometry.superblock;
        let mut blocks = vec![0; 2 * sb.block_size as usize];
        let mut entries = Vec::new();
        let mut visited = HashSet::new();
        loop {
            require(
                visited.insert(normalized(pair))
                    && pair
                        .iter()
                        .all(|at| self.geometry.metadata_blocks.contains(at)),
                "LittleFS directory continuation cycle or unlinked metadata pair",
            )?;
            let (active, block) = read_pair(
                &mut self.source,
                sb.block_size,
                sb.block_count,
                pair,
                &mut blocks,
            )?;
            for (slot_id, slot) in slots(&block)?.into_iter().enumerate() {
                if self.geometry.pending_move == Some((normalized(pair), slot_id)) {
                    continue;
                }
                let Some((kind, raw)) = slot.name else {
                    continue;
                };
                if kind == TagType::Superblock {
                    continue;
                }
                let max_name = if sb.name_max == 0 {
                    NAME_MAX
                } else {
                    sb.name_max as usize
                };
                require(
                    !raw.is_empty()
                        && raw.len() <= max_name
                        && !raw.contains(&0)
                        && !raw.contains(&b'/'),
                    "invalid LittleFS filename",
                )?;
                let layout = slot
                    .layout
                    .ok_or(FoldError::Corrupt("LittleFS entry has no structure"))?;
                let content = match (kind, layout) {
                    (TagType::Directory, (TagType::DirStruct, bytes)) => {
                        Content::Directory(decode_pair(bytes)?)
                    }
                    (TagType::RegularFile, (TagType::InlineStruct, bytes)) => {
                        Content::Inline(bytes.to_vec())
                    }
                    (TagType::RegularFile, (TagType::CtzStruct, bytes)) => {
                        let chain = CtzStruct::from_bytes(bytes)
                            .ok_or(FoldError::Corrupt("invalid LittleFS CTZ structure"))?;
                        require(
                            chain.size <= i32::MAX as u32
                                && chain.block_count(sb.block_size) <= sb.block_count,
                            "LittleFS CTZ length exceeds declared geometry",
                        )?;
                        Content::Ctz(chain)
                    }
                    _ => {
                        return Err(FoldError::Corrupt(
                            "LittleFS entry and structure types disagree",
                        ));
                    }
                };
                let max_file = if sb.file_max == 0 {
                    i32::MAX as u64
                } else {
                    u64::from(sb.file_max)
                };
                require(
                    content.metadata().size <= max_file,
                    "LittleFS file exceeds declared file size limit",
                )?;
                let id = ((u64::from(active) << 10) | slot_id as u64) + 1;
                entries.push((id, display_name(raw), content));
            }
            let Some((next, true)) = block.tail()? else {
                break;
            };
            pair = next;
        }
        Ok(entries)
    }

    fn children(&mut self, id: NodeId) -> Result<&BTreeMap<String, NodeId>> {
        if !self.directories.contains_key(&id) {
            let &Content::Directory(pair) = self.node(id)? else {
                return Err(FoldError::NotDirectory);
            };
            let entries = self.read_directory(pair)?;
            let mut children = BTreeMap::new();
            let mut new_pairs = HashSet::new();
            for (id, name, content) in &entries {
                require(
                    !self.nodes.contains_key(id) && children.insert(name.clone(), *id).is_none(),
                    "duplicate LittleFS directory entry",
                )?;
                if let Content::Directory(pair) = content {
                    let normalized = normalized(*pair);
                    require(
                        pair.iter()
                            .all(|at| self.geometry.metadata_blocks.contains(at))
                            && !self.directory_pairs.contains(&normalized)
                            && new_pairs.insert(normalized),
                        "invalid or repeated LittleFS subdirectory pair",
                    )?;
                }
            }
            self.directory_pairs.extend(new_pairs);
            self.nodes
                .extend(entries.into_iter().map(|(id, _, content)| (id, content)));
            self.directories.insert(id, children);
        }
        Ok(&self.directories[&id])
    }

    fn read_ctz(&mut self, chain: CtzStruct, offset: u32, bytes: &mut [u8]) -> Result<()> {
        let sb = self.geometry.superblock;
        let head_index = chain.block_count(sb.block_size) - 1;
        let mut observed = HashMap::new();
        let mut done = 0;
        while done < bytes.len() {
            let (index, within) = CtzStruct::position(offset + done as u32, sb.block_size);
            require(
                index <= head_index && within < sb.block_size,
                "invalid LittleFS CTZ logical position",
            )?;
            let mut current = head_index;
            let mut block = chain.head_block;
            loop {
                require(
                    block < sb.block_count && !self.geometry.metadata_blocks.contains(&block),
                    "LittleFS CTZ pointer reaches metadata or exceeds the volume",
                )?;
                require(
                    observed
                        .insert(block, current)
                        .is_none_or(|previous| previous == current),
                    "LittleFS CTZ chain reuses a block",
                )?;
                if current == index {
                    break;
                }
                let distance = current - index;
                let skip = (31 - distance.leading_zeros()).min(current.trailing_zeros());
                let mut pointer = [0; 4];
                let pointer_at = u64::from(block) * u64::from(sb.block_size) + u64::from(4 * skip);
                self.source.read_at(pointer_at, &mut pointer)?;
                block = le32(&pointer);
                current -= 1 << skip;
            }
            let count = ((sb.block_size - within) as usize).min(bytes.len() - done);
            let address = u64::from(block) * u64::from(sb.block_size) + u64::from(within);
            self.source
                .read_at(address, &mut bytes[done..done + count])?;
            done += count;
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for LittleFs<S> {
    const TAG: &'static str = "littlefs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Self::header(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let geometry = Self::header(&mut source)?
            .ok_or(FoldError::Corrupt("LittleFS superblock absent"))?
            .open(&mut source)?;
        let root = geometry.root;
        Ok(Self {
            source,
            geometry,
            nodes: HashMap::from([(ROOT, Content::Directory(root))]),
            directories: HashMap::new(),
            directory_pairs: HashSet::from([normalized(root)]),
        })
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self.children(dir)?.get(name).copied())
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.children(dir)?;
        self.directories[&dir]
            .iter()
            .map(|(name, &node)| {
                Ok(DirEntry {
                    name: name.clone(),
                    node,
                    kind: self.node(node)?.metadata().kind,
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        Ok(self.node(node)?.metadata())
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let content = self.node(node)?;
        if matches!(content, Content::Directory(_)) {
            return Err(FoldError::IsDirectory);
        }
        let count = content
            .metadata()
            .size
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        match content {
            Content::Inline(inline) => {
                bytes[..count].copy_from_slice(&inline[offset as usize..offset as usize + count])
            }
            Content::Ctz(chain) => self.read_ctz(*chain, offset as u32, &mut bytes[..count])?,
            Content::Directory(_) => return Err(FoldError::IsDirectory),
        }
        Ok(count)
    }
}

fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn normalized(mut pair: Pair) -> Pair {
    pair.sort_unstable();
    pair
}

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if !condition {
        return Err(FoldError::Corrupt(reason));
    }
    Ok(())
}

fn display_name(raw: &[u8]) -> String {
    let mut name = String::new();
    if raw == b"." || raw == b".." {
        return "%2E".repeat(raw.len());
    }
    for part in raw.utf8_chunks() {
        for ch in part.valid().chars() {
            if ch == '%' || ch == '\\' || ch.is_ascii_control() {
                let _ = write!(name, "%{:02X}", u32::from(ch));
            } else {
                name.push(ch);
            }
        }
        for &byte in part.invalid() {
            let _ = write!(name, "%{byte:02X}");
        }
    }
    name
}
