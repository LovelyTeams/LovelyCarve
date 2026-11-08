use std::path::{Path, PathBuf};

use super::*;
use crate::catalog::filedb::filedb_store::test_support::build_test_index;
use crate::catalog::snapshot::volume_snapshot::SnapshotEntry;
use crate::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};

/// 2026-01-01T00:00:00Z: the build time the tests measure plausibility from.
const NOW: i64 = 1_767_225_600;

fn row(parent: &str, id: &str, name: &str, kind: NodeKind, size: u64) -> SnapshotEntry {
    SnapshotEntry {
        parent: parent.to_string(),
        node: FsNode {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: Some(parent.to_string()),
            kind,
            size,
            deleted: false,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        },
        path: format!("/{name}"),
    }
}

fn dir(parent: &str, id: &str, name: &str) -> SnapshotEntry {
    row(parent, id, name, NodeKind::Dir, 0)
}

fn file(parent: &str, id: &str, name: &str, size: u64) -> SnapshotEntry {
    row(parent, id, name, NodeKind::File, size)
}

fn deleted(mut entry: SnapshotEntry) -> SnapshotEntry {
    entry.node.deleted = true;
    entry
}

fn modified(mut entry: SnapshotEntry, time: &str) -> SnapshotEntry {
    entry.node.timestamps.modified = Some(time.to_string());
    entry
}

/// A small Windows-like volume in walk order: every directory row comes
/// before its own entries.
fn volume() -> Vec<SnapshotEntry> {
    vec![
        dir("part:0", "ntfs:part:0:100", "Windows"),
        dir("part:0", "ntfs:part:0:200", "Users"),
        file("part:0", "ntfs:part:0:300", "pagefile.sys", 2_000),
        dir("ntfs:part:0:100", "ntfs:part:0:101", "System32"),
        file("ntfs:part:0:100", "ntfs:part:0:104", "notepad.exe", 50),
        modified(
            file("ntfs:part:0:200", "ntfs:part:0:201", "photo.jpg", 300),
            "2025-12-20T08:00:00+00:00",
        ),
        deleted(modified(
            file("ntfs:part:0:200", "ntfs:part:0:202", "gone.docx", 900),
            "2025-12-31T23:00:00+00:00",
        )),
        file("ntfs:part:0:101", "ntfs:part:0:102", "kernel32.dll", 600),
        modified(
            file("ntfs:part:0:101", "ntfs:part:0:103", "ntdll.dll", 400),
            "2025-11-02T10:00:00+00:00",
        ),
    ]
}

fn build(directory: &Path, entries: &[SnapshotEntry]) -> (PathBuf, SizeTree) {
    let index = build_test_index(directory, "part:0", entries);
    let tree = build_size_tree(&index, NOW).expect("build size tree");
    (index, tree)
}

fn options(depth: u32, deleted: DeletedEntries) -> ViewOptions {
    ViewOptions::new(depth, SizeMetric::Bytes, deleted, 0.0)
}

/// A view as names and totals, for whole-object comparison.
#[derive(Debug, PartialEq, Eq)]
struct Shape {
    name: String,
    bytes: u64,
    files: u64,
    dirs: u64,
    children: Option<Vec<Shape>>,
    others: Option<SizeTreeOthers>,
}

fn shape(entry: &SizeTreeEntry) -> Shape {
    Shape {
        name: entry
            .node
            .as_ref()
            .map_or_else(|| "/".to_string(), |node| node.name.clone()),
        bytes: entry.bytes,
        files: entry.files,
        dirs: entry.dirs,
        children: entry
            .children
            .as_ref()
            .map(|children| children.iter().map(shape).collect()),
        others: entry.others,
    }
}

fn leaf(name: &str, bytes: u64) -> Shape {
    Shape {
        name: name.to_string(),
        bytes,
        files: 1,
        dirs: 0,
        children: None,
        others: None,
    }
}

fn read_shape(index: &Path, tree: &SizeTree, start: &str, options: &ViewOptions) -> Shape {
    let mut view = tree.view(start, options).expect("view");
    read_view_nodes(index, &mut view).expect("read nodes");
    shape(&view.root)
}

