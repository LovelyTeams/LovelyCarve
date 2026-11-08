//! Snapshot format, publication and traversal regressions.

use super::*;

fn node(id: &str, name: &str, kind: NodeKind) -> FsNode {
    FsNode {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: None,
        kind,
        size: 0,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }
}

fn header() -> SnapshotHeader {
    SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: "part:0".to_string(),
        image_id: "img-1".to_string(),
        source_volume_size: 1024,
        built_at: "2026-05-29T00:00:00Z".to_string(),
    }
}

#[test]
fn file_name_is_sanitized() {
    assert_eq!(snapshot_file_name("img-1", "part:2"), "img-1__part_2.dirz");
    assert_eq!(
        snapshot_file_name("a:b/c", "lvm:0:1"),
        "a_b_c__lvm_0_1.dirz"
    );
    assert_eq!(snapshot_file_name("", ""), "image__root.dirz");
    // Two images sharing a volume key never collide on a file name.
    assert_ne!(
        snapshot_file_name("img-a", "part:1"),
        snapshot_file_name("img-b", "part:1")
    );
}

#[test]
fn legacy_lv_names_are_normalized_on_load() {
    let entries = vec![
        SnapshotEntry {
            parent: "part:2".to_string(),
            node: node("lvm:2:0", "LV ubuntu-lv · Ext4", NodeKind::Dir),
            path: "/LV ubuntu-lv · Ext4".to_string(),
        },
        // 新格式与非 LVM 节点原样保留——包括恰好叫 "LV ..." 的普通目录。
        SnapshotEntry {
            parent: "part:2".to_string(),
            node: node("lvm:2:1", "swap_1", NodeKind::Unallocated),
            path: "/swap_1".to_string(),
        },
        SnapshotEntry {
            parent: "lvm:2:0".to_string(),
            node: node("ext4:lvm:2:0:11", "LV docs · old", NodeKind::Dir),
            path: "/LV ubuntu-lv · Ext4/LV docs · old".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);

    let lvs = snapshot.children("part:2").expect("LV listing covered");
    assert_eq!(lvs[0].name, "ubuntu-lv");
    assert_eq!(lvs[1].name, "swap_1");
    let inner = snapshot.children("lvm:2:0").expect("LV subtree covered");
    assert_eq!(inner[0].name, "LV docs · old");
}

#[test]
fn children_index_keys_by_query_parent() {
    let entries = vec![
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:2", "dir", NodeKind::Dir),
            path: "/dir".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:11", "file.txt", NodeKind::File),
            path: "/dir/file.txt".to_string(),
        },
        // A directory that WAS walked (it is a non-deleted dir entry under
        // /dir) but produced no children of its own → genuinely empty.
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:12", "empty", NodeKind::Dir),
            path: "/dir/empty".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);

    let root = snapshot.children("part:0").expect("root covered");
    assert_eq!(root.len(), 1);
    assert_eq!(root[0].id, "ext4:part:0:2");

    let sub = snapshot.children("ext4:part:0:2").expect("subdir covered");
    assert_eq!(sub.len(), 2);
    assert_eq!(sub[0].name, "file.txt");

    // child_count mirrors children().len() without cloning for populated
    // dirs, infers Some(0) for a walked-but-empty dir, and reports None for
    // a parent the snapshot never recorded.
    assert_eq!(snapshot.child_count("part:0"), Some(1));
    assert_eq!(snapshot.child_count("ext4:part:0:2"), Some(2));
    assert_eq!(snapshot.child_count("ext4:part:0:12"), Some(0));
    assert_eq!(snapshot.child_count("ext4:part:0:999"), None);

    // An un-walked / empty directory is not covered → caller goes live.
    assert!(snapshot.children("ext4:part:0:999").is_none());
    assert!(!snapshot.covers("ext4:part:0:999"));
}

