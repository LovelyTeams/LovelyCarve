use super::*;

/// tempdir 与连接一起持有，目录生命周期覆盖连接。
struct Store {
    _dir: tempfile::TempDir,
    conn: Connection,
}

fn store() -> Store {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = open_writer(dir.path(), "img-1").expect("timeline store");
    Store { _dir: dir, conn }
}

fn file_row(oid: &str, tl_time: i64) -> TimelineInsertRow {
    TimelineInsertRow {
        oid: oid.to_string(),
        tid: String::new(),
        deleted: false,
        tl_time,
        from_app: "part:0".to_string(),
        data_type: DATA_TYPE_FILE,
        tl_flag: "修改".to_string(),
        content: format!("/data/{oid}"),
        extra: String::new(),
    }
}

fn event_row(oid: &str, tl_time: i64, tid: &str) -> TimelineInsertRow {
    TimelineInsertRow {
        oid: oid.to_string(),
        tid: tid.to_string(),
        deleted: false,
        tl_time,
        from_app: "org.demo".to_string(),
        data_type: DATA_TYPE_PLUGIN_EVENT,
        tl_flag: "browser.visit".to_string(),
        content: "访问 https://example.test".to_string(),
        extra: String::new(),
    }
}

#[test]
fn readers_see_nothing_until_a_writer_creates_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(open_reader(dir.path(), "img-1").expect("reader").is_none());
    drop(open_writer(dir.path(), "img-1").expect("first open"));
    drop(open_writer(dir.path(), "img-1").expect("second open"));
    assert!(dir
        .path()
        .join("evidence")
        .join("img-1")
        .join("timeline.db")
        .is_file());
    assert!(open_reader(dir.path(), "img-1").expect("reader").is_some());
}

#[test]
fn run_deletion_and_clear_remove_projections() {
    let store = store();
    insert_rows(
        &store.conn,
        &[
            file_row("a", 100),
            file_row("b", 200),
            event_row("event-1", 300, "run-1"),
        ],
    )
    .expect("insert");
    assert_eq!(
        delete_runs(&store.conn, &["run-1".to_string()]).expect("unbuilt"),
        0,
        "a timeline that was never built is left to the rebuild"
    );
    meta_begin(&store.conn, 1).expect("begin");
    assert_eq!(
        delete_runs(&store.conn, &["run-1".to_string()]).expect("delete run"),
        1
    );
    clear(&store.conn).expect("clear");
    assert_eq!(time_range(&store.conn).expect("range"), None);
}

#[test]
fn count_buckets_groups_by_strftime_with_timezone() {
    let store = store();
    // 2023-12-31 23:30 UTC 与 2024-01-01 00:30 UTC：UTC+8 下同属 2024 年桶，
    // UTC 下分属 2023/2024——验证时区参数化聚合。
    insert_rows(
        &store.conn,
        &[
            file_row("a", 1704067200 - 1800),
            file_row("b", 1704067200 + 1800),
        ],
    )
    .expect("insert");

    let query = |tz: i32| TimelineQuery {
        time_zone_hours: tz,
        time_unit: TimelineUnit::Year,
        show_delete: true,
        ..Default::default()
    };
    let utc = count_buckets(&store.conn, &query(0), None, 0).expect("utc buckets");
    assert_eq!(
        utc,
        vec![
            TimelineBucket {
                date_str: "2023".into(),
                count: 1
            },
            TimelineBucket {
                date_str: "2024".into(),
                count: 1
            },
        ]
    );
    let utc8 = count_buckets(&store.conn, &query(8), None, 0).expect("utc8 buckets");
    assert_eq!(
        utc8,
        vec![TimelineBucket {
            date_str: "2024".into(),
            count: 2
        }]
    );
}

#[test]
fn count_buckets_limit_offset_pages() {
    let store = store();
    insert_rows(
        &store.conn,
        &[
            file_row("a", 1704067200),
            file_row("b", 1704153600),
            file_row("c", 1704240000),
        ],
    )
    .expect("insert");
    let query = TimelineQuery {
        time_unit: TimelineUnit::Day,
        show_delete: true,
        ..Default::default()
    };
    let first = count_buckets(&store.conn, &query, Some(2), 0).expect("page 1");
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].date_str, "2024-01-01");
    let second = count_buckets(&store.conn, &query, Some(2), 2).expect("page 2");
    assert_eq!(second.len(), 1);
}

