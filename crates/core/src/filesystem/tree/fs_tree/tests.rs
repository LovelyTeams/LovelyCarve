use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

#[test]
fn dispatches_bitlocker_partition_locked_then_unlocked() {
    let Some(path) = std::env::var_os("LOVELYCARVE_BDE_TEST") else {
        return;
    };
    let recovery = std::env::var("LOVELYCARVE_BDE_RECOVERY")
        .expect("LOVELYCARVE_BDE_RECOVERY must accompany LOVELYCARVE_BDE_TEST");

    let image = Arc::new(EvidenceSession::open(path).expect("BDE-bearing E01 should open"));
    let bitlocker_partition = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Bitlocker)
        .cloned()
        .expect("expected a BitLocker partition in the image");

    // Locked: dispatcher returns a single placeholder node.
    let parent_id = format!("part:{}", bitlocker_partition.id);
    let locked_children =
        children_for_image(&image, &parent_id).expect("locked listing should succeed");
    assert_eq!(locked_children.len(), 1);
    assert!(locked_children[0].id.starts_with("bde-locked:part:"));

    // Unlock through the EvidenceSession API and re-list: now we should see real
    // NTFS root entries with `ntfs:part:N:…` ids.
    image
        .unlock_bitlocker_partition(bitlocker_partition.id, Some(&recovery), None)
        .expect("BitLocker unlock should succeed");
    let unlocked_children =
        children_for_image(&image, &parent_id).expect("unlocked listing should succeed");
    assert!(!unlocked_children.is_empty());
    assert!(unlocked_children
        .iter()
        .all(|node| node.id.starts_with("ntfs:part:")));
}

#[test]
fn lists_real_vmdk_ntfs_root_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_VMDK_TEST") else {
        return;
    };
    let image = Arc::new(EvidenceSession::open(path).expect("VMDK image should open"));
    let ntfs_partitions = image
        .partitions
        .iter()
        .filter(|partition| partition.kind == PartitionKind::Ntfs)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !ntfs_partitions.is_empty(),
        "expected at least one NTFS partition in {}",
        image.path.display()
    );
    for partition in ntfs_partitions {
        let parent_id = format!("part:{}", partition.id);
        let children =
            children_for_image(&image, &parent_id).expect("NTFS root listing should succeed");
        assert!(
            !children.is_empty(),
            "partition {} should have at least one root child",
            partition.id
        );
        assert!(children
            .iter()
            .all(|node| node.id.starts_with("ntfs:part:")));
        if let Some(file) = children.iter().find(|node| node.kind == NodeKind::File) {
            let data =
                read_file_limited(&image, &file.id, 4096).expect("NTFS file read should succeed");
            assert!(!data.is_empty(), "NTFS file read should yield bytes");
        }
    }
}

#[test]
fn lists_real_e01_ext4_roots_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_E01_TEST") else {
        return;
    };
    let image = Arc::new(EvidenceSession::open(path).expect("E01 image should open"));
    let partitions = children_for_image(&image, "").expect("partitions should list");
    assert!(partitions.iter().any(|node| node.kind == NodeKind::Dir));

    let ext4_partitions = partitions
        .iter()
        .filter(|node| node.id.starts_with("part:"))
        .collect::<Vec<_>>();
    assert!(!ext4_partitions.is_empty());
    for partition in ext4_partitions {
        let root_children =
            children_for_image(&image, &partition.id).expect("ext4 root should list");
        assert!(
            !root_children.is_empty(),
            "{} should have at least one root child",
            partition.name
        );
    }

    let lvm_partition = partitions
        .iter()
        .find(|node| node.id == "part:2")
        .expect("LVM partition should be present");
    let logical_volumes =
        children_for_image(&image, &lvm_partition.id).expect("LVM logical volumes should list");
    assert!(!logical_volumes.is_empty());
    let first_ext4_lv = logical_volumes
        .iter()
        .find(|node| node.kind == NodeKind::Dir)
        .expect("an Ext4 logical volume should be present");
    let lv_root_children = children_for_image(&image, &first_ext4_lv.id)
        .expect("Ext4 logical volume root should list");
    assert!(!lv_root_children.is_empty());
}

/// ext4 with 128-byte inodes **and** `metadata_csum` enabled — the
/// layout mke2fs emits for small filesystems, as seen on iKuai /
/// OpenWRT soft-router images — must list and traverse without
/// tripping the inode-checksum verifier. The vendored `ext4-view`
/// crate used to reject 128-byte inodes outright as `InodeTruncated`,
/// and once that was relaxed it compared the full 32-bit CRC instead
/// of masking to the on-disk 16-bit `l_i_checksum_lo` the way the
/// kernel's `ext4_inode_csum_verify` does (see
/// `vendor/ext4-view/src/inode.rs`). Gate on a dedicated env var so CI
/// stays hermetic; point it at any image whose ext4 volumes use
/// 128-byte inodes (`s_inode_size == 128`).
#[test]
fn traverses_ext4_with_128_byte_inodes_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_EXT4_128_TEST") else {
        return;
    };
    let image =
        Arc::new(EvidenceSession::open(path).expect("128-byte-inode ext4 image should open"));
    let partitions = children_for_image(&image, "").expect("partitions should list");
    let ext4_partitions = partitions
        .iter()
        .filter(|node| node.id.starts_with("part:"))
        .collect::<Vec<_>>();
    assert!(
        !ext4_partitions.is_empty(),
        "image should expose at least one ext4 partition"
    );

    for partition in ext4_partitions {
        // Listing the root reads inode 2 with checksum verification —
        // the exact operation that failed on 128-byte inodes.
        let root = children_for_image(&image, &partition.id)
            .unwrap_or_else(|err| panic!("{} root should list: {err:#}", partition.name));
        assert!(
            !root.is_empty(),
            "{} root should have children",
            partition.name
        );

        // Descend breadth-first (bounded) so many more inode checksums
        // get verified, not just the root's.
        let mut queue: Vec<FsNode> = root;
        let mut dirs_visited = 0;
        while let Some(node) = queue.pop() {
            if node.kind != NodeKind::Dir {
                continue;
            }
            dirs_visited += 1;
            if dirs_visited > 20 {
                break;
            }
            let kids = children_for_image(&image, &node.id)
                .unwrap_or_else(|err| panic!("dir '{}' should list: {err:#}", node.name));
            queue.extend(kids);
        }
    }
}