#[test]
fn subtree_has_file_distinguishes_empty_subtrees() {
    let mut deleted_file = node("ext4:part:0:90", "ghost.txt", NodeKind::File);
    deleted_file.deleted = true;
    let entries = vec![
        // vendor/ — only empty subdirs (bin, etc) plus a deleted file, so it
        // has no *visible* content anywhere beneath it → recursively empty.
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:2", "vendor", NodeKind::Dir),
            path: "/vendor".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:3", "bin", NodeKind::Dir),
            path: "/vendor/bin".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:4", "etc", NodeKind::Dir),
            path: "/vendor/etc".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: deleted_file,
            path: "/vendor/ghost.txt".to_string(),
        },
        // data/ — holds a real file, so it (and the volume root) is non-empty.
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:5", "data", NodeKind::Dir),
            path: "/data".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:5".to_string(),
            node: node("ext4:part:0:6", "app.db", NodeKind::File),
            path: "/data/app.db".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);

    // vendor and a directly-empty subdir: no real content beneath → empty.
    assert_eq!(snapshot.subtree_has_file("ext4:part:0:2"), Some(false));
    assert_eq!(snapshot.subtree_has_file("ext4:part:0:3"), Some(false));
    // data holds a file directly; the volume root has one further down.
    assert_eq!(snapshot.subtree_has_file("ext4:part:0:5"), Some(true));
    assert_eq!(snapshot.subtree_has_file("part:0"), Some(true));
    // A directory the snapshot never recorded → unknown, not "empty".
    assert_eq!(snapshot.subtree_has_file("ext4:part:0:999"), None);
}

#[test]
fn flat_descendants_preorder_paths_and_deleted_leaf() {
    let mut deleted_dir = node("ntfs:part:0:77", "recycler", NodeKind::Dir);
    deleted_dir.deleted = true;
    let entries = vec![
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:2", "dir", NodeKind::Dir),
            path: "/dir".to_string(),
        },
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: deleted_dir,
            path: "/recycler".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:11", "file.txt", NodeKind::File),
            path: "/dir/file.txt".to_string(),
        },
        SnapshotEntry {
            parent: "ext4:part:0:2".to_string(),
            node: node("ext4:part:0:12", "empty", NodeKind::Dir),
            path: "/dir/empty".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);

    // From the volume root: preorder, each child expanded before its
    // sibling, paths rebuilt relative to the queried parent.
    let (flat, truncated) = snapshot
        .flat_descendants("part:0", 100)
        .expect("root covered");
    assert!(!truncated);
    let paths: Vec<&str> = flat.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["/dir", "/dir/file.txt", "/dir/empty", "/recycler"]
    );

    // From a subdirectory: paths are relative to it, not the volume root.
    let (flat, _) = snapshot
        .flat_descendants("ext4:part:0:2", 100)
        .expect("subdir covered");
    let paths: Vec<&str> = flat.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["/file.txt", "/empty"]);

    // The deleted dir is listed as a leaf — its subtree is the live
    // splice's job — and an uncaptured parent yields None.
    assert!(snapshot.flat_descendants("ntfs:part:0:77", 100).is_none());
    assert!(snapshot.flat_descendants("ext4:part:0:999", 100).is_none());
}

#[test]
fn flat_descendants_caps_and_flags_truncation() {
    let entries = vec![
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:2", "a", NodeKind::File),
            path: "/a".to_string(),
        },
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:3", "b", NodeKind::File),
            path: "/b".to_string(),
        },
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: node("ext4:part:0:4", "c", NodeKind::File),
            path: "/c".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);
    let (flat, truncated) = snapshot
        .flat_descendants("part:0", 2)
        .expect("root covered");
    assert_eq!(flat.len(), 2);
    assert!(truncated);
}

