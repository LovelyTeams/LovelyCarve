use super::test_support::{page_query, sample_hit, sample_meta};
use super::*;
use crate::casework::case_paths;

fn live(search_ids: &[&str]) -> HashSet<String> {
    search_ids.iter().map(|id| id.to_string()).collect()
}

/// 另一条连接持有检材 `search.db` 的写锁（模拟同一检材另一个搜索的收尾
/// 事务）。
fn hold_store_write_lock(case_dir: &Path, image_id: &str) -> Connection {
    let writer = open_store(case_dir, image_id).expect("store");
    writer
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("take store write lock");
    writer
}

#[test]
fn task_lifecycle_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut meta = sample_meta("search-1", "running");
    create_running(dir.path(), &meta).expect("create running");

    let entries = history_list(dir.path(), "image-1", 50, || live(&["search-1"])).expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, "running");
    assert_eq!(entries[0].keywords, vec!["password".to_string()]);
    assert_eq!(entries[0].regexes, vec![r"\d{4}".to_string()]);

    // 命中落库 + record_count 以实数回填
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[
            sample_hit(0, "a.txt", "password here"),
            sample_hit(1, "a.txt", "password again"),
            sample_hit(2, "b.txt", "no match text"),
        ],
    )
    .expect("append");

    meta.status = "success".to_string();
    meta.finished_ms = Some(2000);
    meta.matched_file_count = 2;
    meta.total_matches = 3;
    finish(dir.path(), &meta).expect("finish");
    let stored = history_get(dir.path(), "image-1", "search-1")
        .expect("get")
        .expect("present");
    assert_eq!(stored.status, "success");
    assert_eq!(stored.record_count, 3);
}

#[test]
fn paging_keyword_and_incremental() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    let hits: Vec<GlobalSearchHit> = (0..5)
        .map(|seq| {
            let context = if seq % 2 == 0 { "hit ALPHA" } else { "miss" };
            sample_hit(seq, &format!("f{seq}.txt"), context)
        })
        .collect();
    append_hits(dir.path(), "image-1", "search-1", &hits).expect("append");

    // 基础分页
    let first = page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert_eq!(first.items.len(), 5);
    assert_eq!(first.total, 5);

    // 增量：只取 seq>2
    let mut query = page_query("search-1");
    query.after_seq = Some(2);
    let incremental = page(dir.path(), "image-1", &query).expect("page");
    assert_eq!(incremental.items.len(), 2);
    assert_eq!(incremental.items[0].hit_seq, 3);
    assert_eq!(incremental.total, 2);

    // 关键词过滤命中 context_low
    let mut query = page_query("search-1");
    query.keyword = Some("alpha".to_string());
    let filtered = page(dir.path(), "image-1", &query).expect("page");
    assert_eq!(filtered.items.len(), 3);
    assert_eq!(filtered.total, 3);

    // 跳页
    let mut query = page_query("search-1");
    query.skip = 3;
    query.limit = 2;
    let skipped = page(dir.path(), "image-1", &query).expect("page");
    assert_eq!(skipped.items.len(), 2);
    assert_eq!(skipped.items[0].hit_seq, 3);
}

#[test]
fn distinct_file_and_sha256() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[
            sample_hit(0, "a.txt", "x"),
            sample_hit(1, "a.txt", "x"),
            sample_hit(2, "b.txt", "y"),
        ],
    )
    .expect("append");
    set_source_hashes(dir.path(), "image-1", "search-1", "a.txt", "m1", "s1").expect("hash a");
    set_source_hashes(dir.path(), "image-1", "search-1", "b.txt", "m2", "s1").expect("hash b");

    let mut query = page_query("search-1");
    query.distinct = GlobalSearchDistinct::File;
    let by_file = page(dir.path(), "image-1", &query).expect("page");
    assert_eq!(by_file.items.len(), 2);
    assert_eq!(by_file.items[0].hit_seq, 0);
    assert_eq!(by_file.items[1].hit_seq, 2);

    // a/b 同 sha256 → 按哈希只剩首条
    let mut query = page_query("search-1");
    query.distinct = GlobalSearchDistinct::Sha256;
    let by_hash = page(dir.path(), "image-1", &query).expect("page");
    assert_eq!(by_hash.items.len(), 1);
    assert_eq!(by_hash.items[0].hit_seq, 0);
}

