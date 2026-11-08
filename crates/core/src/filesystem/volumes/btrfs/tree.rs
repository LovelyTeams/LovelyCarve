//! B-tree traversal and inode lookup.
use super::*;

impl<'a> BtrfsVolume<'a> {
    /// Diagnostic: yield the first `limit` (objectid, key_type, offset)
    /// triples observed in the default subvolume's FS tree, in key order.
    /// Intended for the `probe_btrfs` example only — not exposed via IPC.
    pub fn debug_sample_fs_keys(&self, limit: usize) -> anyhow::Result<Vec<(u64, u8, u64)>> {
        let mut out = Vec::with_capacity(limit);
        let (root_bytenr, root_level) = self.subvol_root(self.default_subvol)?;
        let walker = self.walker();
        walker.iter_range(
            root_bytenr,
            root_level,
            &Key {
                objectid: 0u64.into(),
                key_type: 0,
                offset: 0u64.into(),
            },
            &Key {
                objectid: u64::MAX.into(),
                key_type: u8::MAX,
                offset: u64::MAX.into(),
            },
            |key, _data| {
                out.push((key.objectid.get(), key.key_type, key.offset.get()));
                Ok(out.len() < limit)
            },
        )?;
        Ok(out)
    }

    /// Diagnostic: for every DIR_INDEX entry under `parent` in `subvol`,
    /// surface the raw `(name, location_objectid, location_key_type,
    /// file_type)`.
    pub fn debug_dir_index(
        &self,
        subvol: u64,
        parent: u64,
    ) -> anyhow::Result<Vec<(String, u64, u8, u8)>> {
        let mut out = Vec::new();
        let (root_bytenr, root_level) = self.subvol_root(subvol)?;
        let walker = self.walker();
        walker.iter_range(
            root_bytenr,
            root_level,
            &Key {
                objectid: parent.into(),
                key_type: KEY_DIR_INDEX,
                offset: 0u64.into(),
            },
            &Key {
                objectid: parent.into(),
                key_type: KEY_DIR_INDEX,
                offset: u64::MAX.into(),
            },
            |key, data| {
                if key.objectid.get() != parent || key.key_type != KEY_DIR_INDEX {
                    return Ok(false);
                }
                if data.len() < 30 {
                    return Ok(true);
                }
                let location_objectid = u64::from_le_bytes([
                    data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
                ]);
                let location_type = data[8];
                let name_len = u16::from_le_bytes([data[27], data[28]]) as usize;
                let ftype = data[29];
                let name = if 30 + name_len <= data.len() {
                    String::from_utf8_lossy(&data[30..30 + name_len]).into_owned()
                } else {
                    String::from("<truncated>")
                };
                out.push((name, location_objectid, location_type, ftype));
                Ok(true)
            },
        )?;
        Ok(out)
    }

    pub fn debug_default_fs_tree_root(&self) -> anyhow::Result<(u64, u8)> {
        self.subvol_root(self.default_subvol)
    }

    pub(super) fn walker(&self) -> TreeWalker<'a, '_> {
        TreeWalker::new(
            self.image,
            self.base_offset,
            &self.chunks,
            self.nodesize as usize,
            self.layout.as_ref(),
        )
    }

    pub fn read_inode(&self, subvol: u64, inode: u64) -> anyhow::Result<BtrfsInode> {
        let mut found: Option<BtrfsInode> = None;
        let (root_bytenr, root_level) = self.subvol_root(subvol)?;
        let walker = self.walker();
        let key_min = Key {
            objectid: inode.into(),
            key_type: KEY_INODE_ITEM,
            offset: 0u64.into(),
        };
        let key_max = Key {
            objectid: inode.into(),
            key_type: KEY_INODE_ITEM,
            offset: u64::MAX.into(),
        };
        walker.iter_range(root_bytenr, root_level, &key_min, &key_max, |key, data| {
            if key.objectid.get() != inode || key.key_type != KEY_INODE_ITEM {
                return Ok(false);
            }
            found = parse_inode_item(inode, data);
            Ok(false)
        })?;
        found.ok_or_else(|| anyhow!("btrfs: inode {inode} not found in subvolume {subvol}"))
    }

