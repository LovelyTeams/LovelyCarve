//! Master-copy selection and bounded traversal of the committed UBIFS index.
use super::{
    UbifsImage,
    nodes::{self, Geometry, Key, Location, MAGIC, wide, word},
};
use crate::{ReadAt, volume::read_exact};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub struct Master {
    pub commit: u64,
    pub sequence: u64,
    pub root: Location,
    pub highest_inode: u64,
    pub flags: u32,
}
impl Master {
    pub fn open(source: &dyn ReadAt, geometry: &Geometry) -> Result<Self> {
        let mut candidates = Vec::new();
        for block in [1u32, 2] {
            let stride = geometry.minimum_io.max(512) as usize;
            for offset in (0..geometry.leb_size as usize).step_by(stride) {
                let location = Location {
                    block,
                    offset: offset as u32,
                    length: 512,
                };
                if offset + 512 > geometry.leb_size as usize {
                    break;
                }
                let prefix = read_exact(source, geometry.address(location)?, 24)?;
                if prefix[..4] != MAGIC || prefix[20] != 7 {
                    continue;
                }
                // A torn/corrupt master record may be replaced by a valid copy.
                let bytes = read_exact(source, geometry.address(location)?, 512)?;
                if nodes::validate_node(&bytes, 7).is_err() {
                    continue;
                }
                let root = Location::parse(&bytes[48..60])?;
                geometry.address(root)?;
                ensure!(
                    word(&bytes, 164)? == geometry.leb_count,
                    "UBIFS master/superblock LEB counts differ"
                );
                candidates.push(Self {
                    commit: wide(&bytes, 32)?,
                    sequence: wide(&bytes, 8)?,
                    root,
                    highest_inode: wide(&bytes, 24)?,
                    flags: word(&bytes, 40)?,
                });
            }
        }
        candidates.sort_unstable_by_key(|master| (master.commit, master.sequence));
        let selected = candidates
            .pop()
            .context("UBIFS has no CRC-valid master copy")?;
        for master in candidates
            .iter()
            .filter(|master| master.commit == selected.commit)
        {
            ensure!(
                master.root == selected.root && master.highest_inode == selected.highest_inode,
                "conflicting UBIFS masters for the same commit"
            );
            ensure!(
                master.sequence != selected.sequence || master.flags == selected.flags,
                "conflicting UBIFS master sequence"
            );
        }
        ensure!(
            selected.flags == 2,
            "unsupported UBIFS dirty/recovery/orphan state: journal replay required"
        );
        ensure!(
            selected.highest_inode <= u64::from(u32::MAX),
            "UBIFS inode exceeds simple-key range"
        );
        Ok(selected)
    }
}

