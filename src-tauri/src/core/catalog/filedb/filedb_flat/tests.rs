use super::*;
use crate::core::catalog::filedb::filedb_store;
use crate::core::catalog::flat_query::{flat_filter_keeps, FlatSizeMod};
use crate::core::catalog::snapshot::volume_snapshot::SnapshotEntry;
use crate::core::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};

struct Seed {
    parent: &'static str,
    id: &'static str,
    name: &'static str,
    path: &'static str,
    kind: NodeKind,
    size: u64,
    deleted: bool,
    inode: Option<u64>,
    modified: Option<&'static str>,
    created: Option<&'static str>,
    birth: Option<&'static str>,
    link_target: Option<&'static str>,
}

const fn file(
    parent: &'static str,
    id: &'static str,
    name: &'static str,
    path: &'static str,
    size: u64,
    modified: Option<&'static str>,
) -> Seed {
    Seed {
        parent,
        id,
        name,
        path,
        kind: NodeKind::File,
        size,
        deleted: false,
        inode: Some(7),
        modified,
        created: None,
        birth: None,
        link_target: None,
    }
}

fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            kind: NodeKind::Dir,
            ..file("part:0", "d-docs", "Docs", "/Docs", 0, None)
        },
        Seed {
            kind: NodeKind::Dir,
            deleted: true,
            ..file("part:0", "d-gone", "gone", "/gone", 0, None)
        },
        // 子项已在索引里的已删目录:不该进拼接名单、不该触发窗口模式。
        Seed {
            kind: NodeKind::Dir,
            deleted: true,
            ..file("part:0", "d-dead", "dead", "/dead", 0, None)
        },
        file("d-dead", "f9", "x.txt", "/dead/x.txt", 0, None),
        Seed {
            created: Some("2026-02-01T00:00:00Z"),
            ..file(
                "part:0",
                "f1",
                "Report.PDF",
                "/Report.PDF",
                2048,
                Some("2026-03-02T10:20:30.500Z"),
            )
        },
        Seed {
            inode: None,
            birth: Some("2025-12-31T23:00:00Z"),
            ..file(
                "part:0",
                "f2",
                "notes.txt",
                "/notes.txt",
                10,
                Some("2026-01-01T00:00:00Z"),
            )
        },
        file("part:0", "f3", "file-10.log", "/file-10.log", 512, None),
        Seed {
            inode: Some(99),
            ..file("part:0", "f4", "file-2.log", "/file-2.log", 1024, None)
        },
        Seed {
            kind: NodeKind::Symlink,
            link_target: Some("/etc/Target"),
            ..file("part:0", "s1", "link", "/link", 0, None)
        },
        file(
            "d-docs",
            "f5",
            "Evidence.docx",
            "/Docs/Evidence.docx",
            4096,
            Some("2026-03-02T23:59:59.900Z"),
        ),
        file("d-docs", "f6", "a.PNG", "/Docs/a.PNG", 300, None),
        file("d-docs", "f7", "README", "/Docs/README", 5, None),
        Seed {
            deleted: true,
            ..file("d-docs", "f8", "x.tmp", "/Docs/x.tmp", 7, None)
        },
    ]
}

fn snapshot_entries() -> Vec<SnapshotEntry> {
    seeds()
        .into_iter()
        .map(|seed| SnapshotEntry {
            parent: seed.parent.to_string(),
            node: FsNode {
                id: seed.id.to_string(),
                name: seed.name.to_string(),
                parent_id: Some(seed.parent.to_string()),
                kind: seed.kind,
                size: seed.size,
                deleted: seed.deleted,
                inode: seed.inode,
                timestamps: Mactb {
                    modified: seed.modified.map(str::to_string),
                    accessed: None,
                    created: seed.created.map(str::to_string),
                    mft_changed: None,
                    birth: seed.birth.map(str::to_string),
                },
                link_target: seed.link_target.map(str::to_string),
                has_snapshot: false,
            },
            path: seed.path.to_string(),
        })
        .collect()
}

/// The seeded `part:0` index of a temporary directory.
struct Seeded {
    _dir: tempfile::TempDir,
    index: std::path::PathBuf,
}

impl Seeded {
    fn path(&self) -> &Path {
        &self.index
    }
}

fn seeded() -> Seeded {
    let dir = tempfile::tempdir().expect("tempdir");
    let index =
        filedb_store::test_support::build_test_index(dir.path(), "part:0", &snapshot_entries());
    Seeded { _dir: dir, index }
}

