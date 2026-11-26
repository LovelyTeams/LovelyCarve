//! Volume initialization and validation.
use super::*;
use crate::core::common::error::Failure;

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
                let volume =
                    crate::core::filesystem::volumes::lvm::LvmVolume::open(image, partition)?;
                volume
                    .logical_volumes
                    .get(lv_index)
                    .ok_or_else(|| anyhow!(Failure::not_found("LVM LV missing")))?
                    .reader_geometry()?
            }
            BtrfsSource::Partition(_) => (base_offset, None),
        };
        let (sb, devices) = super::devices::DeviceSet::open(
            image,
            layout.as_ref(),
            source,
            base_offset,
            length_bytes,
        )?;
        let root = read_u64(&sb, 80);
        let chunk_root = read_u64(&sb, 88);
        let root_dir_objectid = read_u64(&sb, 128);
        let sectorsize = read_u32(&sb, 144);
        let nodesize = read_u32(&sb, 148);
        let sys_chunk_array_size = read_u32(&sb, 160) as usize;
        let root_level = sb[198];
        let chunk_root_level = sb[199];
        let label = decode_label(&sb[299..299 + 256]);

        if !nodesize.is_power_of_two() || !(4096..=65536).contains(&nodesize) {
            bail!("btrfs: implausible nodesize {nodesize}");
        }
        if sys_chunk_array_size > 2048 {
            bail!("btrfs: sys_chunk_array_size {sys_chunk_array_size} > 2048");
        }

        let bootstrap =
            ChunkMapping::from_system_array(&sb[811..811 + sys_chunk_array_size], &devices)
                .context("btrfs: parse sys_chunk_array")?;
        if bootstrap.is_empty() {
            bail!("btrfs: sys_chunk_array did not yield any system chunk mappings");
        }

        let mut chunks: Vec<ChunkMapping> = Vec::new();
        {
            let walker = TreeWalker::new(image, &bootstrap, nodesize as usize, layout.as_ref());
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
                        let (map, used) = ChunkMapping::parse(key.offset.get(), data, &devices)?;
                        anyhow::ensure!(
                            used == data.len(),
                            Failure::corrupt("btrfs: trailing bytes in chunk item")
                        );
                        chunks.push(map);
                    }
                    Ok(true)
                },
            )?;
        }
        super::chunks::sort_chunks(&mut chunks)?;
        // Match bootstrap items against the selected chunk tree. A legacy
        // system-only map may be absent there, but cannot disagree with it.
        for map in bootstrap {
            match chunks.binary_search_by_key(&map.logical, |chunk| chunk.logical) {
                Ok(index) => anyhow::ensure!(
                    chunks[index] == map,
                    Failure::corrupt("btrfs: system chunk disagrees with chunk tree")
                ),
                Err(index) => chunks.insert(index, map),
            }
        }
        super::chunks::sort_chunks(&mut chunks)?;

        let mut subvolumes: BTreeMap<u64, SubvolEntry> = BTreeMap::new();
        {
            let walker = TreeWalker::new(image, &chunks, nodesize as usize, layout.as_ref());
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
            bail!(Failure::corrupt(format!(
                "btrfs: default FS tree (objectid {FS_TREE_OBJECTID}) not found in root tree"
            )));
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
