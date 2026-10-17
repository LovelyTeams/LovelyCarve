use super::*;

/// In-memory `SectorSource` for assembling synthetic member images.
struct MemSource(Vec<u8>);
impl SectorSource for MemSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let start = (offset as usize).min(self.0.len());
        let end = (start + len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
}

fn mem_image(bytes: Vec<u8>) -> Arc<DiskImage> {
    Arc::new(
        DiskImage::from_assembled("test".into(), Arc::new(MemSource(bytes)), Vec::new(), 512)
            .expect("assembled image"),
    )
}

/// Parity vote of an equal-size group, read the way the heuristic reads it.
fn group_parity_vote(group: &[Arc<DiskImage>], size: u64) -> f64 {
    let samples = read_group_samples(group, size);
    parity_vote(&samples.iter().collect::<Vec<_>>())
}

/// Data offsets derived for `group`, read the way the heuristic reads them.
fn group_data_offsets(group: &[Arc<DiskImage>]) -> Vec<u64> {
    let size = group.first().map_or(0, |image| image.len());
    let samples = read_group_samples(group, size);
    derive_data_offsets(&samples.iter().collect::<Vec<_>>())
}

#[test]
fn orderings_small_is_factorial() {
    assert_eq!(orderings(3).len(), 6);
    assert_eq!(orderings(4).len(), 24);
    // 5 is now on the exact-permutation path (5! = 120).
    assert_eq!(orderings(5).len(), 120);
}

#[test]
fn orderings_large_is_bounded() {
    // 6 members: 6 rotations + reverse, deduped — past the factorial cap.
    let o = orderings(6);
    assert!(o.len() <= 7);
    assert!(o.contains(&vec![0, 1, 2, 3, 4, 5]));
}

#[test]
fn is_filesystem_classifies_known_kinds() {
    assert!(is_filesystem(PartitionKind::Ext4));
    assert!(is_filesystem(PartitionKind::Ntfs));
    assert!(!is_filesystem(PartitionKind::Swap));
    assert!(!is_filesystem(PartitionKind::Unknown));
}

#[test]
fn parity_vote_is_unanimous_for_xor_member() {
    let size = 8 * 1024 * 1024usize;
    let m0: Vec<u8> = (0..size).map(|i| (i * 31 + 7) as u8).collect();
    let m1: Vec<u8> = (0..size).map(|i| (i * 17 + 3) as u8).collect();
    let m2: Vec<u8> = m0.iter().zip(&m1).map(|(a, b)| a ^ b).collect();
    let group = [mem_image(m0), mem_image(m1), mem_image(m2)];
    // Clean RAID5: every dense row XORs to zero.
    assert_eq!(group_parity_vote(&group, size as u64), 1.0);
}

#[test]
fn parity_vote_low_for_non_raid5() {
    let size = 8 * 1024 * 1024usize;
    // Three independent members: their all-member XOR is almost never zero,
    // so the vote falls well below the RAID5 acceptance threshold.
    let m0: Vec<u8> = (0..size).map(|i| (i * 31 + 7) as u8).collect();
    let m1: Vec<u8> = (0..size).map(|i| (i * 17 + 3) as u8).collect();
    let m2: Vec<u8> = (0..size).map(|i| (i * 5 + 11) as u8).collect();
    let group = [mem_image(m0), mem_image(m1), mem_image(m2)];
    assert!(group_parity_vote(&group, size as u64) < PARITY_VOTE_MIN);
}

#[test]
fn parity_vote_tolerates_one_stale_member() {
    // A clean 3-member RAID5 whose third member (parity) is partially
    // stale: only the second half diverged (a member that dropped midway).
    // The vote must stay above the threshold despite the broken rows.
    let size = 8 * 1024 * 1024usize;
    let m0: Vec<u8> = (0..size).map(|i| (i * 31 + 7) as u8).collect();
    let m1: Vec<u8> = (0..size).map(|i| (i * 17 + 3) as u8).collect();
    let mut m2: Vec<u8> = m0.iter().zip(&m1).map(|(a, b)| a ^ b).collect();
    for b in m2.iter_mut().skip(size / 2) {
        *b ^= 0xFF; // diverge the back half only
    }
    let group = [mem_image(m0), mem_image(m1), mem_image(m2)];
    let vote = group_parity_vote(&group, size as u64);
    assert!(
        vote >= PARITY_VOTE_MIN,
        "vote {vote} should clear threshold"
    );
    assert!(vote < 1.0, "vote {vote} should not be unanimous");
}