fn reader(seeded: &Seeded) -> Connection {
    filedb_file::open_reader(seeded.path()).expect("open index")
}

fn run(
    index: &Path,
    parent_id: &str,
    filter: &FlatFileFilter,
    sort: FlatSort,
    skip: usize,
    limit: usize,
) -> Option<FlatIndexPage> {
    flat_page(FlatPageQuery {
        index,
        volume_key: "part:0",
        parent_id,
        filter,
        sort,
        skip,
        limit,
        force_window: false,
        window_floor: 0,
    })
    .expect("flat page")
}

fn ids(page: &FlatIndexPage) -> Vec<&str> {
    page.items.iter().map(|e| e.node.id.as_str()).collect()
}

#[test]
fn subtree_scope_pages_directly_and_strips_prefix() {
    let dir = seeded();
    let page = run(
        dir.path(),
        "d-docs",
        &FlatFileFilter::default(),
        FlatSort::default(),
        1,
        2,
    )
    .expect("covered");
    // Docs 下按 natsort:Evidence.docx, README, a.PNG, x.tmp → 跳 1 取 2。
    assert_eq!(ids(&page), vec!["f7", "f6"]);
    assert_eq!(page.total, 4);
    assert!(!page.windowed);
    assert!(page.deleted_dirs.is_empty());
    let paths: Vec<&str> = page.items.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["/README", "/a.PNG"]);
}

#[test]
fn volume_scope_with_deleted_dirs_returns_window_and_reports_them() {
    let dir = seeded();
    let page = run(
        dir.path(),
        "part:0",
        &FlatFileFilter::default(),
        FlatSort::default(),
        2,
        3,
    )
    .expect("covered");
    assert!(page.windowed);
    assert_eq!(page.total, 10);
    // 窗口 = 前 skip+limit = 5 行:kind file 在 symlink 前,名字 natsort
    // (x.txt 与 x.tmp 都在 f4/f3/f2 之后,d-dead 的子项不算拼接目标)。
    assert_eq!(ids(&page), vec!["f5", "f7", "f1", "f6", "f4"]);
    // d-dead 的子项已在索引里 → 不进拼接名单;d-gone 没有 → 要拼。
    assert_eq!(
        page.deleted_dirs,
        vec![("d-gone".to_string(), "/gone".to_string())]
    );
}

#[test]
fn window_floor_bounds_the_window_from_below() {
    let dir = seeded();
    // 根范围因 d-gone 走窗口模式;floor=2 时窗口从全局第 2 行取到 skip+limit。
    let page = flat_page(FlatPageQuery {
        index: dir.path(),
        volume_key: "part:0",
        parent_id: "part:0",
        filter: &FlatFileFilter::default(),
        sort: FlatSort::default(),
        skip: 4,
        limit: 2,
        force_window: false,
        window_floor: 2,
    })
    .expect("flat page")
    .expect("covered");
    assert!(page.windowed);
    // 全序:f5 f7 f1 f6 f4 f3 f2 f8 f9 s1 → 窗口 = [2, 6) = f1 f6 f4 f3。
    assert_eq!(ids(&page), vec!["f1", "f6", "f4", "f3"]);
    assert_eq!(page.total, 10);
}

#[test]
fn short_pages_report_the_total_from_their_rows() {
    let dir = seeded();
    // Docs 下 natsort 全序 f5 f7 f6 f8:跳 3 取 10 只剩最后一行。
    let tail = run(
        dir.path(),
        "d-docs",
        &FlatFileFilter::default(),
        FlatSort::default(),
        3,
        10,
    )
    .expect("covered");
    assert_eq!(ids(&tail), vec!["f8"]);
    assert_eq!(tail.total, 4);
    // 根范围因 d-gone 走窗口模式:窗口 [8, 13) 只取到末尾两行。
    let window = flat_page(FlatPageQuery {
        index: dir.path(),
        volume_key: "part:0",
        parent_id: "part:0",
        filter: &FlatFileFilter::default(),
        sort: FlatSort::default(),
        skip: 8,
        limit: 5,
        force_window: false,
        window_floor: 8,
    })
    .expect("flat page")
    .expect("covered");
    assert!(window.windowed);
    assert_eq!(ids(&window), vec!["f9", "s1"]);
    assert_eq!(window.total, 10);
}

#[test]
fn skip_beyond_total_still_reports_total() {
    let dir = seeded();
    let page = run(
        dir.path(),
        "d-docs",
        &FlatFileFilter::default(),
        FlatSort::default(),
        100,
        10,
    )
    .expect("covered");
    assert!(page.items.is_empty());
    assert_eq!(page.total, 4);
}

