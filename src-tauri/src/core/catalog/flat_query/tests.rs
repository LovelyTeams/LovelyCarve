use super::*;
use crate::core::filesystem::tree::fs_tree::{FsNode, Mactb};

fn node(id: &str, name: &str, kind: NodeKind) -> FsNode {
    FsNode {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: None,
        kind,
        size: 100,
        deleted: false,
        inode: Some(42),
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }
}

fn entry(id: &str, name: &str, kind: NodeKind, path: &str) -> FsWalkEntry {
    FsWalkEntry {
        node: node(id, name, kind),
        path: path.to_string(),
    }
}

#[test]
fn ext_of_matches_frontend_semantics() {
    assert_eq!(flat_ext_of(&node("a", "Photo.JPG", NodeKind::File)), "jpg");
    assert_eq!(
        flat_ext_of(&node("a", "archive.tar.gz", NodeKind::File)),
        "gz"
    );
    assert_eq!(flat_ext_of(&node("a", ".gitignore", NodeKind::File)), "");
    assert_eq!(flat_ext_of(&node("a", "trailing.", NodeKind::File)), "");
    assert_eq!(flat_ext_of(&node("a", "noext", NodeKind::File)), "");
    assert_eq!(flat_ext_of(&node("a", "dir", NodeKind::Dir)), "");
    assert_eq!(
        flat_ext_of(&node("a", ".config.yaml", NodeKind::File)),
        "yaml"
    );
}

#[test]
fn name_query_glob_matches_frontend_matcher() {
    assert!(name_query_matches("report", "my report.pdf"));
    assert!(!name_query_matches("report", "summary.txt"));
    assert!(name_query_matches("*.pdf", "a.pdf"));
    assert!(name_query_matches("file-?.txt", "file-1.txt"));
    assert!(!name_query_matches("file-?.txt", "file-10.txt"));
    assert!(name_query_matches("*log*", "system.log.1"));
    assert!(name_query_matches("", "anything"));
}

#[test]
fn name_query_like_pattern_round_trips_glob() {
    assert_eq!(name_query_like_pattern("a%b"), "a\\%b");
    assert_eq!(name_query_like_pattern("*.pdf"), "%.pdf");
    assert_eq!(name_query_like_pattern("file-?.txt"), "file-_.txt");
}

#[test]
fn filter_covers_keyword_name_path_and_kinds() {
    let mut filter = FlatFileFilter {
        keyword: Some("evi".into()),
        ..FlatFileFilter::default()
    };
    let hit = entry("n1", "Evidence.docx", NodeKind::File, "/case/Evidence.docx");
    let miss_name = entry("n2", "readme.txt", NodeKind::File, "/case/evi-note.md");
    assert!(flat_filter_keeps(&hit, &filter));
    // 路径命中也算 keyword 命中。
    assert!(flat_filter_keeps(&miss_name, &filter));
    assert!(!flat_filter_keeps(
        &entry("n3", "other", NodeKind::File, "/x/y"),
        &filter
    ));

    filter = FlatFileFilter {
        name_query: Some("*.docx".into()),
        ..FlatFileFilter::default()
    };
    assert!(flat_filter_keeps(&hit, &filter));
    assert!(!flat_filter_keeps(
        &entry("n4", "a.pdf", NodeKind::File, "/a.pdf"),
        &filter
    ));

    // 锚定路径:匹配「当前目录相对路径」的前缀;目录行本身从不出现。
    filter = FlatFileFilter {
        path_query: Some("/win/sys".into()),
        ..FlatFileFilter::default()
    };
    assert!(flat_filter_keeps(
        &entry("n5", "a.dll", NodeKind::File, "/win/system32/a.dll"),
        &filter
    ));
    assert!(!flat_filter_keeps(
        &entry("n6", "a.dll", NodeKind::File, "/data/win/system32/a.dll"),
        &filter
    ));

    // 类型漏斗按中文标签匹配(与前端 kindLabel 一致)。
    filter = FlatFileFilter {
        kind_query: Some("符号".into()),
        ..FlatFileFilter::default()
    };
    assert!(flat_filter_keeps(
        &entry("n7", "link", NodeKind::Symlink, "/link"),
        &filter
    ));
    assert!(!flat_filter_keeps(&hit, &filter));

    // 目录行永远不进全显结果。
    filter = FlatFileFilter::default();
    assert!(!flat_filter_keeps(
        &entry("n8", "dir", NodeKind::Dir, "/dir"),
        &filter
    ));
}