#[test]
fn delete_removes_task_and_hits() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[sample_hit(0, "a.txt", "x")],
    )
    .expect("append");
    assert!(delete(dir.path(), "image-1", "search-1").expect("delete"));
    assert!(history_list(dir.path(), "image-1", 50, HashSet::new)
        .expect("list")
        .is_empty());
    let after_delete = page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert!(after_delete.items.is_empty());
}

fn statuses(entries: &[GlobalSearchMeta]) -> Vec<(&str, &str)> {
    let mut statuses: Vec<(&str, &str)> = entries
        .iter()
        .map(|meta| (meta.search_id.as_str(), meta.status.as_str()))
        .collect();
    statuses.sort_unstable();
    statuses
}

/// 孤儿修复后的 `meta`；修复时刻取自实际结果 `actual`。
fn orphaned(meta: GlobalSearchMeta, actual: &GlobalSearchMeta) -> GlobalSearchMeta {
    GlobalSearchMeta {
        status: "failed".to_string(),
        error: Some(ORPHANED_ERROR.to_string()),
        finished_ms: actual.finished_ms,
        ..meta
    }
}

#[test]
fn orphaned_running_marked_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create 1");
    create_running(dir.path(), &sample_meta("search-2", "running")).expect("create 2");
    let entries = history_list(dir.path(), "image-1", 50, || live(&["search-2"])).expect("list");
    assert_eq!(
        statuses(&entries),
        vec![("search-1", "failed"), ("search-2", "running")]
    );

    // 已落库：再列一次（两条都算在跑，无需修复）结果不变。
    let stored = history_list(dir.path(), "image-1", 50, || {
        live(&["search-1", "search-2"])
    })
    .expect("relist");
    assert_eq!(statuses(&stored), statuses(&entries));
    let orphan = history_get(dir.path(), "image-1", "search-1")
        .expect("get")
        .expect("present");
    assert!(orphan.finished_ms.is_some());
    assert_eq!(
        orphan,
        orphaned(sample_meta("search-1", "running"), &orphan)
    );
}

#[test]
fn orphaned_running_with_no_live_searches_clears_all() {
    // 重启后的常见路径：存活集为空，全部 running 行都是孤儿。
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create 1");
    create_running(dir.path(), &sample_meta("search-2", "running")).expect("create 2");
    let entries = history_list(dir.path(), "image-1", 50, HashSet::new).expect("list");
    assert_eq!(
        statuses(&entries),
        vec![("search-1", "failed"), ("search-2", "failed")]
    );
}

#[test]
fn search_finishing_during_listing_keeps_its_status() {
    // 读行时还是 running，取存活集前恰好收尾并移除登记：不能误判为孤儿。
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    let mut finished = sample_meta("search-1", "success");
    finished.finished_ms = Some(2000);
    let entries = history_list(dir.path(), "image-1", 50, || {
        finish(dir.path(), &finished).expect("finish");
        HashSet::new()
    })
    .expect("list");
    assert_eq!(entries, vec![finished.clone()]);
    let stored = history_get(dir.path(), "image-1", "search-1")
        .expect("get")
        .expect("present");
    assert_eq!(stored, finished);
}

#[test]
fn reads_without_a_store_create_nothing() {
    // 检材从未搜索过：读路径与删除都按空结果返回，不为此建库。
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        history_list(dir.path(), "image-1", 50, HashSet::new).expect("list"),
        Vec::new()
    );
    assert_eq!(
        history_get(dir.path(), "image-1", "search-1").expect("get"),
        None
    );
    let empty = page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert_eq!((empty.items, empty.total), (Vec::new(), 0));
    assert_eq!(
        export_rows(dir.path(), "image-1", &page_query("search-1"), |_| Ok(())).expect("export"),
        0
    );
    assert!(!delete(dir.path(), "image-1", "search-1").expect("delete"));
    assert!(!dir.path().join(case_paths::EVIDENCE_DIR).exists());
}