/// Encode `logical` into `n` RAID5 members using Right-Asymmetric layout
/// (parity = row % n) with `chunk`-byte stripes and a per-member
/// `data_offset` reserved header — the inverse of `raid5_step` for RA, used
/// to synthesise arrays the heuristic must reconstruct.
fn encode_raid5_ra(logical: &[u8], n: usize, chunk: usize, data_offset: usize) -> Vec<Vec<u8>> {
    let data_disks = n - 1;
    let rows = logical.len().div_ceil(chunk * data_disks);
    let per_member = rows * chunk;
    let mut members = vec![vec![0u8; data_offset + per_member]; n];
    for row in 0..rows {
        let parity = row % n;
        let mut parity_chunk = vec![0u8; chunk];
        for pos in 0..data_disks {
            let slot = if pos < parity { pos } else { pos + 1 };
            let lc = row * data_disks + pos;
            let mut buf = vec![0u8; chunk];
            let start = lc * chunk;
            if start < logical.len() {
                let end = (start + chunk).min(logical.len());
                buf[..end - start].copy_from_slice(&logical[start..end]);
            }
            let off = data_offset + row * chunk;
            members[slot][off..off + chunk].copy_from_slice(&buf);
            for (p, &b) in parity_chunk.iter_mut().zip(buf.iter()) {
                *p ^= b;
            }
        }
        let off = data_offset + row * chunk;
        members[parity][off..off + chunk].copy_from_slice(&parity_chunk);
    }
    members
}

/// Write a minimal valid MBR + NTFS VBR + 13 self-consistent $MFT records
/// into the front of `logical` (all within the first 16 KiB chunk).
fn write_ntfs(logical: &mut [u8]) {
    let part_lba = 1u32;
    let part_sectors = (logical.len() / 512) as u32 - part_lba;
    logical[446 + 4] = 0x07; // partition type NTFS
    logical[446 + 8..446 + 12].copy_from_slice(&part_lba.to_le_bytes());
    logical[446 + 12..446 + 16].copy_from_slice(&part_sectors.to_le_bytes());
    logical[510] = 0x55;
    logical[511] = 0xAA;
    let base = part_lba as usize * 512;
    logical[base + 3..base + 11].copy_from_slice(b"NTFS    ");
    logical[base + 0x0B..base + 0x0D].copy_from_slice(&512u16.to_le_bytes());
    logical[base + 0x0D] = 1; // sectors/cluster
    logical[base + 0x28..base + 0x30].copy_from_slice(&(part_sectors as u64).to_le_bytes());
    logical[base + 0x30..base + 0x38].copy_from_slice(&4u64.to_le_bytes()); // $MFT LCN
    logical[base + 0x40] = 2; // 2 clusters/record = 1024 bytes
    logical[base + 0x48..base + 0x50].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
    let mft = base + 4 * 512;
    for idx in 0u32..13 {
        let r = mft + idx as usize * 1024;
        logical[r..r + 4].copy_from_slice(b"FILE");
        logical[r + 0x2C..r + 0x30].copy_from_slice(&idx.to_le_bytes());
    }
}

