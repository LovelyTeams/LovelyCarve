//! Regression coverage for indexed navigation and its non-indexed root boundary.

use super::{locate_node, FsLocateNode, FsLocateResult};
use crate::commands::filesystem::filesystem_snapshot_index::SnapshotIndexTarget;
use crate::core::catalog::filedb::filedb_store::{self, test_support::entry};
use crate::core::catalog::snapshot::volume_snapshot::SnapshotEntry;
use crate::core::evidence::image::{Partition, PartitionKind};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, NodeKind};
use std::path::Path;
use std::sync::Arc;

fn frame(id: &str, name: &str) -> FsLocateNode {
    FsLocateNode {
        id: id.into(),
        name: name.into(),
        kind: "dir".into(),
    }
}

fn fixture() -> (tempfile::TempDir, Arc<EvidenceSession>) {
    let case = tempfile::tempdir().expect("temporary case");
    let path = case.path().join("image.raw");
    std::fs::write(&path, vec![0; 4096]).expect("write raw fixture");
    let mut image = crate::core::evidence::image::DiskImage::open_with_id("evidence".into(), path)
        .expect("open fixture");
    image.partitions = vec![Partition {
        id: 0,
        kind: PartitionKind::Ext4,
        start_lba: 0,
        length_lba: 8,
        label: Some("Linux (系统) · 主卷".into()),
        unique_guid: None,
    }];
    (case, Arc::new(EvidenceSession::from(image)))
}

/// Builds `entries` into a `part:0` index file named `name` under `dir`.
fn index_of(dir: &Path, name: &str, entries: &[SnapshotEntry]) -> Vec<SnapshotIndexTarget> {
    let index = dir.join(format!("{name}.db"));
    filedb_store::build_index_from_entries(
        entries,
        &index,
        &filedb_store::test_support::source("part:0"),
    )
    .expect("build index");
    vec![SnapshotIndexTarget {
        index,
        volume_key: "part:0".into(),
        vol_id: 0,
    }]
}

#[test]
fn indexed_file_location_stops_at_unindexed_partition_or_root() {
    let (case, image) = fixture();
    for parent in ["part:0", "root", "unsupported:0"] {
        let targets = index_of(
            case.path(),
            &parent.replace(':', "-"),
            &[
                entry(parent, "ext4:part:0:12", "etc", NodeKind::Dir, "/etc"),
                entry(
                    "ext4:part:0:12",
                    "ext4:part:0:13",
                    "hosts",
                    NodeKind::File,
                    "/etc/hosts",
                ),
            ],
        );
        let mut frames = vec![frame("root", "/")];
        if parent != "root" {
            frames.push(frame("part:0", "Linux_系统_主卷"));
        }
        frames.push(frame("ext4:part:0:12", "etc"));
        assert_eq!(
            locate_node(&image, &targets, "ext4:part:0:13").expect("locate file"),
            FsLocateResult {
                node_id: "ext4:part:0:13".into(),
                volume_key: "part:0".into(),
                frames,
                file_id: Some("ext4:part:0:13".into()),
            }
        );
    }
}

#[test]
fn indexed_directory_location_retains_the_target_directory() {
    let (case, image) = fixture();
    let targets = index_of(
        case.path(),
        "directory",
        &[entry(
            "part:0",
            "ext4:part:0:12",
            "etc",
            NodeKind::Dir,
            "/etc",
        )],
    );
    assert_eq!(
        locate_node(&image, &targets, "ext4:part:0:12").expect("locate directory"),
        FsLocateResult {
            node_id: "ext4:part:0:12".into(),
            volume_key: "part:0".into(),
            frames: vec![
                frame("root", "/"),
                frame("part:0", "Linux_系统_主卷"),
                frame("ext4:part:0:12", "etc")
            ],
            file_id: None,
        }
    );
}

#[test]
fn absent_indexed_target_reports_missing_node_instead_of_sqlite_error() {
    let (case, image) = fixture();
    let targets = index_of(
        case.path(),
        "absent",
        &[entry(
            "part:0",
            "ext4:part:0:12",
            "etc",
            NodeKind::Dir,
            "/etc",
        )],
    );
    for targets in [targets.as_slice(), &[]] {
        let error = locate_node(&image, targets, "ext4:part:0:999").expect_err("missing target");
        assert_eq!(error.to_string(), "节点不在文件索引中");
    }
}

#[test]
fn virtual_nodes_do_not_require_an_index_record() {
    let (_case, image) = fixture();
    for node_id in ["unalloc:part:0", "sysfile:part:0:test"] {
        assert_eq!(
            locate_node(&image, &[], node_id).expect("locate virtual node"),
            FsLocateResult {
                node_id: node_id.into(),
                volume_key: "part:0".into(),
                frames: vec![frame("root", "/"), frame("part:0", "Linux_系统_主卷")],
                file_id: Some(node_id.into()),
            }
        );
    }
}

#[test]
#[ignore = "set LOVELYCARVE_LOCATE_VMDK_TEST to the reported Kali VMDK"]
fn real_vmdk_locates_reported_ext4_inode() {
    let path = std::env::var_os("LOVELYCARVE_LOCATE_VMDK_TEST").expect("VMDK test path");
    let image = Arc::new(EvidenceSession::open(&path).expect("open VMDK read-only"));
    let case = tempfile::tempdir().expect("isolated test index");
    let parts = [
        "var",
        "lib",
        "docker",
        "overlay2",
        "909e5a82c694b09b7869b9b3f3ed26ba3dadf93b3cd9d9ef2bc59938a600dbf1",
        "diff",
        "usr",
        "bin",
        "ipcs",
    ];
    let mut parent = "part:0".to_string();
    let mut full_path = String::new();
    let mut entries = Vec::new();
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.id == 0)
        .expect("partition 0");
    let partition_name = fs_tree::partition_node(&image, partition).name;
    let mut expected_frames = vec![frame("root", "/"), frame("part:0", &partition_name)];
    for name in parts {
        let node = fs_tree::children_for_image(&image, &parent)
            .expect("read VMDK directory")
            .into_iter()
            .find(|node| node.name == name && !node.deleted)
            .expect("path component");
        full_path.push('/');
        full_path.push_str(name);
        if node.kind == NodeKind::Dir {
            expected_frames.push(frame(&node.id, name));
        }
        let next_parent = node.id.clone();
        entries.push(SnapshotEntry {
            parent,
            path: full_path.clone(),
            node,
        });
        parent = next_parent;
    }
    assert_eq!(parent, "ext4:part:0:3553653");
    let targets = index_of(case.path(), "vmdk", &entries);
    // Reopen the image, as when revisiting search/timeline results.
    let reopened =
        Arc::new(EvidenceSession::open_with_id(image.id.clone(), path).expect("reopen VMDK"));
    let located = locate_node(&reopened, &targets, &parent).expect("locate reported inode");
    assert_eq!(
        located,
        FsLocateResult {
            node_id: parent.clone(),
            volume_key: "part:0".into(),
            frames: expected_frames,
            file_id: Some(parent.clone()),
        }
    );
    let directory = located.frames.last().expect("parent frame");
    assert!(fs_tree::children_for_image(&reopened, &directory.id)
        .expect("navigate to file")
        .iter()
        .any(|node| node.id == parent && node.name == "ipcs"));
    let bytes = fs_tree::read_file_range(&reopened, &parent, 0, 4).expect("read target signature");
    assert_eq!(bytes, b"\x7fELF");
}