#[test]
fn searches_of_different_evidence_do_not_queue() {
    // 每个检材一个 search.db：另一个检材的搜索正持有写锁（收尾事务），
    // 本检材搜索的启动、命中落库、哈希回填、收尾、列表、分页与删除都不能
    // 排在它后面等满 60 s 的 busy_timeout。
    let dir = tempfile::tempdir().expect("tempdir");
    let _other = hold_store_write_lock(dir.path(), "image-2");
    let started = std::time::Instant::now();

    let mut meta = sample_meta("search-1", "running");
    create_running(dir.path(), &meta).expect("create running");
    let mut writer = HitWriter::open(dir.path(), "image-1", "search-1").expect("hit writer");
    writer
        .append(&[
            sample_hit(0, "a.txt", "password here"),
            sample_hit(1, "b.txt", "password there"),
        ])
        .expect("append");
    writer
        .set_source_hashes(&[("a.txt".to_string(), "m1".to_string(), "s1".to_string())])
        .expect("hashes");
    drop(writer);
    meta.status = "success".to_string();
    meta.finished_ms = Some(2000);
    finish(dir.path(), &meta).expect("finish");
    let listed = history_list(dir.path(), "image-1", 50, HashSet::new).expect("list");
    let hits = page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    let deleted = delete(dir.path(), "image-1", "search-1").expect("delete");

    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(10),
        "search waited {waited:?} for another evidence's search"
    );
    assert_eq!(
        listed,
        vec![GlobalSearchMeta {
            record_count: 2,
            ..meta
        }]
    );
    let mut hashed = sample_hit(0, "a.txt", "password here");
    hashed.md5 = "m1".to_string();
    hashed.sha256 = "s1".to_string();
    assert_eq!(
        (hits.items, hits.total),
        (vec![hashed, sample_hit(1, "b.txt", "password there")], 2)
    );
    assert!(deleted);
}

#[test]
fn finish_writes_every_counter() {
    // 命令层按启动时的样子落 running 行（计数全 0），收尾带回扫描摘要。
    let dir = tempfile::tempdir().expect("tempdir");
    let running = GlobalSearchMeta {
        files_total: 0,
        ..sample_meta("search-1", "running")
    };
    create_running(dir.path(), &running).expect("create");
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[sample_hit(0, "a.txt", "x")],
    )
    .expect("append");
    let finished = GlobalSearchMeta {
        status: "success".to_string(),
        finished_ms: Some(2000),
        files_total: 7,
        files_scanned: 6,
        matched_file_count: 1,
        total_matches: 3,
        record_count: 1,
        read_errors: 1,
        truncated: true,
        ..running
    };
    finish(dir.path(), &finished).expect("finish");
    assert_eq!(
        history_get(dir.path(), "image-1", "search-1").expect("get"),
        Some(finished)
    );
}

#[test]
fn finish_drops_hits_written_after_the_task_was_deleted() {
    // 删除运行中的搜索只等扫描停下 10 s：之后才停下的扫描还会落命中，
    // 收尾时任务行已经不在。
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    assert!(delete(dir.path(), "image-1", "search-1").expect("delete"));
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[sample_hit(0, "a.txt", "x")],
    )
    .expect("late hit");
    finish(dir.path(), &sample_meta("search-1", "cancelled")).expect("finish");
    assert_eq!(
        history_get(dir.path(), "image-1", "search-1").expect("get"),
        None
    );
    let remaining = page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert_eq!((remaining.items, remaining.total), (Vec::new(), 0));
}