/// End-to-end: a fully-present NTFS RAID5 (RA, 32 KiB-sectors chunk, 1 MiB-
/// style reserved offset) is reconstructed by `detect`, exercising the
/// data-offset derivation, parity vote, NTFS $MFT scoring and order search.
#[test]
fn detect_reconstructs_ntfs_raid5_all_present() {
    let chunk = 16384usize;
    let data_offset = 4096usize; // 8 reserved sectors
    let mut logical = vec![0u8; 65536];
    for (ci, fill) in [(1usize, 0x11u8), (2, 0x22), (3, 0x33)] {
        for b in &mut logical[ci * chunk..(ci + 1) * chunk] {
            *b = fill;
        }
    }
    write_ntfs(&mut logical);
    let members = encode_raid5_ra(&logical, 3, chunk, data_offset);
    let imgs: Vec<Arc<DiskImage>> = members.into_iter().map(mem_image).collect();
    let out = detect(&imgs, &HashSet::new());
    assert_eq!(out.arrays.len(), 1, "diagnostics: {:?}", out.diagnostics);
    let arr = &out.arrays[0];
    assert_eq!(arr.level, 5);
    assert_eq!(arr.raid_disks, 3);
    assert_eq!(arr.chunksize_sectors, 32);
    assert_eq!(arr.data_offset_sectors, 8);
    assert_eq!(arr.assembled_bytes, 65536);
    assert!(arr.heuristic);
    assert!(arr.members.iter().all(|m| !m.reconstructed));
    assert_eq!(out.filesystems.len(), 1);
    assert_eq!(out.filesystems[0].kind, "ntfs");
}

#[test]
fn detect_reconstructs_raid5_after_large_erased_reserve() {
    let chunk = 16384usize;
    let data_offset = 9 * 1024 * 1024;
    let mut logical = vec![0u8; 98304];
    for (index, byte) in logical.iter_mut().enumerate() {
        *byte = (index * 31 + (index / chunk) * 17) as u8;
    }
    logical[..1024].fill(0);
    write_ntfs(&mut logical);
    for index in 0u32..64 {
        let record = 512 + 4 * 512 + index as usize * 1024;
        logical[record..record + 4].copy_from_slice(b"FILE");
        logical[record + 0x2c..record + 0x30].copy_from_slice(&index.to_le_bytes());
    }
    let images: Vec<_> = encode_raid5_ra(&logical, 3, chunk, data_offset)
        .into_iter()
        .map(mem_image)
        .collect();
    let output = detect(&images, &HashSet::new());
    assert_eq!(output.arrays.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(output.arrays[0].data_offset_sectors, 18432);
    let source = &output.assembled[0].source;
    assert_eq!(source.read(0, logical.len()).expect("read"), logical);
}

#[test]
fn data_offset_scan_is_bounded_and_skips_empty_members() {
    let image = mem_image(vec![0; 10 * 1024 * 1024]);
    assert_eq!(group_data_offsets(&[image]), vec![0]);
    let mut bytes = vec![0; 20 * 1024 * 1024];
    for offset in (0..bytes.len() - 2048).step_by(DATA_OFFSET_SCAN_STEP as usize) {
        bytes[offset + 510..offset + 512].copy_from_slice(&[0x55, 0xaa]);
    }
    let offsets = group_data_offsets(&[mem_image(bytes)]);
    assert_eq!(offsets.len(), MAX_DATA_OFFSETS);
    assert_eq!(offsets.first(), Some(&0));
}

/// Small NAS-shaped tree: root has only share and lost+found; nested metadata
/// crosses stripe boundaries, and sparse backup superblocks span the volume.
fn ext4_nas_tree() -> Vec<u8> {
    let mut bytes = vec![0; 64 * 1024];
    let superblock = &mut bytes[1024..2048];
    superblock[0..4].copy_from_slice(&32u32.to_le_bytes());
    superblock[4..8].copy_from_slice(&64u32.to_le_bytes());
    superblock[0x14..0x18].copy_from_slice(&1u32.to_le_bytes());
    superblock[0x20..0x24].copy_from_slice(&16u32.to_le_bytes());
    superblock[0x28..0x2c].copy_from_slice(&32u32.to_le_bytes());
    superblock[0x38..0x3a].copy_from_slice(&0xef53u16.to_le_bytes());
    superblock[0x58..0x5a].copy_from_slice(&128u16.to_le_bytes());
    superblock[0x64..0x68].copy_from_slice(&1u32.to_le_bytes());
    superblock[0x68..0x78].fill(0x31);
    let superblock = superblock.to_vec();
    for group in [1usize, 3] {
        let offset = (1 + group * 16) * 1024;
        bytes[offset..offset + 1024].copy_from_slice(&superblock);
        bytes[offset + 0x5a..offset + 0x5c].copy_from_slice(&(group as u16).to_le_bytes());
    }
    bytes[2048 + 8..2048 + 12].copy_from_slice(&4u32.to_le_bytes());
    let directories: [(u32, usize, Vec<(&str, u32)>); 5] = [
        (
            2,
            8,
            vec![(".", 2), ("..", 2), ("share", 3), ("lost+found", 4)],
        ),
        (3, 9, vec![(".", 3), ("..", 2), ("project", 5)]),
        (4, 16, vec![(".", 4), ("..", 2)]),
        (5, 24, vec![(".", 5), ("..", 3), ("nested", 6)]),
        (6, 25, vec![(".", 6), ("..", 5)]),
    ];
    for (inode, block, entries) in directories {
        let offset = 4 * 1024 + (inode as usize - 1) * 128;
        bytes[offset..offset + 2].copy_from_slice(&0x41edu16.to_le_bytes());
        bytes[offset + 0x28..offset + 0x2c].copy_from_slice(&(block as u32).to_le_bytes());
        let count = entries.len();
        let data = &mut bytes[block * 1024..(block + 1) * 1024];
        let mut offset = 0;
        for (index, (name, inode)) in entries.into_iter().enumerate() {
            let record_size = if index + 1 == count {
                1024 - offset
            } else {
                (8 + name.len()).next_multiple_of(4)
            };
            data[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
            data[offset + 4..offset + 6].copy_from_slice(&(record_size as u16).to_le_bytes());
            data[offset + 6] = name.len() as u8;
            data[offset + 8..offset + 8 + name.len()].copy_from_slice(name.as_bytes());
            offset += record_size;
        }
    }
    bytes
}

#[test]
fn ext4_validation_descends_into_a_single_share() {
    let source = Arc::new(MemSource(ext4_nas_tree()));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (4, 4, 0)
    );
    assert!(ext4_backups_match(source.as_ref(), 0, source.size()));
}

#[test]
fn ext4_validation_preserves_directories_with_non_utf8_names() {
    let mut bytes = ext4_nas_tree();
    bytes[8 * 1024 + 32] = 0xff;
    let source = Arc::new(MemSource(bytes));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (4, 4, 0)
    );
}