#[test]
fn kind_filters_accept_both_languages() {
    let items = [
        entry("file", "文件列表", NodeKind::File, "/文件列表"),
        entry("link", "target", NodeKind::Symlink, "/target"),
        entry("unalloc:1", "gap", NodeKind::File, "/gap"),
    ];
    for (query, expected) in [
        ("文件", vec!["file"]),
        ("file", vec!["file"]),
        ("symbolic", vec!["link"]),
        ("符号", vec!["link"]),
        ("UNALLOCATED", vec!["unalloc:1"]),
        ("未分配", vec!["unalloc:1"]),
    ] {
        let filter = FlatFileFilter {
            kind_query: Some(query.into()),
            ..FlatFileFilter::default()
        }
        .normalized();
        let actual: Vec<&str> = items
            .iter()
            .filter(|item| flat_filter_keeps(item, &filter))
            .map(|item| item.node.id.as_str())
            .collect();
        assert_eq!(actual, expected, "{query}");
    }
    assert_eq!(items[0].node.name, "文件列表");
}

#[test]
fn filter_size_mod_and_deleted() {
    let mut base = node("s", "a.bin", NodeKind::File);
    base.size = 1025;
    let entry = FsWalkEntry {
        node: base,
        path: "/a.bin".into(),
    };
    let filter = FlatFileFilter {
        size_ge: Some(1024),
        size_le: Some(2048),
        size_mod: Some(FlatSizeMod {
            divisor: 512,
            remainder: 1,
        }),
        ..FlatFileFilter::default()
    };
    assert!(flat_filter_keeps(&entry, &filter));

    let filter = FlatFileFilter {
        deleted: Some(true),
        ..FlatFileFilter::default()
    };
    assert!(!flat_filter_keeps(&entry, &filter));

    // divisor = 0 归一化后剔除,不过滤。
    let filter = FlatFileFilter {
        size_mod: Some(FlatSizeMod {
            divisor: 0,
            remainder: 3,
        }),
        ..FlatFileFilter::default()
    }
    .normalized();
    assert!(flat_filter_keeps(&entry, &filter));
}

#[test]
fn filter_time_ranges_use_display_semantics() {
    let mut base = node("t", "t.txt", NodeKind::File);
    base.timestamps.modified = Some("2026-03-02T10:20:30.500Z".into());
    let entry = FsWalkEntry {
        node: base,
        path: "/t.txt".into(),
    };
    let in_range = FlatTimeRange {
        from: Some("2026-03-02 10:20:30".into()),
        to: Some("2026-03-02 10:20:30".into()),
    };
    // 同秒带毫秒:显示串截断到秒,两端都含 → 命中。
    assert!(time_range_keeps(
        entry.node.timestamps.modified.as_deref(),
        Some(&in_range)
    ));
    let before = FlatTimeRange {
        from: Some("2026-03-02 10:20:31".into()),
        to: None,
    };
    assert!(!time_range_keeps(
        entry.node.timestamps.modified.as_deref(),
        Some(&before)
    ));
    // 缺失时间戳在区间激活时排除。
    assert!(!time_range_keeps(None, Some(&before)));
    // 无法解析的显示串 → None。
    assert_eq!(display_timestamp(Some("not-a-date")), None);
    assert_eq!(
        display_timestamp(Some("2026-03-02T10:20:30Z")),
        Some("2026-03-02 10:20:30".to_string())
    );
}

#[test]
fn time_range_sql_bounds_convert_to_exclusive_upper() {
    let range = FlatTimeRange {
        from: Some("2026-03-02 10:20:30".into()),
        to: Some("2026-03-02 23:59:59".into()),
    };
    let (from, to) = time_range_sql_bounds(&range);
    assert_eq!(from.as_deref(), Some("2026-03-02T10:20:30"));
    assert_eq!(to.as_deref(), Some("2026-03-03T00:00:00"));
    // 毫秒列值 …:59.9Z 落在排他上界之内。
    assert!("2026-03-02T23:59:59.900Z" < to.unwrap().as_str());
    assert!("2026-03-02T10:20:30.500Z" >= from.unwrap().as_str());
}