#[test]
fn page_filters_sorts_and_pages() {
    let store = store();
    let mut deleted = file_row("del", 1704067200);
    deleted.deleted = true;
    insert_rows(
        &store.conn,
        &[
            file_row("a", 1704067100),
            deleted,
            event_row("event-9", 1704153600, "run-9"),
        ],
    )
    .expect("insert");

    let base = TimelineQuery {
        show_delete: true,
        ..Default::default()
    };
    // 默认 tl_time desc。
    let page_all = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: base.clone(),
            skip: 0,
            limit: 10,
            ascending: false,
        },
    )
    .expect("page");
    assert_eq!(page_all.total, 3);
    assert_eq!(page_all.items[0].oid, "event-9");
    assert_eq!(page_all.items[0].eid, "img-1");

    // show_delete=false 剔除软删行；asc 排序。
    let mut live_only = base.clone();
    live_only.show_delete = false;
    let live = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: live_only,
            skip: 0,
            limit: 10,
            ascending: true,
        },
    )
    .expect("live page");
    assert_eq!(live.total, 2);
    assert_eq!(live.items[0].oid, "a");

    // from_app + text + 时间范围过滤。
    let mut filtered = base;
    filtered.from_apps = vec!["org.demo".into()];
    filtered.text = Some("example".into());
    filtered.end = Some(1704153600);
    let hit = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: filtered,
            skip: 0,
            limit: 10,
            ascending: false,
        },
    )
    .expect("filtered page");
    assert_eq!(hit.total, 1);
    assert_eq!(hit.items[0].oid, "event-9");
    assert_eq!(hit.items[0].tid, "run-9");

    // skip/limit 分页。
    let paged = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: TimelineQuery {
                show_delete: true,
                ..Default::default()
            },
            skip: 1,
            limit: 1,
            ascending: true,
        },
    )
    .expect("paged");
    assert_eq!(paged.items.len(), 1);
    assert_eq!(paged.items[0].oid, "del");
}

#[test]
fn types_groups_two_levels() {
    let store = store();
    insert_rows(
        &store.conn,
        &[file_row("a", 100), event_row("event-1", 100, "run-1")],
    )
    .expect("insert");

    let groups = types(&store.conn).expect("types");
    assert_eq!(
        groups,
        vec![
            TimelineTypeGroup {
                from_app: "org.demo".into(),
                types: vec![TimelineTypeInfo {
                    data_type: DATA_TYPE_PLUGIN_EVENT,
                    data_type_name: "插件时间线".into(),
                    count: 1,
                }],
            },
            TimelineTypeGroup {
                from_app: "part:0".into(),
                types: vec![TimelineTypeInfo {
                    data_type: DATA_TYPE_FILE,
                    data_type_name: "文件".into(),
                    count: 1
                }],
            },
        ]
    );
}

#[test]
fn time_range_reports_min_max() {
    let store = store();
    assert_eq!(
        time_range(&store.conn).expect("range"),
        None,
        "empty table has no range"
    );
    insert_rows(&store.conn, &[file_row("a", 100), file_row("b", 300)]).expect("insert");
    assert_eq!(time_range(&store.conn).expect("range"), Some((100, 300)));
}

#[test]
fn meta_round_trip_and_watermark_only_moves_forward() {
    let store = store();
    assert!(meta_get(&store.conn).expect("meta").is_none());
    meta_begin(&store.conn, 1000).expect("begin");
    assert_eq!(
        meta_get(&store.conn).expect("meta").unwrap().status,
        "running"
    );
    let mut meta = meta_get(&store.conn).expect("meta").unwrap();
    meta.status = "success".into();
    meta.file_rows = 10;
    meta.event_rows = 2;
    meta.source_file_watermark = "2026-01-01T00:00:00Z".into();
    meta.source_event_watermark = "2026-01-02T00:00:00Z".into();
    meta_finish(&store.conn, &meta, 2000).expect("finish");
    let stored = meta_get(&store.conn).expect("meta").unwrap();
    assert_eq!(stored.status, "success");
    assert_eq!(stored.finished_ms, Some(2000));
    assert_eq!(stored.file_rows, 10);

    // 两条任务的钩子乱序提交：较早完成的 run 不会把水位拉回去。
    for completed_at in ["2026-02-01T00:00:00Z", "2026-01-15T00:00:00Z"] {
        project_run_rows(&store.conn, "run-x", &[], &[], completed_at)
            .expect("project")
            .expect("timeline is built");
    }
    assert_eq!(
        meta_get(&store.conn)
            .expect("meta")
            .unwrap()
            .source_event_watermark,
        "2026-02-01T00:00:00Z"
    );
}

