//! Volume initialization and validation.
use super::*;

impl<'a> BtrfsVolume<'a> {
    pub fn open(
        image: &'a DiskImage,
        source: BtrfsSource,
        base_offset: u64,
        length_bytes: u64,
        root_parent_id: String,
    ) -> anyhow::Result<Self> {
        let (base_offset, layout) = match source {
            BtrfsSource::Lvm { part_id, lv_index } => {
                let partition = find_partition(image, &part_id.to_string())?;
                let volume = crate::filesystem::volumes::lvm::LvmVolume::open(image, partition)?;
                volume
                    .logical_volumes
                    .get(lv_index)
                    .ok_or_else(|| anyhow!("LVM LV missing"))?
                    .reader_geometry()?
            }
            BtrfsSource::Partition(_) => (base_offset, None),
        };
        let sb = crate::filesystem::volumes::lvm::read_backing_range(
            image,
            layout.as_ref(),
            base_offset + BTRFS_PRIMARY_SUPERBLOCK,
            4096,
        )
        .context("btrfs: read primary superblock")?;
        if sb.len() < 4096 {
            bail!("btrfs: primary superblock truncated ({} bytes)", sb.len());
        }
        let magic = read_u64(&sb, 64);
        // `_BHRfS_M` little-endian u64: bytes are 0x5F 0x42 0x48 0x52 0x66 0x53 0x5F 0x4D
        let expected_magic = u64::from_le_bytes(*b"_BHRfS_M");
        if magic != expected_magic {
            bail!("btrfs: superblock magic mismatch (0x{magic:016x})");
        }

        let root = read_u64(&sb, 80);
        let chunk_root = read_u64(&sb, 88);
        let root_dir_objectid = read_u64(&sb, 128);
        let sectorsize = read_u32(&sb, 144);
        let nodesize = read_u32(&sb, 148);
        let sys_chunk_array_size = read_u32(&sb, 160) as usize;
        let root_level = sb[198];
        let chunk_root_level = sb[199];
        let label = decode_label(&sb[299..299 + 256]);

        if !nodesize.is_power_of_two() || nodesize < 4096 {
            bail!("btrfs: implausible nodesize {nodesize}");
        }
        if sys_chunk_array_size > 2048 {
            bail!("btrfs: sys_chunk_array_size {sys_chunk_array_size} > 2048");
        }

        let bootstrap = parse_sys_chunk_array(&sb[811..811 + sys_chunk_array_size])
            .context("btrfs: parse sys_chunk_array")?;
        if bootstrap.is_empty() {
            bail!("btrfs: sys_chunk_array did not yield any system chunk mappings");
        }

        let mut chunks: Vec<ChunkMapping> = Vec::new();
        {
            let walker = TreeWalker::new(
                image,
                base_offset,
                &bootstrap,
                nodesize as usize,
                layout.as_ref(),
            );
            walker.iter_range(
                chunk_root,
                chunk_root_level,
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
                    if key.key_type == KEY_CHUNK_ITEM {
                        if let Some(map) = parse_chunk_item(key, data) {
                            chunks.push(map);
                        }
                    }
                    Ok(true)
                },
            )?;
        }
        for cm in &bootstrap {
            if !chunks.iter().any(|c| c.logical == cm.logical) {
                chunks.push(*cm);
            }
        }
        chunks.sort_by_key(|c| c.logical);
        chunks.dedup_by_key(|c| c.logical);

        let mut subvolumes: BTreeMap<u64, SubvolEntry> = BTreeMap::new();
        {
            let walker = TreeWalker::new(
                image,
                base_offset,
                &chunks,
                nodesize as usize,
                layout.as_ref(),
            );
            walker.iter_range(
                root,
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
                    let oid = key.objectid.get();
                    let is_real_subvol = oid == FS_TREE_OBJECTID
                        || (FIRST_FREE_OBJECTID..=LAST_FREE_OBJECTID).contains(&oid);
                    match key.key_type {
                        KEY_ROOT_ITEM if is_real_subvol => {
                            if let Some((bytenr, level)) = parse_root_item(data) {
                                subvolumes
                                    .entry(oid)
                                    .and_modify(|e| {
                                        e.bytenr = bytenr;
                                        e.level = level;
                                    })
                                    .or_insert(SubvolEntry {
                                        bytenr,
                                        level,
                                        name: None,
                                        parent_subvols: Vec::new(),
                                    });
                            }
                        }
                        // ROOT_REF 的 key 是 (父子卷 ROOT_REF 子子卷)，payload 里的
                        // name 是子子卷在父目录里的名字——注意方向：oid 是父，offset
                        // 才是被引用的子卷。
                        KEY_ROOT_REF => {
                            let parent_subvol = oid;
                            let child_subvol = key.offset.get();
                            let parent_ok = parent_subvol == FS_TREE_OBJECTID
                                || (FIRST_FREE_OBJECTID..=LAST_FREE_OBJECTID)
                                    .contains(&parent_subvol);
                            let child_ok =
                                (FIRST_FREE_OBJECTID..=LAST_FREE_OBJECTID).contains(&child_subvol);
                            if parent_ok && child_ok {
                                if let Some(name) = parse_root_ref_name(data) {
                                    subvolumes
                                        .entry(child_subvol)
                                        .and_modify(|e| {
                                            if e.name.is_none() {
                                                e.name = Some(name.clone());
                                            }
                                            e.parent_subvols.push(parent_subvol);
                                        })
                                        .or_insert(SubvolEntry {
                                            bytenr: 0,
                                            level: 0,
                                            name: Some(name),
                                            parent_subvols: vec![parent_subvol],
                                        });
                                }
                            }
                        }
                        _ => {}
                    }
                    Ok(true)
                },
            )?;
        }
        // Drop incomplete subvolume entries (a ROOT_REF with no matching
        // ROOT_ITEM in this tree is meaningless; the FS root itself can't
        // be located).
        subvolumes.retain(|_, entry| entry.bytenr != 0);
        if !subvolumes.contains_key(&FS_TREE_OBJECTID) {
            bail!("btrfs: default FS tree (objectid {FS_TREE_OBJECTID}) not found in root tree");
        }

        let default_subvol = FS_TREE_OBJECTID;
        let root_dir_inode = if root_dir_objectid >= FIRST_FREE_OBJECTID {
            root_dir_objectid
        } else {
            FIRST_FREE_OBJECTID
        };

        Ok(Self {
            image,
            layout,
            source,
            base_offset,
            length_bytes,
            nodesize,
            sectorsize,
            label,
            chunks,
            subvolumes,
            default_subvol,
            root_dir_inode,
            root_parent_id,
        })
    }
}