#[test]
fn round_trips_through_dirz_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("part_0.dirz");

    // Exercise the awkward-to-encode shapes: Some timestamps, a Some
    // link_target, and the lowercase-renamed Symlink enum variant — all
    // positional in the bincode body.
    let mut etc = node("ext4:part:0:2", "etc", NodeKind::Dir);
    etc.timestamps = Mactb {
        modified: Some("2026-05-29T01:02:03Z".to_string()),
        accessed: Some("2026-05-29T01:02:04Z".to_string()),
        created: None,
        mft_changed: None,
        birth: Some("2026-05-29T00:00:00Z".to_string()),
    };
    let mut link = node("ext4:part:0:3", "self", NodeKind::Symlink);
    link.link_target = Some("/etc".to_string());
    let entries = vec![
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: etc,
            path: "/etc".to_string(),
        },
        SnapshotEntry {
            parent: "part:0".to_string(),
            node: link,
            path: "/self".to_string(),
        },
    ];
    let snapshot = VolumeSnapshot::new(header(), entries);

    // Write through the v2 helpers (the exact framing the builders emit).
    let file = File::create(&path).expect("create");
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3).expect("enc");
    write_snapshot_header(&mut encoder, &snapshot.header).expect("header");
    let mut scratch = Vec::new();
    for entry in &snapshot.entries {
        write_snapshot_entry(&mut encoder, entry, &mut scratch).expect("entry");
    }
    encoder.finish().expect("finish");

    let loaded_header = load_snapshot_header(&path).expect("header");
    assert_eq!(loaded_header.volume_key, "part:0");
    assert_eq!(loaded_header.source_volume_size, 1024);

    let loaded = load_snapshot(&path).expect("load");
    assert_eq!(loaded.entries.len(), 2);
    let root = loaded.children("part:0").expect("covered");
    assert_eq!(root[0].name, "etc");
    // Rich fields survive the positional round-trip.
    assert_eq!(
        root[0].timestamps.modified.as_deref(),
        Some("2026-05-29T01:02:03Z")
    );
    assert_eq!(root[0].timestamps.created, None);
    let self_link = root.iter().find(|n| n.name == "self").expect("symlink");
    assert_eq!(self_link.kind, NodeKind::Symlink);
    assert_eq!(self_link.link_target.as_deref(), Some("/etc"));
}

/// Helper: write a valid v2 `.dirz` (header + N entry frames) and return the
/// raw, decompressed body bytes (header line + frames) so a test can corrupt
/// the tail before re-compressing.
fn write_v2_dirz(path: &Path, entries: &[SnapshotEntry]) {
    let file = File::create(path).expect("create");
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3).expect("enc");
    write_snapshot_header(&mut encoder, &header()).expect("header");
    let mut scratch = Vec::new();
    for entry in entries {
        write_snapshot_entry(&mut encoder, entry, &mut scratch).expect("entry");
    }
    encoder.finish().expect("finish");
}

#[test]
fn v2_clean_frame_boundary_loads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clean.dirz");
    let entries = vec![SnapshotEntry {
        parent: "part:0".to_string(),
        node: node("ext4:part:0:2", "etc", NodeKind::Dir),
        path: "/etc".to_string(),
    }];
    write_v2_dirz(&path, &entries);
    // Ends exactly at a frame boundary → clean EOF, no error.
    let loaded = load_snapshot(&path).expect("load");
    assert_eq!(loaded.entries.len(), 1);
}

#[test]
fn v2_truncated_tail_is_rejected() {
    // Build a valid v2 body, then lop bytes off the END of the
    // (decompressed) frame stream and re-compress: loading must error, not
    // silently return a short snapshot.
    let entry = SnapshotEntry {
        parent: "part:0".to_string(),
        node: node("ext4:part:0:2", "etc", NodeKind::Dir),
        path: "/etc".to_string(),
    };
    // Reconstruct the decompressed bytes the loader would see: header line +
    // one full frame, then drop the last byte of the frame body.
    let mut raw: Vec<u8> = Vec::new();
    write_snapshot_header(&mut raw, &header()).expect("header");
    let mut scratch = Vec::new();
    write_snapshot_entry(&mut raw, &entry, &mut scratch).expect("frame");
    raw.truncate(raw.len() - 1); // tear the frame body

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("torn.dirz");
    let file = File::create(&path).expect("create");
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3).expect("enc");
    encoder.write_all(&raw).expect("write raw");
    encoder.finish().expect("finish");

    let err = load_snapshot(&path).expect_err("truncated body must error");
    assert!(
        err.to_string().contains("truncated"),
        "unexpected error: {err}"
    );
}