#[test]
fn keyword_matches_name_or_path_and_glob_matches_name() {
    let dir = seeded();
    let by_name = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            keyword: Some("evi".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&by_name), vec!["f5"]);
    let by_path = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            keyword: Some("DOCS".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(by_path.total, 4);
    let glob = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            name_query: Some("file-?.log".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&glob), vec!["f4"]);
}

#[test]
fn anchored_path_query_is_relative_to_scope() {
    let dir = seeded();
    let at_root = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            path_query: Some("/docs/".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(at_root.total, 4);
    let in_docs = run(
        dir.path(),
        "d-docs",
        &FlatFileFilter {
            path_query: Some("/evi".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&in_docs), vec!["f5"]);
    let substring = run(
        dir.path(),
        "d-docs",
        &FlatFileFilter {
            path_query: Some("readme".into()),
            ..FlatFileFilter::default()
        }
        .normalized(),
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&substring), vec!["f7"]);
}

#[test]
fn time_range_uses_display_seconds_with_inclusive_bounds() {
    let dir = seeded();
    let same_second = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            time_modified: Some(FlatTimeRange {
                from: Some("2026-03-02 10:20:30".into()),
                to: Some("2026-03-02 10:20:30".into()),
            }),
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&same_second), vec!["f1"]);
    let whole_day = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            time_modified: Some(FlatTimeRange {
                from: Some("2026-03-02 00:00:00".into()),
                to: Some("2026-03-02 23:59:59".into()),
            }),
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    // f5 的 …23:59:59.900Z 落在含上界之内;无 mtime 的行被排除。
    assert_eq!(ids(&whole_day), vec!["f5", "f1"]);
}

#[test]
fn deleted_size_and_category_clauses() {
    let dir = seeded();
    let deleted = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            deleted: Some(true),
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&deleted), vec!["f8"]);
    let sized = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            size_ge: Some(1024),
            size_le: Some(2048),
            size_mod: Some(FlatSizeMod {
                divisor: 512,
                remainder: 0,
            }),
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&sized), vec!["f1", "f4"]);
    let mapped: Vec<String> = ["pdf", "png", "docx", "txt", "log", "tmp"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let pictures_and_pdf = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            extensions: vec!["pdf".into(), "png".into()],
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(ids(&pictures_and_pdf), vec!["f1", "f6"]);
    let others = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            other_category_exts: mapped.clone(),
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    // 无扩展名的 README 与符号链接 link 归「其他」。
    assert_eq!(ids(&others), vec!["f7", "s1"]);
    let both = run(
        dir.path(),
        "part:0",
        &FlatFileFilter {
            extensions: vec!["pdf".into()],
            other_category_exts: mapped,
            ..FlatFileFilter::default()
        },
        FlatSort::default(),
        0,
        50,
    )
    .expect("covered");
    assert_eq!(both.total, 3);
}