#[test]
fn run_projection_replaces_rows_by_tid_and_skips_unbuilt_or_rebuilding_timelines() {
    let store = store();
    let run_rows = |n: i32| vec![event_row(&format!("event-{n}"), 100, "run-1")];
    // 时间线未建：钩子跳过，交给全量重建。
    assert_eq!(
        project_run_rows(&store.conn, "run-1", &[], &run_rows(1), "t1").expect("skip"),
        None
    );
    meta_begin(&store.conn, 1).expect("begin");
    // 重建进行中：同样跳过，重建会投影这个 run。
    assert_eq!(
        project_run_rows(&store.conn, "run-1", &[], &run_rows(1), "t1").expect("skip"),
        None
    );
    let mut meta = meta_get(&store.conn).expect("meta").unwrap();
    meta.status = "success".into();
    meta_finish(&store.conn, &meta, 2).expect("finish");

    project_run_rows(&store.conn, "run-1", &[], &run_rows(2), "t1")
        .expect("project")
        .expect("projected");
    // 重投影同一 run：先按 tid 删旧，再插新，行数不膨胀。
    assert_eq!(
        project_run_rows(&store.conn, "run-1", &[], &run_rows(1), "t2").expect("again"),
        Some(1)
    );
    let page_all = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: TimelineQuery {
                show_delete: true,
                ..Default::default()
            },
            skip: 0,
            limit: 10,
            ascending: true,
        },
    )
    .expect("page");
    assert_eq!(page_all.total, 1);
}

#[test]
fn export_rows_streams_in_time_order() {
    let store = store();
    insert_rows(
        &store.conn,
        &[file_row("late", 300), file_row("early", 100)],
    )
    .expect("insert");
    let mut seen = Vec::new();
    let count = export_rows(&store.conn, "img-1", &TimelineQuery::default(), |row| {
        seen.push(row.oid.clone());
        Ok(())
    })
    .expect("export");
    assert_eq!(count, 2);
    assert_eq!(seen, vec!["early".to_string(), "late".to_string()]);
}

#[test]
fn pairs_filter_matches_from_app_type_exactly() {
    let store = store();
    insert_rows(
        &store.conn,
        &[file_row("a", 100), event_row("event-1", 200, "run-1")],
    )
    .expect("insert");

    // 勾选 (part:0, 文件)：只命中文件行，不含 org.demo 的事件行。
    let mut with_pair = TimelineQuery {
        show_delete: true,
        ..Default::default()
    };
    with_pair.pairs = vec![TimelineTypePair {
        from_app: "part:0".into(),
        data_type: DATA_TYPE_FILE,
    }];
    let only_file = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: with_pair.clone(),
            skip: 0,
            limit: 10,
            ascending: true,
        },
    )
    .expect("page");
    assert_eq!(only_file.total, 1);
    assert_eq!(only_file.items[0].oid, "a");

    // pairs 非空时优先于 from_apps/data_types 独立列表。
    with_pair.from_apps = vec!["org.demo".into()];
    with_pair.data_types = vec![DATA_TYPE_PLUGIN_EVENT];
    let still_pair = page(
        &store.conn,
        "img-1",
        &TimelinePageQuery {
            query: with_pair,
            skip: 0,
            limit: 10,
            ascending: true,
        },
    )
    .expect("page");
    assert_eq!(still_pair.total, 1);
    assert_eq!(still_pair.items[0].oid, "a");
}

#[test]
fn unit_ladder_drills_and_parses() {
    assert_eq!(TimelineUnit::Year.drill_down(), TimelineUnit::Month);
    assert_eq!(TimelineUnit::Hour.drill_down(), TimelineUnit::Minute);
    assert_eq!(TimelineUnit::Second.drill_down(), TimelineUnit::Second);
    assert_eq!(
        TimelineUnit::parse("minute").expect("parse"),
        TimelineUnit::Minute
    );
    assert!(TimelineUnit::parse("week").is_err());
    assert_eq!(TimelineUnit::Minute.strftime_format(), "%Y-%m-%d %H:%M");
}