#[test]
fn ext4_validation_rejects_bad_parent_and_backup_identity() {
    let mut bytes = ext4_nas_tree();
    bytes[24 * 1024 + 12..24 * 1024 + 16].copy_from_slice(&77u32.to_le_bytes());
    bytes[49 * 1024 + 0x68] ^= 1;
    let source = Arc::new(MemSource(bytes));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (2, 3, 0)
    );
    assert!(!ext4_backups_match(source.as_ref(), 0, source.size()));
}

#[test]
fn ext4_directory_cycle_does_not_inflate_score() {
    let mut bytes = ext4_nas_tree();
    // share/project loops back to share itself.
    bytes[9 * 1024 + 24..9 * 1024 + 28].copy_from_slice(&3u32.to_le_bytes());
    let source = Arc::new(MemSource(bytes));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (2, 2, 0)
    );
}

#[test]
fn ext4_file_headers_distinguish_payload_beyond_directory_metadata() {
    let mut bytes = ext4_nas_tree();
    let directory = 9 * 1024;
    bytes[directory + 28..directory + 30].copy_from_slice(&16u16.to_le_bytes());
    let entry = directory + 40;
    bytes[entry..entry + 4].copy_from_slice(&7u32.to_le_bytes());
    bytes[entry + 4..entry + 6].copy_from_slice(&984u16.to_le_bytes());
    bytes[entry + 6] = 10;
    bytes[entry + 8..entry + 18].copy_from_slice(b"sample.png");
    let inode = 4 * 1024 + 6 * 128;
    bytes[inode..inode + 2].copy_from_slice(&0x81a4u16.to_le_bytes());
    bytes[inode + 0x28..inode + 0x2c].copy_from_slice(&40u32.to_le_bytes());
    bytes[40 * 1024..40 * 1024 + 8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let source = Arc::new(MemSource(bytes.clone()));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (4, 5, 1)
    );
    bytes[40 * 1024] ^= 1;
    let source = Arc::new(MemSource(bytes));
    assert_eq!(
        ext4_descend_score(source.clone(), 0, source.size()),
        (4, 5, 0)
    );
}