    /// One sequential pass over `subvol`'s whole FS tree, emitting every
    /// inode item and directory entry in key order. Each tree node is read
    /// exactly once — the bulk-enumeration primitive behind the snapshot
    /// fast path, replacing the per-directory `dir_entries` plus per-child
    /// `read_inode` root-to-leaf descents of the browse path.
    ///
    /// `heartbeat` runs every [`SCAN_HEARTBEAT_EVERY`] tree items of ANY key
    /// type (a fragmented file can put 10^5+ EXTENT_DATA items between two
    /// directory records, so pacing on emitted records alone would leave
    /// long uncancellable stretches); returning `false` from it stops the
    /// walk, as does returning `false` from `visit`. Returns `Ok(true)`
    /// after a complete pass, `Ok(false)` when either callback stopped it.
    pub(crate) fn scan_subvol_tree(
        &self,
        subvol: u64,
        heartbeat: &mut dyn FnMut() -> bool,
        visit: &mut dyn FnMut(BtrfsTreeRecord) -> bool,
    ) -> anyhow::Result<bool> {
        let (root_bytenr, root_level) = self.subvol_root(subvol)?;
        let walker = self.walker();
        let mut completed = true;
        let mut items: u64 = 0;
        walker.iter_range(
            root_bytenr,
            root_level,
            &Key {
                objectid: 0u64.into(),
                key_type: 0,
                offset: 0u64.into(),
            },
            &Key {
                objectid: u64::MAX.into(),
                key_type: u8::MAX,
                offset: u64::MAX.into(),
            },
            |key, data| {
                items += 1;
                if items.is_multiple_of(SCAN_HEARTBEAT_EVERY) && !heartbeat() {
                    completed = false;
                    return Ok(false);
                }
                let record = match key.key_type {
                    // Unparseable INODE_ITEM payloads are surfaced (with
                    // `item: None`) rather than dropped, so consumers can
                    // replicate `read_inode`'s first-record-wins semantics
                    // on malformed trees.
                    KEY_INODE_ITEM => Some(BtrfsTreeRecord::Inode {
                        objectid: key.objectid.get(),
                        item: parse_inode_item(key.objectid.get(), data),
                    }),
                    KEY_DIR_INDEX => parse_dir_item(data).map(|entry| BtrfsTreeRecord::DirIndex {
                        parent: key.objectid.get(),
                        entry,
                    }),
                    _ => None,
                };
                match record {
                    Some(record) => {
                        if visit(record) {
                            Ok(true)
                        } else {
                            completed = false;
                            Ok(false)
                        }
                    }
                    None => Ok(true),
                }
            },
        )?;
        Ok(completed)
    }
}

// =============================================================================
// B-tree walker.
// =============================================================================

pub(super) struct TreeWalker<'img, 'c> {
    pub(super) image: &'img DiskImage,
    pub(super) base_offset: u64,
    pub(super) chunks: &'c [ChunkMapping],
    pub(super) nodesize: usize,
    layout: Option<&'c crate::evidence::partitions::lvm2::layout::LinearLayout>,
}

pub(super) struct Frame {
    #[allow(dead_code)]
    pub(super) logical: u64,
    pub(super) level: u8,
    pub(super) num_items: u32,
    pub(super) slot: usize,
    pub(super) block: Vec<u8>,
}

impl<'img, 'c> TreeWalker<'img, 'c> {
    pub(super) fn new(
        image: &'img DiskImage,
        base_offset: u64,
        chunks: &'c [ChunkMapping],
        nodesize: usize,
        layout: Option<&'c crate::evidence::partitions::lvm2::layout::LinearLayout>,
    ) -> Self {
        Self {
            image,
            base_offset,
            chunks,
            nodesize,
            layout,
        }
    }