#[test]
fn legacy_schema1_body_is_rejected() {
    // An old schema-1 file has a JSON header claiming version 1; the loader
    // must refuse it (we no longer parse JSON-lines bodies) rather than try
    // to bincode-decode JSON.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v1.dirz");
    let mut h = header();
    h.schema_version = 1;
    let file = File::create(&path).expect("create");
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3).expect("enc");
    write_snapshot_header(&mut encoder, &h).expect("header");
    // (no body needed — the version gate fires before any frame is read)
    encoder.finish().expect("finish");

    // The header still reads (JSON line 0, unchanged), but a full load bails.
    assert_eq!(
        load_snapshot_header(&path).expect("header").schema_version,
        1
    );
    let err = load_snapshot(&path).expect_err("schema 1 must be rejected");
    assert!(
        err.to_string().contains("no longer supported"),
        "unexpected error: {err}"
    );
}

fn edge(
    record: u64,
    parent: u64,
    name: &str,
    kind: NodeKind,
    deleted: bool,
    parent_id: &str,
) -> NtfsTreeEdge {
    let mut node = node(&format!("ntfs:part:0:{record}"), name, kind);
    node.parent_id = Some(parent_id.to_string());
    node.deleted = deleted;
    node.inode = Some(record);
    NtfsTreeEdge {
        record_number: record,
        parent_record_number: parent,
        node,
    }
}

#[test]
fn tree_snapshot_rebuilds_from_edges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("part_0.dirz");
    // A small tree: root (record 5) holds dir "etc" + two files; "etc"
    // holds a file, a deleted file, and one name of a hard-linked record
    // that is also named under root. One orphan points at a parent that is
    // unreachable from root and must not surface.
    let edges = vec![
        edge(64, 5, "etc", NodeKind::Dir, false, "part:0"),
        edge(65, 5, "aaa.txt", NodeKind::File, false, "part:0"),
        edge(100, 64, "hosts", NodeKind::File, false, "ntfs:part:0:64"),
        edge(101, 64, "old.txt", NodeKind::File, true, "ntfs:part:0:64"),
        edge(200, 5, "link_a", NodeKind::File, false, "part:0"),
        edge(200, 64, "link_b", NodeKind::File, false, "ntfs:part:0:64"),
        edge(300, 999, "orphan", NodeKind::File, false, "ntfs:part:0:999"),
    ];
    let cancel = CancellationToken::new();
    let built = write_tree_snapshot(
        edges,
        Vec::new(),
        header(),
        &path,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        5,
        false,
        &cancel,
        &mut |_| {},
    )
    .expect("write");
    assert!(!built.truncated);
    // Six reachable edges; the orphan under parent 999 is dropped.
    assert_eq!(built.entry_count, 6);

    let snap = load_snapshot(&path).expect("load");

    // Root listing: dirs first, then files by case-insensitive name.
    let root: Vec<String> = snap
        .children("part:0")
        .expect("root")
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(root, vec!["etc", "aaa.txt", "link_a"]);

    // Subdirectory listing, same ordering; the deleted file is a leaf.
    let etc = snap.children("ntfs:part:0:64").expect("etc");
    let names: Vec<&str> = etc.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, vec!["hosts", "link_b", "old.txt"]);
    assert!(etc.iter().find(|n| n.name == "old.txt").unwrap().deleted);

    // The orphan never becomes browsable.
    assert!(!snap.covers("ntfs:part:0:999"));

    // Paths are reconstructed through the tree; the hard-linked record
    // surfaces under both of its parents with the right per-edge name.
    let hosts = snap
        .entries
        .iter()
        .find(|e| e.node.name == "hosts")
        .unwrap();
    assert_eq!(hosts.path, "/etc/hosts");
    let link_a = snap.entries.iter().find(|e| e.path == "/link_a").unwrap();
    assert_eq!(link_a.node.id, "ntfs:part:0:200");
    let link_b = snap
        .entries
        .iter()
        .find(|e| e.path == "/etc/link_b")
        .unwrap();
    assert_eq!(link_b.node.id, "ntfs:part:0:200");
}