#[test]
#[ignore = "requires LOVELYCARVE_NAS_FIXTURE_DIR with the four NAS E01 members"]
fn real_nas_erased_mdadm_reconstructs_left_symmetric_raid5() {
    let directory = std::env::var("LOVELYCARVE_NAS_FIXTURE_DIR").expect("fixture directory");
    let images: Vec<_> = ["sdb.E01", "sdc.E01", "sdd.E01", "sde.E01"]
        .into_iter()
        .map(|name| {
            Arc::new(
                DiskImage::open(std::path::Path::new(&directory).join(name)).expect("open member"),
            )
        })
        .collect();
    let output = detect(&images, &HashSet::new());
    assert_eq!(output.arrays.len(), 1, "{:?}", output.diagnostics);
    let array = &output.arrays[0];
    assert_eq!(
        (
            array.level,
            array.layout,
            array.chunksize_sectors,
            array.data_offset_sectors
        ),
        (5, 2, 1024, 18432)
    );
    let member_ids: Vec<_> = array
        .members
        .iter()
        .map(|member| match &member.origin {
            RaidMemberOrigin::Whole { image_id, .. } => image_id.as_str(),
            RaidMemberOrigin::Lv { .. } => panic!("whole-device members expected"),
        })
        .collect();
    assert_eq!(
        member_ids,
        images
            .iter()
            .map(|image| image.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        output.filesystems[0].fsid.as_deref(),
        Some("59494dc69af24768b6a690819d35420e")
    );
}

/// Degraded reconstruction: the disk holding the MBR/MFT chunk is dropped
/// (zeroed) and its column is rebuilt from parity. The MBR lives only on
/// the dropped disk (row-0 parity differs from it), so only a degraded
/// candidate validates — confirming the search's missing-slot path.
#[test]
fn search_reconstructs_degraded_ntfs_raid5() {
    let chunk = 16384usize;
    let data_offset = 4096usize;
    let mut logical = vec![0u8; 65536];
    for b in &mut logical[chunk..2 * chunk] {
        *b = 0x5A; // row-0's other data chunk is non-zero ⇒ parity != MBR
    }
    write_ntfs(&mut logical);
    let mut members = encode_raid5_ra(&logical, 3, chunk, data_offset);
    let member_size = members[0].len() as u64;
    members[1] = vec![0u8; member_size as usize]; // drop the MBR-bearing disk
    let imgs: Vec<Arc<DiskImage>> = members.into_iter().map(mem_image).collect();

    let survivors = search(
        &imgs,
        member_size,
        &orderings(3),
        &[data_offset as u64],
        Some(1),
        Raid5Variants,
    );
    let (winner, _deep, _probed) = pick_winner(survivors, Some(1)).expect("a degraded winner");
    assert_eq!(winner.missing_disk, Some(1));
    assert_eq!(winner.level_tag, 5);
    let fs = largest_validatable_fs(&winner.partitions).expect("ntfs partition");
    assert_eq!(fs.kind, PartitionKind::Ntfs);
}

/// Real-fixture regression: load the five genuine `case141` member images
/// (`1,2,3,7,8.dd`, with `1.dd` the dropped/stale disk) and confirm the
/// heuristic reconstructs the NTFS RAID5 with the ground-truth geometry.
/// Gated on `LOVELYCARVE_RAID_FIXTURE_DIR` pointing at the directory; a
/// no-op when unset so CI without the (multi-GiB) fixtures still passes.
#[test]
#[ignore = "requires LOVELYCARVE_RAID_FIXTURE_DIR with the case141 .dd files"]
fn real_case141_five_members_reconstruct_ntfs_raid5() {
    let Ok(dir) = std::env::var("LOVELYCARVE_RAID_FIXTURE_DIR") else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let imgs: Vec<Arc<DiskImage>> = ["1.dd", "2.dd", "3.dd", "7.dd", "8.dd"]
        .iter()
        .map(|n| {
            Arc::new(
                crate::core::evidence::image::DiskImage::open(dir.join(n))
                    .unwrap_or_else(|e| panic!("open {n}: {e}")),
            )
        })
        .collect();
    let out = detect(&imgs, &HashSet::new());
    eprintln!("diagnostics: {:#?}", out.diagnostics);
    assert_eq!(out.arrays.len(), 1, "diagnostics: {:?}", out.diagnostics);
    let arr = &out.arrays[0];
    assert_eq!(arr.level, 5);
    assert_eq!(arr.raid_disks, 5);
    assert_eq!(arr.chunksize_sectors, 64, "32 KiB chunk");
    assert_eq!(arr.data_offset_sectors, 2048, "1 MiB reserved");
    assert_eq!(arr.assembled_bytes, 4_290_772_992);
    assert_eq!(
        arr.members.iter().filter(|m| m.reconstructed).count(),
        1,
        "exactly one dropped/reconstructed member"
    );
    assert_eq!(out.filesystems.len(), 1);
    assert_eq!(out.filesystems[0].kind, "ntfs");
}

/// Real-fixture regression for the full goal: load ALL EIGHT `case141`
/// images and confirm subset selection auto-isolates the NTFS RAID5 member
/// set `{1,2,3,7,8}` (with `1.dd` the dropped disk) out of the 8, ignoring
/// the unrelated `{4,5,6}` disks. Gated on `LOVELYCARVE_RAID_FIXTURE_DIR`.
#[test]
#[ignore = "requires LOVELYCARVE_RAID_FIXTURE_DIR with the case141 .dd files"]
fn real_case141_all_eight_isolates_ntfs_members() {
    let Ok(dir) = std::env::var("LOVELYCARVE_RAID_FIXTURE_DIR") else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let imgs: Vec<Arc<DiskImage>> = (1..=8)
        .map(|i| {
            let name = format!("{i}.dd");
            Arc::new(
                crate::core::evidence::image::DiskImage::open_with_id(
                    name.clone(),
                    dir.join(&name),
                )
                .unwrap_or_else(|e| panic!("open {name}: {e}")),
            )
        })
        .collect();
    let out = detect(&imgs, &HashSet::new());
    eprintln!("diagnostics:\n{}", out.diagnostics.join("\n"));
    let ntfs_fs = out
        .filesystems
        .iter()
        .find(|f| f.kind == "ntfs")
        .expect("an NTFS array among the 8 disks");
    let arr = out
        .arrays
        .iter()
        .find(|a| a.set_uuid == ntfs_fs.set_uuid)
        .expect("array record for the NTFS filesystem");
    assert_eq!(arr.level, 5);
    assert_eq!(arr.raid_disks, 5);
    assert_eq!(arr.chunksize_sectors, 64);
    assert_eq!(arr.data_offset_sectors, 2048);
    let mut members: Vec<String> = arr
        .members
        .iter()
        .filter_map(|m| match &m.origin {
            RaidMemberOrigin::Whole { image_id, .. } => Some(image_id.clone()),
            RaidMemberOrigin::Lv { .. } => None,
        })
        .collect();
    members.sort();
    assert_eq!(members, ["1.dd", "2.dd", "3.dd", "7.dd", "8.dd"]);
    let dropped: Vec<&str> = arr
        .members
        .iter()
        .filter(|m| m.reconstructed)
        .filter_map(|m| match &m.origin {
            RaidMemberOrigin::Whole { image_id, .. } => Some(image_id.as_str()),
            RaidMemberOrigin::Lv { .. } => None,
        })
        .collect();
    assert_eq!(dropped, ["1.dd"], "1.dd is the dropped/parity-rebuilt disk");
}