#[test]
fn default_sort_is_kind_then_natural_name() {
    let sort = FlatSort::default();
    let file10 = entry("a", "file-10", NodeKind::File, "/file-10");
    let file2 = entry("b", "file-2", NodeKind::File, "/file-2");
    let dir = entry("c", "a-dir", NodeKind::Dir, "/a-dir");
    let symlink = entry("d", "zzz", NodeKind::Symlink, "/zzz");
    // 目录 < 文件 < 符号链接(字典序 kind);file-2 自然序先于 file-10。
    assert_eq!(sort.compare(&dir, &file2), Ordering::Less);
    assert_eq!(sort.compare(&file2, &symlink), Ordering::Less);
    assert_eq!(sort.compare(&file2, &file10), Ordering::Less);
    // nid 兜底:同键同值时按 id 升序。
    let x = entry("x", "same", NodeKind::File, "/same");
    let y = entry("y", "same", NodeKind::File, "/same");
    assert_eq!(sort.compare(&x, &y), Ordering::Less);
}

#[test]
fn assembler_merges_windows_and_slices_pages() {
    let sort = FlatSort::default();
    let mut assembler = FlatPageAssembler::new(sort, 1, 2);
    let source_a = vec![
        entry("a1", "1.txt", NodeKind::File, "/1.txt"),
        entry("a2", "3.txt", NodeKind::File, "/3.txt"),
        entry("a3", "5.txt", NodeKind::File, "/5.txt"),
    ];
    let source_b = vec![
        entry("b1", "2.txt", NodeKind::File, "/2.txt"),
        entry("b2", "4.txt", NodeKind::File, "/4.txt"),
    ];
    assembler.add_sorted_source(source_a, 30, false);
    assembler.add_sorted_source(source_b, 22, true);
    let page = assembler.finish();
    let names: Vec<&str> = page.items.iter().map(|e| e.node.name.as_str()).collect();
    assert_eq!(names, vec!["2.txt", "3.txt"]);
    assert_eq!(page.total, 52);
    assert!(page.truncated);
}

#[test]
fn assembler_clamps_skip_beyond_loaded_rows() {
    let mut assembler = FlatPageAssembler::new(FlatSort::default(), 10, 5);
    assembler.add_sorted_source(
        vec![entry("a", "1.txt", NodeKind::File, "/1.txt")],
        1,
        false,
    );
    let page = assembler.finish();
    assert!(page.items.is_empty());
    assert_eq!(page.total, 1);
}

#[test]
fn filter_category_extensions_cover_other_bucket() {
    let make = |name: &str| {
        let mut base = node("c", name, NodeKind::File);
        base.size = 0;
        FsWalkEntry {
            node: base,
            path: format!("/{name}"),
        }
    };
    let filter = FlatFileFilter {
        extensions: vec!["png".into()],
        other_category_exts: vec![
            "png".into(),
            "jpg".into(),
            "mp4".into(),
            "exe".into(),
            "txt".into(),
        ],
        ..FlatFileFilter::default()
    };
    assert!(flat_filter_keeps(&make("a.png"), &filter));
    assert!(flat_filter_keeps(&make("unknown.zzz"), &filter));
    assert!(flat_filter_keeps(&make("noext"), &filter));
    assert!(!flat_filter_keeps(&make("b.jpg"), &filter));
    assert!(!flat_filter_keeps(&make("c.exe"), &filter));
}

#[test]
fn normalized_lowercases_and_drops_empties() {
    let filter = FlatFileFilter {
        keyword: Some("  KEY.Word ".into()),
        name_query: Some("".into()),
        ..FlatFileFilter::default()
    }
    .normalized();
    assert_eq!(filter.keyword.as_deref(), Some("key.word"));
    assert_eq!(filter.name_query, None);
}