#[test]
fn tree_snapshot_bakes_ads_pseudo_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("part_0.dirz");
    // root (5) → "hosts" file (record 64) carrying one named ADS.
    let edges = vec![edge(64, 5, "hosts", NodeKind::File, false, "part:0")];
    let ads = vec![NtfsAdsStream {
        record_number: 64,
        name: "secret".to_string(),
        size: 1234,
    }];
    let cancel = CancellationToken::new();
    let built = write_tree_snapshot(
        edges,
        ads,
        header(),
        &path,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        5,
        false,
        &cancel,
        &mut |_| {},
    )
    .expect("write");
    // The owner plus its one pseudo-row.
    assert_eq!(built.entry_count, 2);

    let snap = load_snapshot(&path).expect("load");
    let root = snap.children("part:0").expect("root");

    let owner = root.iter().find(|n| n.name == "hosts").expect("owner");
    assert!(owner.has_snapshot, "owner should be flagged");
    assert_eq!(owner.id, "ntfs:part:0:64");

    let ads_row = root
        .iter()
        .find(|n| n.id.starts_with("ntfs-ads:"))
        .expect("ads pseudo-row");
    assert_eq!(
        ads_row.id,
        format!("ntfs-ads:part:0:64:{}", hex::encode("secret"))
    );
    assert_eq!(ads_row.parent_id.as_deref(), Some("ntfs:part:0:64"));
    assert_eq!(ads_row.kind, NodeKind::File);
    assert_eq!(ads_row.size, 1234);
    assert!(!ads_row.has_snapshot);
    assert!(ads_row.name.starts_with("hosts:secret"));

    // The pseudo-row's id round-trips through the ADS-id parser.
    let (_source, record, stream) =
        crate::filesystem::volumes::ntfs::parse_ntfs_ads_id(&ads_row.id).expect("parse ads id");
    assert_eq!(record, 64);
    assert_eq!(stream, "secret");
}

#[test]
fn tree_snapshot_bakes_root_directory_ads() {
    // The volume root (record 5) carries a named ADS but has no edge of its
    // own — the classic `\:secret.txt` hiding spot. It must surface as a
    // top-level pseudo-row under the volume root key. (Regression: directory
    // ADS, especially the root's, were previously dropped by the scan + the
    // File-only gate in the builder.)
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("part_0.dirz");
    let edges = vec![edge(64, 5, "readme.txt", NodeKind::File, false, "part:0")];
    let ads = vec![NtfsAdsStream {
        record_number: 5,
        name: "多个密码记录.txt".to_string(),
        size: 23,
    }];
    let cancel = CancellationToken::new();
    let built = write_tree_snapshot(
        edges,
        ads,
        header(),
        &path,
        DEFAULT_MAX_SNAPSHOT_ENTRIES,
        5,
        false,
        &cancel,
        &mut |_| {},
    )
    .expect("write");
    // The "readme.txt" file plus root's one stream pseudo-row.
    assert_eq!(built.entry_count, 2);

    let snap = load_snapshot(&path).expect("load");
    let root = snap.children("part:0").expect("root");

    let ads_row = root
        .iter()
        .find(|n| n.id.starts_with("ntfs-ads:"))
        .expect("root ads pseudo-row");
    assert_eq!(
        ads_row.id,
        format!("ntfs-ads:part:0:5:{}", hex::encode("多个密码记录.txt"))
    );
    // Filed directly under the volume root key, so it lists at top level.
    assert_eq!(ads_row.parent_id.as_deref(), Some("part:0"));
    assert_eq!(ads_row.kind, NodeKind::File);
    assert_eq!(ads_row.size, 23);
    assert_eq!(ads_row.inode, Some(5));
    assert!(ads_row.name.starts_with(":多个密码记录.txt"));

    // Round-trips through the ADS-id parser back to root record 5.
    let (_source, record, stream) =
        crate::filesystem::volumes::ntfs::parse_ntfs_ads_id(&ads_row.id).expect("parse ads id");
    assert_eq!(record, 5);
    assert_eq!(stream, "多个密码记录.txt");
}