#[test]
fn totals_cover_every_level_and_children_come_largest_first() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (index, tree) = build(directory.path(), &volume());

    let live = read_shape(
        &index,
        &tree,
        "part:0",
        &options(3, DeletedEntries::Exclude),
    );
    assert_eq!(
        live,
        Shape {
            name: "/".to_string(),
            bytes: 3_350,
            files: 5,
            dirs: 3,
            children: Some(vec![
                leaf("pagefile.sys", 2_000),
                Shape {
                    name: "Windows".to_string(),
                    bytes: 1_050,
                    files: 3,
                    dirs: 1,
                    children: Some(vec![
                        Shape {
                            name: "System32".to_string(),
                            bytes: 1_000,
                            files: 2,
                            dirs: 0,
                            children: Some(
                                vec![leaf("kernel32.dll", 600), leaf("ntdll.dll", 400),]
                            ),
                            others: None,
                        },
                        leaf("notepad.exe", 50),
                    ]),
                    others: None,
                },
                Shape {
                    name: "Users".to_string(),
                    bytes: 300,
                    files: 1,
                    dirs: 0,
                    children: Some(vec![leaf("photo.jpg", 300)]),
                    others: None,
                },
            ]),
            others: None,
        }
    );

    // With deleted entries, Users outgrows Windows.
    let all = read_shape(
        &index,
        &tree,
        "part:0",
        &options(1, DeletedEntries::Include),
    );
    let order: Vec<(String, u64)> = all
        .children
        .expect("children")
        .into_iter()
        .map(|child| (child.name, child.bytes))
        .collect();
    assert_eq!(
        order,
        vec![
            ("pagefile.sys".to_string(), 2_000),
            ("Users".to_string(), 1_200),
            ("Windows".to_string(), 1_050),
        ]
    );
    assert_eq!((all.bytes, all.files), (4_250, 6));
}

#[test]
fn a_view_starts_anywhere_and_stops_at_its_depth() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (index, tree) = build(directory.path(), &volume());

    let windows = read_shape(
        &index,
        &tree,
        "ntfs:part:0:100",
        &options(1, DeletedEntries::Exclude),
    );
    assert_eq!(windows.name, "Windows");
    let children = windows.children.expect("children");
    assert_eq!(children[0].name, "System32");
    assert_eq!(
        children[0].children, None,
        "a directory past the depth stays closed"
    );
    assert!(
        tree.view("ntfs:part:0:102", &options(1, DeletedEntries::Exclude))
            .is_none(),
        "a file is no directory"
    );
    assert!(tree
        .view("ntfs:part:0:999", &options(1, DeletedEntries::Exclude))
        .is_none());
}

#[test]
fn a_hard_linked_file_is_charged_to_its_first_name_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let entries = vec![
        dir("part:0", "ntfs:part:0:40", "a"),
        dir("part:0", "ntfs:part:0:41", "b"),
        file("ntfs:part:0:40", "ntfs:part:0:500", "linked.bin", 700),
        file("ntfs:part:0:41", "ntfs:part:0:500", "linked.bin", 700),
    ];
    let (index, tree) = build(directory.path(), &entries);
    let root = read_shape(
        &index,
        &tree,
        "part:0",
        &options(2, DeletedEntries::Exclude),
    );
    assert_eq!(root.bytes, 700);
    let children = root.children.expect("children");
    assert_eq!(
        children.len(),
        1,
        "the second name weighs nothing and is not drawn"
    );
    assert_eq!((children[0].name.as_str(), children[0].bytes), ("a", 700));
}

#[test]
fn the_bad_cluster_stream_is_left_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let bad = format!("ntfs-ads:part:0:8:{}", hex::encode("$Bad"));
    let entries = vec![
        file("part:0", "ntfs:part:0:8", "$BadClus", 0),
        file("part:0", &bad, "$BadClus:$Bad (数据流)", 1 << 40),
        file("part:0", "ntfs:part:0:300", "data.bin", 10),
    ];
    let (_index, tree) = build(directory.path(), &entries);
    let view = tree
        .view("part:0", &options(1, DeletedEntries::Exclude))
        .expect("view");
    assert_eq!(view.root.bytes, 10);
}

#[test]
fn small_and_surplus_children_merge_into_others() {
    let directory = tempfile::tempdir().expect("tempdir");
    let entries: Vec<SnapshotEntry> = (1..=10u64)
        .map(|size| {
            file(
                "part:0",
                &format!("ntfs:part:0:{}", 100 + size),
                &format!("f{size}.bin"),
                size,
            )
        })
        .collect();
    let (_index, tree) = build(directory.path(), &entries);

    let mut capped = options(1, DeletedEntries::Exclude);
    capped.max_children = 4;
    let view = tree.view("part:0", &capped).expect("view");
    let kept: Vec<u64> = view
        .root
        .children
        .as_ref()
        .expect("children")
        .iter()
        .map(|child| child.bytes)
        .collect();
    assert_eq!(kept, vec![10, 9, 8, 7]);
    assert_eq!(
        view.root.others,
        Some(SizeTreeOthers {
            count: 6,
            bytes: 21,
            files: 6,
        })
    );

    // A child worth less than a tenth of the 55-byte root is too small to draw.
    let shared = ViewOptions::new(1, SizeMetric::Bytes, DeletedEntries::Exclude, 0.1);
    let view = tree.view("part:0", &shared).expect("view");
    let kept: Vec<u64> = view
        .root
        .children
        .as_ref()
        .expect("children")
        .iter()
        .map(|child| child.bytes)
        .collect();
    assert_eq!(kept, vec![10, 9, 8, 7, 6]);
    assert_eq!(view.root.others.map(|others| others.count), Some(5));
}

