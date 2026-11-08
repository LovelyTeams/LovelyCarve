//! Power-Safe superblock generations, endian-aware metadata and uniform trees.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Tree {
    size: u64,
    pointers: [u32; 16],
    levels: u8,
}
impl Tree {
    fn decode(
        order: Endian,
        bytes: &[u8],
        size_offset: usize,
        pointers_offset: usize,
        level_offset: usize,
        block_size: u64,
    ) -> Result<Self> {
        let size = order.u64(bytes, size_offset)?;
        let levels = *bytes
            .get(level_offset)
            .ok_or(FoldError::Corrupt("QNX6 tree level"))?;
        require(levels <= 5, "QNX6 indirect depth unsupported")?;
        require(
            size as u128 <= 16 * (block_size as u128 / 4).pow(levels as u32) * block_size as u128,
            "QNX6 tree shorter than size",
        )?;
        let mut pointers = [0; 16];
        for (index, pointer) in pointers.iter_mut().enumerate() {
            *pointer = order.u32(bytes, pointers_offset + index * 4)?;
        }
        Ok(Self {
            size,
            pointers,
            levels,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Geometry {
    order: Endian,
    block_size: u64,
    blocks: u32,
    inodes: u32,
    base: u64,
    mmi: bool,
    identity: Vec<u8>,
}
impl Geometry {
    fn data_start(&self) -> u64 {
        self.base + 4096
    }
    fn second(&self) -> u64 {
        self.data_start() + self.blocks as u64 * self.block_size
    }
}
struct Superblock {
    geometry: Geometry,
    serial: u64,
    inodes: Tree,
    longnames: Tree,
    unsupported_features: bool,
}
#[derive(Clone)]
struct Entry {
    info: Info,
    tree: Tree,
    unreadable: bool,
}
pub struct Qnx6<S> {
    source: S,
    sb: Superblock,
    entries: BTreeMap<u64, Entry>,
}

/// QNX6's non-reflected, zero-seeded CRC32, without a final complement.
pub fn superblock_checksum(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for byte in bytes {
        crc ^= (*byte as u32) << 24;
        for _ in 0..8 {
            crc = (crc << 1) ^ (0x04c11db7 & 0u32.wrapping_sub(crc >> 31));
        }
    }
    crc
}
fn magic(bytes: &[u8]) -> Option<Endian> {
    match bytes.get(..4) {
        Some([0x22, 0x11, 0x19, 0x68]) => Some(Endian::Little),
        Some([0x68, 0x19, 0x11, 0x22]) => Some(Endian::Big),
        _ => None,
    }
}
fn geometry(bytes: &[u8], order: Endian, base: u64, mmi: bool, length: u64) -> Result<Geometry> {
    let start = if mmi { 40 } else { 48 };
    let block_size = order.u32(bytes, start)? as u64;
    let inodes = order.u32(bytes, start + 4)?;
    let blocks = order.u32(bytes, start + 12)?;
    require(
        [512, 1024, 2048, 4096].contains(&block_size) && blocks > 0 && inodes > 0,
        "QNX6 invalid geometry",
    )?;
    require(
        order.u32(bytes, start + 8)? <= inodes && order.u32(bytes, start + 16)? <= blocks,
        "QNX6 free counters",
    )?;
    let value = Geometry {
        order,
        block_size,
        blocks,
        inodes,
        base,
        mmi,
        identity: bytes[if mmi { 28..40 } else { 32..48 }].to_vec(),
    };
    require(
        value
            .second()
            .checked_add(512)
            .is_some_and(|end| end <= length),
        "QNX6 declared volume truncated",
    )?;
    Ok(value)
}
fn decode(bytes: &[u8], base: u64, mmi: bool, at: u64, length: u64) -> Result<Superblock> {
    let order = magic(bytes).ok_or(FoldError::Corrupt("QNX6 superblock magic"))?;
    require(
        order.u32(bytes, 4)? == superblock_checksum(&bytes[8..512]),
        "QNX6 superblock CRC",
    )?;
    let geometry = geometry(bytes, order, base, mmi, length)?;
    require(
        at == base || at == geometry.second(),
        "QNX6 backup position mismatch",
    )?;
    let start = if mmi { 64 } else { 72 };
    let inodes = Tree::decode(
        order,
        bytes,
        start,
        start + 8,
        start + 72,
        geometry.block_size,
    )?;
    let longnames = Tree::decode(
        order,
        bytes,
        start + 160,
        start + 168,
        start + 232,
        geometry.block_size,
    )?;
    require(
        inodes.size >= geometry.inodes as u64 * 128,
        "QNX6 inode table size",
    )?;
    Ok(Superblock {
        geometry,
        serial: order.u64(bytes, 8)?,
        inodes,
        longnames,
        unsupported_features: !mmi
            && (order.u16(bytes, 28)? != 4
                || order.u16(bytes, 30)? != 3
                || order.u32(bytes, 24)? & !0x101 != 0),
    })
}
#[derive(Clone, Copy)]
enum Holes {
    Zero,
    Reject,
}
impl<S: BlockSource> Qnx6<S> {
    pub fn serial(&self) -> u64 {
        self.sb.serial
    }
    pub fn endian(&self) -> Endian {
        self.sb.geometry.order
    }
    pub fn block_size(&self) -> u64 {
        self.sb.geometry.block_size
    }
    pub fn is_mmi(&self) -> bool {
        self.sb.geometry.mmi
    }
    pub(super) fn probe_source(source: &mut S) -> Result<bool> {
        for offset in [8192, 0] {
            if source.len() >= offset + 4 {
                let mut bytes = [0; 4];
                exact(source, offset, &mut bytes)?;
                if magic(&bytes).is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub(super) fn open_source(mut source: S) -> Result<Self> {
        // A corrupt primary can still locate a CRC-valid backup via bounded
        // geometry. For an exact volume image also try its final reserved area.
        for (base, mmi) in [(8192, false), (0, false), (0, true)] {
            if source.len() < base + 512 {
                continue;
            }
            let mut bytes = [0; 512];
            exact(&mut source, base, &mut bytes)?;
            let mut positions = BTreeSet::from([base]);
            for order in [Endian::Little, Endian::Big] {
                if let Ok(value) = geometry(&bytes, order, base, mmi, source.len()) {
                    positions.insert(value.second());
                }
            }
            if source.len() >= 4096 {
                positions.insert(source.len() - 4096);
            }
            let mut candidates: Vec<Superblock> = Vec::new();
            for position in positions {
                exact(&mut source, position, &mut bytes)?;
                if let Ok(candidate) = decode(&bytes, base, mmi, position, source.len()) {
                    candidates.push(candidate);
                }
            }
            if candidates.is_empty() {
                continue;
            }
            require(
                candidates
                    .iter()
                    .all(|value| value.geometry == candidates[0].geometry),
                "QNX6 superblock geometry disagreement",
            )?;
            candidates.sort_by_key(|value| value.serial);
            let sb = candidates
                .pop()
                .ok_or(FoldError::Corrupt("QNX6 superblock selection"))?;
            if sb.unsupported_features {
                return Err(FoldError::Unsupported(
                    "QNX6 unsupported volume version/features (including encryption)",
                ));
            }
            let mut reader = Self {
                source,
                sb,
                entries: BTreeMap::new(),
            };
            let root = reader.entry(1)?;
            require(
                root.info.metadata.kind == FileKind::Directory && root.tree.size >= 64,
                "QNX6 root inode",
            )?;
            let mut dots = [0; 64];
            reader.read_tree(&root.tree, 0, &mut dots, Holes::Reject)?;
            let order = reader.sb.geometry.order;
            require(
                order.u32(&dots, 0)? == 1
                    && dots[4] == 1
                    && dots[5] == b'.'
                    && order.u32(&dots, 32)? == 1
                    && dots[36] == 2
                    && &dots[37..39] == b"..",
                "QNX6 root dot entries",
            )?;
            return Ok(reader);
        }
        Err(FoldError::Corrupt(
            "QNX6 no valid standard/MMI superblock generation",
        ))
    }
    pub(super) fn root_node(&self) -> u64 {
        1
    }
    fn block(&mut self, pointer: u32) -> Result<Vec<u8>> {
        let geometry = &self.sb.geometry;
        require(
            pointer < geometry.blocks,
            "QNX6 allocated pointer outside data area",
        )?;
        let mut bytes = vec![0; geometry.block_size as usize];
        exact(
            &mut self.source,
            geometry.data_start() + pointer as u64 * geometry.block_size,
            &mut bytes,
        )?;
        Ok(bytes)
    }
    fn map(&mut self, tree: &Tree, index: u64) -> Result<Option<u32>> {
        let fanout = self.sb.geometry.block_size / 4;
        let mut span = fanout.pow(tree.levels as u32);
        let slot = index / span;
        require(slot < 16, "QNX6 logical block outside tree")?;
        let mut pointer = tree.pointers[slot as usize];
        let mut seen = BTreeSet::new();
        for _ in 0..tree.levels {
            if pointer == u32::MAX {
                return Ok(None);
            }
            require(seen.insert(pointer), "QNX6 indirect cycle")?;
            let bytes = self.block(pointer)?;
            span /= fanout;
            pointer = self
                .sb
                .geometry
                .order
                .u32(&bytes, (index / span % fanout) as usize * 4)?;
        }
        if pointer == u32::MAX {
            return Ok(None);
        }
        require(
            !seen.contains(&pointer) && pointer < self.sb.geometry.blocks,
            "QNX6 data pointer cycle/bounds",
        )?;
        Ok(Some(pointer))
    }
    fn read_tree(
        &mut self,
        tree: &Tree,
        offset: u64,
        output: &mut [u8],
        holes: Holes,
    ) -> Result<usize> {
        let count = tree.size.saturating_sub(offset).min(output.len() as u64) as usize;
        let block_size = self.sb.geometry.block_size;
        let mut done = 0;
        while done < count {
            let logical = offset + done as u64;
            let within = (logical % block_size) as usize;
            let take = (block_size as usize - within).min(count - done);
            if let Some(pointer) = self.map(tree, logical / block_size)? {
                output[done..done + take]
                    .copy_from_slice(&self.block(pointer)?[within..within + take]);
            } else {
                require(matches!(holes, Holes::Zero), "QNX6 unmapped metadata block")?;
                output[done..done + take].fill(0);
            }
            done += take;
        }
        Ok(count)
    }
    fn entry(&mut self, node: u64) -> Result<Entry> {
        if let Some(entry) = self.entries.get(&node) {
            return Ok(entry.clone());
        }
        require(
            node > 0 && node <= self.sb.geometry.inodes as u64,
            "QNX6 inode number bounds",
        )?;
        require(self.entries.len() < 100_000, "QNX6 inode cache limit")?;
        let tree = self.sb.inodes.clone();
        let mut bytes = [0; 128];
        require(
            self.read_tree(&tree, (node - 1) * 128, &mut bytes, Holes::Reject)? == 128,
            "QNX6 short inode",
        )?;
        let order = self.sb.geometry.order;
        let mode = order.u16(&bytes, 32)?;
        require(
            matches!(bytes[101], 1 | 3) && mode & 0xf000 != 0,
            "QNX6 deleted/unallocated inode",
        )?;
        let tree = Tree::decode(order, &bytes, 0, 36, 100, self.sb.geometry.block_size)?;
        let entry = Entry {
            info: Info {
                metadata: Metadata {
                    size: tree.size,
                    mode: mode as u32,
                    kind: kind(mode),
                },
                times: times(order, &bytes, 16)?,
            },
            tree,
            unreadable: order.u16(&bytes, 34)? & !7 != 0 || order.u16(&bytes, 102)? != 0,
        };
        self.entries.insert(node, entry.clone());
        Ok(entry)
    }
    pub(super) fn info(&mut self, node: u64) -> Result<Info> {
        Ok(self.entry(node)?.info)
    }
    pub(super) fn content(&mut self, node: u64, offset: u64, output: &mut [u8]) -> Result<usize> {
        let entry = self.entry(node)?;
        if entry.unreadable {
            return Err(FoldError::Unsupported(
                "QNX6 extended/encrypted inode content",
            ));
        }
        self.read_tree(&entry.tree, offset, output, Holes::Zero)
    }
    pub(super) fn directory(&mut self, node: u64) -> Result<Vec<DirEntry>> {
        let entry = self.entry(node)?;
        if entry.unreadable {
            return Err(FoldError::Unsupported("QNX6 extended/encrypted directory"));
        }
        require(
            entry.info.metadata.kind == FileKind::Directory
                && entry.tree.size % 32 == 0
                && entry.tree.size <= 32 * 1024 * 1024,
            "QNX6 directory kind/size/limit",
        )?;
        let mut names = BTreeSet::new();
        let mut result = Vec::new();
        let order = self.sb.geometry.order;
        for offset in (0..entry.tree.size).step_by(32) {
            let mut bytes = [0; 32];
            self.read_tree(&entry.tree, offset, &mut bytes, Holes::Reject)?;
            let node = order.u32(&bytes, 0)? as u64;
            let size = bytes[4] as usize;
            if node == 0 || size == 0 {
                continue;
            }
            let filename = if size == 255 {
                let tree = self.sb.longnames.clone();
                let offset = order.u32(&bytes, 8)? as u64 * self.sb.geometry.block_size;
                let mut block = vec![0; self.sb.geometry.block_size as usize];
                let actual = self.read_tree(&tree, offset, &mut block, Holes::Reject)?;
                let length = order.u16(&block, 0)? as usize;
                require(
                    (1..=510).contains(&length) && length + 2 <= actual,
                    "QNX6 longname length",
                )?;
                let raw = &block[2..length + 2];
                if !self.sb.geometry.mmi {
                    let checksum = raw.iter().fold(0u32, |value, byte| {
                        (value >> 1).wrapping_add(*byte as u32) ^ (value << 31)
                    });
                    require(checksum == order.u32(&bytes, 12)?, "QNX6 longname checksum")?;
                }
                name(raw)?
            } else {
                require(size <= 27, "QNX6 shortname length")?;
                name(&bytes[5..5 + size])?
            };
            if filename == "." || filename == ".." {
                continue;
            }
            require(
                names.insert(filename.clone()),
                "QNX6 duplicate directory name",
            )?;
            result.push(DirEntry {
                name: filename,
                node,
                kind: self.info(node)?.metadata.kind,
            });
        }
        Ok(result)
    }
}