    pub(super) fn read_node(&self, logical: u64) -> anyhow::Result<Vec<u8>> {
        let cm = self
            .chunks
            .iter()
            .find(|c| logical >= c.logical && logical < c.logical + c.length)
            .ok_or_else(|| anyhow!("btrfs: tree node logical 0x{logical:x} unmapped"))?;
        let delta = logical - cm.logical;
        let phys = self.base_offset + cm.physical + delta;
        let block = crate::filesystem::volumes::lvm::read_backing_range(
            self.image,
            self.layout,
            phys,
            self.nodesize,
        )?;
        if block.len() < HEADER_SIZE {
            bail!(
                "btrfs: tree node short-read: got {} bytes for logical 0x{logical:x}",
                block.len()
            );
        }
        Ok(block)
    }

    pub(super) fn iter_range<F>(
        &self,
        root_logical: u64,
        root_level: u8,
        key_min: &Key,
        key_max: &Key,
        mut visit: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(&Key, &[u8]) -> anyhow::Result<bool>,
    {
        let mut stack: Vec<Frame> = Vec::with_capacity(MAX_TREE_DEPTH);
        self.descend_to_first(root_logical, root_level, key_min, &mut stack)?;
        loop {
            let leaf = match stack.last_mut() {
                Some(frame) => frame,
                None => return Ok(()),
            };
            debug_assert_eq!(leaf.level, 0);
            while leaf.slot < leaf.num_items as usize {
                let item = item_at(&leaf.block, leaf.slot)?;
                if cmp_key(&item.key, key_max) == Ordering::Greater {
                    return Ok(());
                }
                if cmp_key(&item.key, key_min) != Ordering::Less {
                    let data = item_data(&leaf.block, &item)?;
                    if !visit(&item.key, data)? {
                        return Ok(());
                    }
                }
                leaf.slot += 1;
            }
            stack.pop();
            self.advance_to_next_leaf(&mut stack)?;
            if stack.is_empty() {
                return Ok(());
            }
        }
    }

    pub(super) fn descend_to_first(
        &self,
        root_logical: u64,
        root_level: u8,
        key_min: &Key,
        stack: &mut Vec<Frame>,
    ) -> anyhow::Result<()> {
        let mut logical = root_logical;
        let mut level = root_level;
        for _ in 0..=MAX_TREE_DEPTH {
            let block = self.read_node(logical)?;
            let n = header_num_items(&block) as usize;
            let header_level = header_level(&block);
            if header_level != level {
                bail!(
                    "btrfs: header level {header_level} disagrees with parent expectation {level} \
                     at logical 0x{logical:x}"
                );
            }
            if level == 0 {
                let mut slot = 0;
                while slot < n {
                    let item = item_at(&block, slot)?;
                    if cmp_key(&item.key, key_min) != Ordering::Less {
                        break;
                    }
                    slot += 1;
                }
                stack.push(Frame {
                    logical,
                    level,
                    num_items: n as u32,
                    slot,
                    block,
                });
                return Ok(());
            }
            let mut slot = 0;
            for i in 0..n {
                let kp = key_pointer_at(&block, i)?;
                if cmp_key(&kp.key, key_min) != Ordering::Greater {
                    slot = i;
                } else {
                    break;
                }
            }
            let kp = key_pointer_at(&block, slot)?;
            let next_logical = kp.block_pointer.get();
            stack.push(Frame {
                logical,
                level,
                num_items: n as u32,
                slot,
                block,
            });
            logical = next_logical;
            level = level.saturating_sub(1);
        }
        bail!("btrfs: tree exceeds MAX_TREE_DEPTH ({MAX_TREE_DEPTH})");
    }

    pub(super) fn descend_left(
        &self,
        root_logical: u64,
        root_level: u8,
        stack: &mut Vec<Frame>,
    ) -> anyhow::Result<()> {
        let mut logical = root_logical;
        let mut level = root_level;
        for _ in 0..=MAX_TREE_DEPTH {
            let block = self.read_node(logical)?;
            let n = header_num_items(&block) as usize;
            stack.push(Frame {
                logical,
                level,
                num_items: n as u32,
                slot: 0,
                block,
            });
            if level == 0 {
                return Ok(());
            }
            let top = stack.last().ok_or_else(|| {
                anyhow::anyhow!("btrfs: descend_left frame stack unexpectedly empty")
            })?;
            let kp = key_pointer_at(&top.block, 0)?;
            logical = kp.block_pointer.get();
            level -= 1;
        }
        bail!("btrfs: tree exceeds MAX_TREE_DEPTH while descending left");
    }

    pub(super) fn advance_to_next_leaf(&self, stack: &mut Vec<Frame>) -> anyhow::Result<()> {
        loop {
            let descent = match stack.last_mut() {
                Some(frame) => {
                    frame.slot += 1;
                    if frame.slot < frame.num_items as usize {
                        let kp = key_pointer_at(&frame.block, frame.slot)?;
                        Some((kp.block_pointer.get(), frame.level - 1))
                    } else {
                        None
                    }
                }
                None => return Ok(()),
            };
            match descent {
                Some((logical, level)) => return self.descend_left(logical, level, stack),
                None => {
                    stack.pop();
                }
            }
        }
    }
}

// =============================================================================
// Byte-level helpers.
// =============================================================================

pub(super) fn read_u64(buf: &[u8], off: usize) -> u64 {
    if buf.len() < off + 8 {
        return 0;
    }
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

pub(super) fn read_u32(buf: &[u8], off: usize) -> u32 {
    if buf.len() < off + 4 {
        return 0;
    }
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

pub(super) fn header_num_items(block: &[u8]) -> u32 {
    if block.len() < 100 {
        return 0;
    }
    u32::from_le_bytes([block[96], block[97], block[98], block[99]])
}

pub(super) fn header_level(block: &[u8]) -> u8 {
    block[100]
}

pub(super) fn item_at(block: &[u8], slot: usize) -> anyhow::Result<Item> {
    let start = HEADER_SIZE + slot * ITEM_RECORD_SIZE;
    let end = start + ITEM_RECORD_SIZE;
    if end > block.len() {
        bail!("btrfs: item slot {slot} past block end");
    }
    let item = Item::ref_from_bytes(&block[start..end])
        .map_err(|_| anyhow!("btrfs: invalid item record at slot {slot}"))?;
    Ok(*item)
}

pub(super) fn item_data<'b>(block: &'b [u8], item: &Item) -> anyhow::Result<&'b [u8]> {
    let off = HEADER_SIZE + item.offset.get() as usize;
    let end = off + item.size.get() as usize;
    if end > block.len() {
        bail!("btrfs: item data slice past block end");
    }
    Ok(&block[off..end])
}

pub(super) fn key_pointer_at(block: &[u8], slot: usize) -> anyhow::Result<KeyPointer> {
    let start = HEADER_SIZE + slot * KEY_POINTER_SIZE;
    let end = start + KEY_POINTER_SIZE;
    if end > block.len() {
        bail!("btrfs: key-pointer slot {slot} past block end");
    }
    let kp = KeyPointer::ref_from_bytes(&block[start..end])
        .map_err(|_| anyhow!("btrfs: invalid key pointer at slot {slot}"))?;
    Ok(*kp)
}

pub(super) fn cmp_key(a: &Key, b: &Key) -> Ordering {
    a.objectid
        .get()
        .cmp(&b.objectid.get())
        .then_with(|| a.key_type.cmp(&b.key_type))
        .then_with(|| a.offset.get().cmp(&b.offset.get()))
}