pub struct IndexBuilder {
    pub image: UbifsImage,
    spans: BTreeMap<(u32, u32), u32>,
    read_bytes: u64,
    nodes: usize,
}
impl IndexBuilder {
    pub fn new(geometry: Geometry, master: &Master) -> Self {
        Self {
            image: UbifsImage {
                geometry,
                commit: master.commit,
                inodes: BTreeMap::new(),
                directories: BTreeMap::new(),
                data: BTreeMap::new(),
            },
            spans: BTreeMap::new(),
            read_bytes: 0,
            nodes: 0,
        }
    }
    fn reserve(&mut self, location: Location, read_bytes: u32) -> Result<()> {
        self.image.geometry.address(location)?;
        ensure!(
            location.block >= self.image.geometry.main_first,
            "UBIFS index points outside main area"
        );
        self.nodes += 1;
        self.read_bytes = self
            .read_bytes
            .checked_add(u64::from(read_bytes))
            .context("UBIFS index size overflow")?;
        ensure!(
            self.nodes <= 1_000_000 && self.read_bytes <= 512 * 1024 * 1024,
            "UBIFS metadata/index resource limit"
        );
        let key = (location.block, location.offset);
        if let Some((&(block, start), &end)) = self.spans.range(..=key).next_back() {
            ensure!(
                block != location.block || end <= location.offset,
                "UBIFS index cycle/overlapping nodes at {block}:{start}"
            );
        }
        let end = location.offset + location.length;
        if let Some((&(block, start), _)) = self.spans.range(key..).next() {
            ensure!(
                block != location.block || start >= end,
                "UBIFS overlapping index references"
            );
        }
        self.spans.insert(key, end);
        Ok(())
    }
    pub fn walk(
        &mut self,
        source: &dyn ReadAt,
        location: Location,
        expected_level: Option<u16>,
        depth: usize,
    ) -> Result<(Key, Key)> {
        ensure!(depth <= 64, "UBIFS index depth limit");
        self.reserve(location, location.length)?;
        ensure!(
            location.length >= 28 && location.length <= 28 + self.image.geometry.fanout * 20,
            "invalid UBIFS index node length"
        );
        let bytes = self.image.geometry.read_node(source, location, 9)?;
        let count = u16::from_le_bytes(nodes::number(&bytes, 24)?) as usize;
        let level = u16::from_le_bytes(nodes::number(&bytes, 26)?);
        ensure!(
            count > 0
                && count <= self.image.geometry.fanout as usize
                && bytes.len() == 28 + count * 20,
            "invalid UBIFS index branches"
        );
        ensure!(
            level <= 64 && expected_level.is_none_or(|expected| expected == level),
            "UBIFS index level mismatch"
        );
        let mut first = None;
        let mut previous: Option<Key> = None;
        for branch in bytes[28..].as_chunks::<20>().0 {
            let child = Location::parse(branch)?;
            let key = Key::parse(&branch[12..20])?;
            ensure!(
                previous
                    .is_none_or(|previous| previous < key || (previous == key && key.kind >= 2)),
                "unsorted/duplicate UBIFS index keys"
            );
            let (actual, last) = if level > 0 {
                self.walk(source, child, Some(level - 1), depth + 1)?
            } else {
                let leaf = self.leaf(source, child, key)?;
                (leaf, leaf)
            };
            ensure!(actual == key, "UBIFS branch/child key mismatch");
            first.get_or_insert(key);
            previous = Some(last);
        }
        Ok((
            first.context("empty UBIFS index")?,
            previous.context("empty UBIFS index")?,
        ))
    }
    fn leaf(&mut self, source: &dyn ReadAt, location: Location, key: Key) -> Result<Key> {
        self.reserve(location, if key.kind == 1 { 40 } else { location.length })?;
        ensure!(
            location.length >= 40 && location.length <= 8192,
            "UBIFS leaf size limit"
        );
        let prefix = read_exact(source, self.image.geometry.address(location)?, 40)?;
        ensure!(
            prefix[..4] == MAGIC && word(&prefix, 16)? == location.length && prefix[20] == key.kind,
            "UBIFS leaf header mismatch"
        );
        ensure!(
            prefix[32..40].iter().all(|byte| *byte == 0),
            "unsupported UBIFS leaf key format"
        );
        let actual = Key::parse(&prefix[24..32])?;
        ensure!(actual == key, "UBIFS leaf key differs from index");
        match key.kind {
            0 => {
                let bytes = self.image.geometry.read_node(source, location, 0)?;
                let inode = nodes::Inode::parse(&bytes, key)?;
                ensure!(
                    self.image.inodes.insert(key.inode, inode).is_none(),
                    "duplicate UBIFS inode"
                );
            }
            1 => {
                ensure!(
                    location.length > 48 && location.length <= 48 + 4096,
                    "invalid UBIFS data node length"
                );
                // Full CRC and decompression are deferred to the requested file
                // block; metadata enumeration never materializes all file bytes.
                ensure!(
                    self.image
                        .data
                        .insert((key.inode, key.value), location)
                        .is_none(),
                    "duplicate UBIFS data key"
                );
            }
            2 | 3 => {
                let bytes = self.image.geometry.read_node(source, location, key.kind)?;
                let entry = nodes::parse_entry(&bytes, key, self.image.geometry.key_hash)?;
                if key.kind == 2 {
                    self.image
                        .directories
                        .entry(key.inode)
                        .or_default()
                        .push(entry);
                }
                // Xattrs are validated as nodes but not projected as path entries.
            }
            _ => anyhow::bail!("unsupported UBIFS leaf key type"),
        }
        Ok(actual)
    }
    pub fn finish(mut self, highest_inode: u64) -> Result<UbifsImage> {
        ensure!(
            self.image
                .inodes
                .keys()
                .all(|inode| u64::from(*inode) <= highest_inode),
            "UBIFS inode exceeds master highest inode"
        );
        ensure!(
            self.image.inode(1)?.kind == nodes::FileType::Directory,
            "UBIFS root inode is not a directory"
        );
        let mut directory_parents = BTreeSet::new();
        for (&parent, entries) in &self.image.directories {
            ensure!(
                self.image.inode(parent)?.kind == nodes::FileType::Directory,
                "UBIFS parent is not a directory"
            );
            let mut names = BTreeSet::new();
            for entry in entries {
                let target = self.image.inode(entry.inode)?;
                ensure!(
                    target.kind.directory_type() == entry.kind && target.flags & 0x20 == 0,
                    "UBIFS directory/inode type mismatch"
                );
                ensure!(names.insert(&entry.name), "duplicate UBIFS directory name");
                if target.kind == nodes::FileType::Directory {
                    ensure!(
                        entry.inode != 1 && directory_parents.insert(entry.inode),
                        "UBIFS directory cycle/multiple parents"
                    );
                }
            }
        }
        let mut visited = BTreeSet::new();
        let mut pending = vec![(1u32, 0usize)];
        while let Some((inode, depth)) = pending.pop() {
            ensure!(depth <= 256, "UBIFS directory depth limit");
            if !visited.insert(inode) {
                continue;
            }
            if let Some(entries) = self.image.directories.get(&inode) {
                pending.extend(entries.iter().map(|entry| (entry.inode, depth + 1)));
            }
        }
        ensure!(
            self.image
                .inodes
                .values()
                .all(|inode| inode.flags & 0x20 != 0 || visited.contains(&inode.number)),
            "unreachable UBIFS committed inode"
        );
        for &(inode, block) in self.image.data.keys() {
            let metadata = self.image.inode(inode)?;
            ensure!(
                metadata.kind == nodes::FileType::File
                    && metadata.flags & 0x20 == 0
                    && u64::from(block) * 4096 < metadata.size,
                "UBIFS data outside regular file size"
            );
        }
        for entries in self.image.directories.values_mut() {
            entries.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Ok(self.image)
    }
}