#[test]
fn files_metric_ranks_by_count() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_index, tree) = build(directory.path(), &volume());
    let by_files = ViewOptions::new(1, SizeMetric::Files, DeletedEntries::Exclude, 0.0);
    let view = tree.view("part:0", &by_files).expect("view");
    let files: Vec<u64> = view
        .root
        .children
        .as_ref()
        .expect("children")
        .iter()
        .map(|child| child.files)
        .collect();
    assert_eq!(files, vec![3, 1, 1], "Windows holds the most files");
}

#[test]
fn directories_that_no_longer_fit_stay_closed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (_index, tree) = build(directory.path(), &volume());
    let mut tight = options(3, DeletedEntries::Exclude);
    tight.max_entries = 5;
    let view = tree.view("part:0", &tight).expect("view");
    let children = view
        .root
        .children
        .as_ref()
        .expect("the view's directory always opens");
    // Root and its 3 children make 4 entries. Windows' 2 children would make
    // 6, so it stays closed; Users' 1 child makes 5 and fits.
    let opened: Vec<(u64, bool)> = children
        .iter()
        .map(|child| (child.bytes, child.children.is_some()))
        .collect();
    assert_eq!(opened, vec![(2_000, false), (1_050, false), (300, true)]);
}

#[test]
fn the_newest_write_ignores_deleted_and_impossible_times() {
    let directory = tempfile::tempdir().expect("tempdir");
    let mut entries = volume();
    entries.push(modified(
        file("part:0", "ntfs:part:0:400", "future.log", 1),
        "2031-01-01T00:00:00+00:00",
    ));
    let (_index, tree) = build(directory.path(), &entries);
    let view = tree
        .view("part:0", &options(1, DeletedEntries::Include))
        .expect("view");
    // photo.jpg: the deleted gone.docx is newer, future.log is impossible.
    assert_eq!(view.newest_write, Some(1_766_217_600));
    let users = view
        .root
        .children
        .as_ref()
        .expect("children")
        .iter()
        .find(|child| child.bytes == 1_200)
        .expect("Users");
    assert_eq!(
        users.newest,
        Some(1_767_222_000),
        "a directory's newest write includes what it shows"
    );
}

#[test]
fn only_walked_ntfs_directories_are_served() {
    let directory = tempfile::tempdir().expect("tempdir");
    let ntfs = build_test_index(directory.path(), "part:0", &volume());
    assert_eq!(ntfs_directory(&ntfs, "part:0").expect("root"), Some(true));
    assert_eq!(
        ntfs_directory(&ntfs, "ntfs:part:0:100").expect("dir"),
        Some(true)
    );
    assert_eq!(
        ntfs_directory(&ntfs, "ntfs:part:0:999").expect("unknown"),
        None
    );

    let other = tempfile::tempdir().expect("tempdir");
    let ext4 = build_test_index(
        other.path(),
        "part:1",
        &[
            dir("part:1", "ext4:part:1:2", "etc"),
            file("ext4:part:1:2", "ext4:part:1:3", "passwd", 1),
        ],
    );
    assert_eq!(ntfs_directory(&ext4, "part:1").expect("root"), Some(false));
    assert_eq!(
        ntfs_directory(&ext4, "ext4:part:1:2").expect("dir"),
        Some(false)
    );
}

#[test]
fn volume_parts_of_ntfs_ids() {
    use super::build::ntfs_volume_of;
    assert_eq!(ntfs_volume_of("ntfs:part:2:1234"), Some("part:2"));
    assert_eq!(ntfs_volume_of("ntfs:lvm:0:1:77"), Some("lvm:0:1"));
    assert_eq!(
        ntfs_volume_of("ntfs-ads:part:2:1234:2442616400"),
        Some("part:2")
    );
    assert_eq!(ntfs_volume_of("ext4:part:1:2"), None);
}