#[test]
fn searches_stay_with_their_evidence() {
    // 搜索落在检材自己的库里：别的检材列不出、读不到、删不掉这次搜索。
    let dir = tempfile::tempdir().expect("tempdir");
    let meta = sample_meta("search-1", "success");
    create_running(dir.path(), &meta).expect("create");
    append_hits(
        dir.path(),
        "image-1",
        "search-1",
        &[sample_hit(0, "a.txt", "x")],
    )
    .expect("append");

    assert_eq!(
        history_list(dir.path(), "image-2", 50, HashSet::new).expect("list"),
        Vec::new()
    );
    assert_eq!(
        history_get(dir.path(), "image-2", "search-1").expect("get"),
        None
    );
    let foreign = page(dir.path(), "image-2", &page_query("search-1")).expect("page");
    assert_eq!((foreign.items, foreign.total), (Vec::new(), 0));
    assert_eq!(
        export_rows(dir.path(), "image-2", &page_query("search-1"), |_| Ok(())).expect("export"),
        0
    );
    assert!(!delete(dir.path(), "image-2", "search-1").expect("delete"));

    assert_eq!(
        history_list(dir.path(), "image-1", 50, HashSet::new).expect("list own"),
        vec![meta]
    );
    let own = page(dir.path(), "image-1", &page_query("search-1")).expect("page own");
    assert_eq!(own.items, vec![sample_hit(0, "a.txt", "x")]);
    assert!(case_paths::evidence_db_path(dir.path(), "image-1", EvidenceDb::Search).is_file());
    assert!(!case_paths::evidence_dir(dir.path(), "image-2").exists());
}

#[test]
fn orphan_fixup_does_not_wait_for_the_write_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_running(dir.path(), &sample_meta("search-1", "running")).expect("create");
    let writer = hold_store_write_lock(dir.path(), "image-1");

    // 库写锁被占（同一检材另一个搜索正在收尾）：孤儿只在结果里按中断呈现，
    // 库里仍是 running；列表不能等满 60 s 的 busy_timeout。
    let started = std::time::Instant::now();
    let entries = history_list(dir.path(), "image-1", 50, HashSet::new).expect("list");
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(10),
        "listing waited {waited:?}"
    );
    assert_eq!(entries.len(), 1);
    assert!(entries[0].finished_ms.is_some());
    assert_eq!(
        entries,
        vec![orphaned(sample_meta("search-1", "running"), &entries[0])]
    );
    let stored = history_get(dir.path(), "image-1", "search-1")
        .expect("get")
        .expect("present");
    assert_eq!(stored, sample_meta("search-1", "running"));

    // 写锁释放后，下一次列表把修复落库。
    writer
        .execute_batch("ROLLBACK;")
        .expect("release write lock");
    history_list(dir.path(), "image-1", 50, HashSet::new).expect("relist");
    let stored = history_get(dir.path(), "image-1", "search-1")
        .expect("get")
        .expect("present");
    assert!(stored.finished_ms.is_some());
    assert_eq!(
        stored,
        orphaned(sample_meta("search-1", "running"), &stored)
    );
}

#[test]
fn history_prunes_beyond_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..(HISTORY_MAX_PER_IMAGE + 5) {
        let mut meta = sample_meta(&format!("search-{index}"), "success");
        meta.started_ms = 1000 + index as i64;
        create_running(dir.path(), &meta).expect("create");
        let mut finished = meta.clone();
        finished.finished_ms = Some(finished.started_ms + 1);
        finish(dir.path(), &finished).expect("finish");
    }
    let entries = history_list(dir.path(), "image-1", 100, HashSet::new).expect("list");
    assert_eq!(entries.len(), HISTORY_MAX_PER_IMAGE);
    assert_eq!(
        entries[0].search_id,
        format!("search-{}", HISTORY_MAX_PER_IMAGE + 4)
    );
}

#[test]
fn distinct_parse_rejects_unknown() {
    assert_eq!(
        GlobalSearchDistinct::parse(None).expect("none"),
        GlobalSearchDistinct::None
    );
    assert_eq!(
        GlobalSearchDistinct::parse(Some("file")).expect("file"),
        GlobalSearchDistinct::File
    );
    assert_eq!(
        GlobalSearchDistinct::parse(Some("sha256")).expect("sha"),
        GlobalSearchDistinct::Sha256
    );
    assert!(GlobalSearchDistinct::parse(Some("md5")).is_err());
}