#[test]
fn ext_counts_histogram_counts_files_only() {
    let dir = seeded();
    let root = flat_ext_counts(dir.path(), "part:0", "part:0")
        .expect("counts")
        .expect("covered");
    let expected: BTreeMap<String, u64> = [
        ("", 1),
        ("docx", 1),
        ("log", 2),
        ("pdf", 1),
        ("png", 1),
        ("tmp", 1),
        ("txt", 2),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(
        root,
        FlatExtCounts {
            counts: expected,
            total: 9,
            deleted_dirs: vec![("d-gone".to_string(), "/gone".to_string())],
        }
    );
    let docs = flat_ext_counts(dir.path(), "part:0", "d-docs")
        .expect("counts")
        .expect("covered");
    assert_eq!(docs.total, 4);
    assert!(docs.deleted_dirs.is_empty());
}

#[test]
fn uncovered_scopes_fall_back_with_none() {
    let dir = seeded();
    let empty = FlatFileFilter::default();
    // 已删除目录:索引从未展开它(无论子项是否已在索引里)。
    assert!(run(dir.path(), "d-gone", &empty, FlatSort::default(), 0, 10).is_none());
    assert!(run(dir.path(), "d-dead", &empty, FlatSort::default(), 0, 10).is_none());
    // 未知节点 / 文件节点。
    assert!(run(dir.path(), "nope", &empty, FlatSort::default(), 0, 10).is_none());
    assert!(run(dir.path(), "f1", &empty, FlatSort::default(), 0, 10).is_none());
}

/// SQL 镜像 ↔ Rust 谓词/比较器对拍:同一份种子数据、同一组过滤与排序,
/// 索引侧结果必须与内存侧逐条一致(id 序列相同)。
#[test]
fn sql_matches_rust_predicate_and_comparator() {
    let dir = seeded();
    let memory: Vec<FsWalkEntry> = snapshot_entries()
        .into_iter()
        .map(|entry| FsWalkEntry {
            node: entry.node,
            path: entry.path,
        })
        .collect();
    let filters = vec![
        FlatFileFilter::default(),
        FlatFileFilter {
            keyword: Some("o".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            ext_query: Some("p".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            kind_query: Some("符号".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            kind_query: Some("symbolic".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            kind_query: Some("FILE".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            kind_query: Some("unallocated".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            link_query: Some("target".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            inode_query: Some("9".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            deleted: Some(false),
            size_ge: Some(6),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            time_created: Some(FlatTimeRange {
                from: Some("2025-12-31 00:00:00".into()),
                to: None,
            }),
            ..FlatFileFilter::default()
        },
    ];
    let sorts = std::iter::once(FlatSort::default()).chain(
        [
            FlatSortKey::Kind,
            FlatSortKey::Name,
            FlatSortKey::Ext,
            FlatSortKey::Size,
            FlatSortKey::Created,
            FlatSortKey::Accessed,
            FlatSortKey::Modified,
            FlatSortKey::Link,
            FlatSortKey::Inode,
            FlatSortKey::Path,
            FlatSortKey::Deleted,
        ]
        .into_iter()
        .flat_map(|key| {
            [
                FlatSort {
                    key: Some(key),
                    desc: false,
                },
                FlatSort {
                    key: Some(key),
                    desc: true,
                },
            ]
        }),
    );
    for sort in sorts {
        for filter in &filters {
            let filter = filter.clone().normalized();
            let mut expected: Vec<&FsWalkEntry> = memory
                .iter()
                .filter(|entry| flat_filter_keeps(entry, &filter))
                .collect();
            expected.sort_by(|left, right| sort.compare(left, right));
            let expected_ids: Vec<&str> = expected.iter().map(|e| e.node.id.as_str()).collect();
            let page = run(dir.path(), "part:0", &filter, sort, 0, 100).expect("covered");
            assert_eq!(ids(&page), expected_ids, "sort={sort:?} filter={filter:?}");
            assert_eq!(page.total, expected_ids.len() as u64);
            // 取满的一页才走 COUNT:用单行页对拍 COUNT 的 SQL 镜像。
            let counted = run(dir.path(), "part:0", &filter, sort, 0, 1).expect("covered");
            assert_eq!(
                counted.total,
                expected_ids.len() as u64,
                "sort={sort:?} filter={filter:?}"
            );
        }
    }
}

/// `EXPLAIN QUERY PLAN` 的 `(parent, detail)` 行;参数不绑定。
fn query_plan(conn: &Connection, sql: &str) -> Vec<(i64, String)> {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare plan");
    let mut rows = stmt.raw_query();
    let mut plan = Vec::new();
    while let Some(row) = rows.next().expect("plan row") {
        plan.push((row.get(1).expect("parent"), row.get(3).expect("detail")));
    }
    plan
}

/// 取页的外层循环必须是 LIMIT 子查询页,filedb 只按主键回表。没有统计时
/// 规划器的误判与数据量无关,种子库上就能复现:普通 JOIN 下这里会沿
/// flat_default 扫 f,再对每一行重跑一遍页子查询。
#[test]
fn page_query_runs_the_limited_page_as_the_outer_loop() {
    let dir = seeded();
    let conn = reader(&dir);
    let filters = [
        FlatFileFilter::default(),
        FlatFileFilter {
            keyword: Some("o".into()),
            ..FlatFileFilter::default()
        },
    ];
    let sorts = [
        FlatSort::default(),
        FlatSort {
            key: Some(FlatSortKey::Name),
            desc: false,
        },
        FlatSort {
            key: Some(FlatSortKey::Size),
            desc: true,
        },
    ];
    let subtree = FlatScope::Subtree {
        prefix: "/Docs".into(),
        prefix_low: "/docs".into(),
    };
    for scope in [&FlatScope::Volume, &subtree] {
        for filter in &filters {
            for sort in sorts {
                let clause = flat_clause(scope, filter);
                let loops: Vec<String> = query_plan(&conn, &page_sql(&clause, sort))
                    .into_iter()
                    .filter(|(parent, detail)| {
                        *parent == 0
                            && (detail.starts_with("SCAN ") || detail.starts_with("SEARCH "))
                    })
                    .map(|(_, detail)| detail)
                    .collect();
                assert_eq!(
                    loops,
                    ["SCAN page", "SEARCH f USING INTEGER PRIMARY KEY (rowid=?)"],
                    "sort={sort:?} filter={filter:?}"
                );
            }
        }
    }
}

/// 已删目录探测每取一页都要跑一次:整卷起点扫只收已删目录的部分索引,
/// 目录起点在它上面按前缀区间定位,都不碰整卷的已删除文件行。
#[test]
fn deleted_dir_probe_reads_only_deleted_directories() {
    let dir = seeded();
    let conn = reader(&dir);
    let subtree = FlatScope::Subtree {
        prefix: "/Docs".into(),
        prefix_low: "/docs".into(),
    };
    for (scope, expected) in [
        (&FlatScope::Volume, "SCAN f USING INDEX deleted_dir_path"),
        (
            &subtree,
            "SEARCH f USING INDEX deleted_dir_path (full_path_low>? AND full_path_low<?)",
        ),
    ] {
        let (sql, _) = deleted_dirs_sql(scope);
        let plan = query_plan(&conn, &sql);
        assert!(
            plan.iter()
                .any(|(parent, detail)| *parent == 0 && detail == expected),
            "{plan:?}"
        );
    }
}

/// 关键词、名称、路径子串、扩展名、类型与类别条过滤都在 flat_default 内
/// 求值:取页的内层与 COUNT 都是覆盖扫描,不逐行回表。
#[test]
fn name_path_and_extension_filters_are_answered_inside_flat_default() {
    let dir = seeded();
    let conn = reader(&dir);
    let covering = "SEARCH filedb USING COVERING INDEX flat_default (kind=?)";
    let filters = [
        FlatFileFilter {
            keyword: Some("evi".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            name_query: Some("file-?.log".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            path_query: Some("docs".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            ext_query: Some("p".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            kind_query: Some("文件".into()),
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            extensions: vec!["pdf".into()],
            ..FlatFileFilter::default()
        },
        FlatFileFilter {
            other_category_exts: vec!["pdf".into()],
            ..FlatFileFilter::default()
        },
    ];
    for filter in filters {
        let filter = filter.normalized();
        let clause = flat_clause(&FlatScope::Volume, &filter);
        for sql in [
            page_sql(&clause, FlatSort::default()),
            count_sql(&filter, &clause),
        ] {
            let plan = query_plan(&conn, &sql);
            assert!(
                plan.iter().any(|(_, detail)| detail == covering),
                "filter={filter:?} plan={plan:?}"
            );
        }
    }
}

/// 带路径区间(子树起点、锚定路径过滤)的取页与 COUNT 沿 full_path_low
/// 区间定位,耗时随子树大小;不能退化成整段扫描 flat_default。
#[test]
fn path_ranges_seek_the_full_path_index() {
    let dir = seeded();
    let conn = reader(&dir);
    let seek = "SEARCH filedb USING INDEX full_path_low \
                (full_path_low>? AND full_path_low<?)";
    let subtree = FlatScope::Subtree {
        prefix: "/Docs".into(),
        prefix_low: "/docs".into(),
    };
    let cases = [
        (&subtree, FlatFileFilter::default()),
        (
            &subtree,
            FlatFileFilter {
                keyword: Some("evi".into()),
                ..FlatFileFilter::default()
            },
        ),
        (
            &FlatScope::Volume,
            FlatFileFilter {
                path_query: Some("/docs/".into()),
                ..FlatFileFilter::default()
            },
        ),
    ];
    for (scope, filter) in cases {
        let filter = filter.normalized();
        let clause = flat_clause(scope, &filter);
        for sql in [
            page_sql(&clause, FlatSort::default()),
            count_sql(&filter, &clause),
        ] {
            let plan = query_plan(&conn, &sql);
            assert!(
                plan.iter().any(|(_, detail)| detail == seek),
                "filter={filter:?} plan={plan:?}"
            );
        }
    }
}

#[test]
fn unfiltered_volume_count_scans_the_narrow_kind_index() {
    let dir = seeded();
    let conn = reader(&dir);
    let filter = FlatFileFilter::default();
    let clause = flat_clause(&FlatScope::Volume, &filter);
    let plan = query_plan(&conn, &count_sql(&filter, &clause));
    assert_eq!(
        plan,
        [(
            0,
            "SEARCH filedb USING COVERING INDEX datatype (kind=?)".to_string()
        )]
    );
}
