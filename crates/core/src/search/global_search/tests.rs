use super::*;
use crate::casework::case::test_support::{case_with_evidence, publish_volume};
use crate::catalog::filedb::filedb_store::test_support::entry;
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::NodeKind;
use crate::search::global_search_store::test_support::{page_query, sample_hit, sample_meta};
use crate::search::global_search_store::{self, GlobalSearchMeta};
use crate::{casework::case_ledger, casework::case_paths};

fn hits(seqs: std::ops::Range<u64>) -> Vec<GlobalSearchHit> {
    seqs.map(|seq| sample_hit(seq, "a.txt", "password"))
        .collect()
}

fn keyword_options(scope: GlobalSearchScope) -> GlobalSearchOptions {
    GlobalSearchOptions {
        keys: vec!["password".to_string()],
        regexes: Vec::new(),
        codepages: vec!["ASCII".to_string()],
        match_case: false,
        search_deleted: true,
        search_empty_space: true,
        calc_hash: true,
        scope,
        per_file_cap: DEFAULT_PER_FILE_CAP,
        stop_after_first: false,
        global_cap: DEFAULT_GLOBAL_CAP,
    }
}

fn matchers_for(options: &GlobalSearchOptions) -> MatcherSet {
    let encodings = resolve_encodings(&options.codepages).expect("encodings");
    MatcherSet::build_plan(&options.keys, &options.regexes, false, &encodings).expect("matchers")
}

#[test]
fn failed_hit_batches_are_retried_not_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut writer = HitWriter::open(dir.path(), "image-1", "search-1").expect("hit writer");
    let mut pending = hits(0..3);
    writer.set_read_only(true);
    flush_hits(&mut writer, &mut pending).expect("a failed batch waits for the next one");
    assert_eq!(pending, hits(0..3));

    writer.set_read_only(false);
    pending.extend(hits(3..5));
    flush_hits(&mut writer, &mut pending).expect("retried with the next batch");
    assert_eq!(pending, Vec::new());
    let stored =
        global_search_store::page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert_eq!(stored.items, hits(0..5));
}

#[test]
fn persistent_write_failures_fail_instead_of_dropping() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut writer = HitWriter::open(dir.path(), "image-1", "search-1").expect("hit writer");
    writer.set_read_only(true);
    // 积压到上限仍写不进去：报错由调用方中止扫描，缓冲原样保留。
    let mut pending = hits(0..HIT_RETRY_BACKLOG as u64);
    assert!(flush_hits(&mut writer, &mut pending).is_err());
    assert_eq!(pending.len(), HIT_RETRY_BACKLOG);
    // 收尾没有下一批：写不进去就报错。
    let mut last = hits(0..1);
    assert!(write_batch(&mut last, |batch| writer.append(batch)).is_err());
    assert_eq!(last, hits(0..1));
}

#[test]
fn failed_hash_batches_are_retried_not_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut writer = HitWriter::open(dir.path(), "image-1", "search-1").expect("hit writer");
    let stored = vec![sample_hit(0, "a.txt", "x"), sample_hit(1, "b.txt", "y")];
    writer.append(&stored).expect("append");
    let hash_row = |node: &str| {
        (
            node.to_string(),
            format!("md5 {node}"),
            format!("sha {node}"),
        )
    };
    let mut batch = vec![hash_row("a.txt")];
    writer.set_read_only(true);
    write_hashes(&mut writer, &mut batch).expect("a failed batch waits for the next one");
    assert_eq!(batch, vec![hash_row("a.txt")]);

    writer.set_read_only(false);
    batch.push(hash_row("b.txt"));
    write_hashes(&mut writer, &mut batch).expect("retried with the next batch");
    assert_eq!(batch, Vec::new());
    let hashed: Vec<GlobalSearchHit> = stored
        .into_iter()
        .map(|mut hit| {
            hit.md5 = format!("md5 {}", hit.node_id);
            hit.sha256 = format!("sha {}", hit.node_id);
            hit
        })
        .collect();
    let page =
        global_search_store::page(dir.path(), "image-1", &page_query("search-1")).expect("page");
    assert_eq!(page.items, hashed);
}

/// 1 MiB 未分区原始镜像（不小于 `MIN_DISK_GAP`，整盘是一个磁盘间隙源），
/// 在 `offsets` 处埋入关键词。
fn planted_image(dir: &Path, offsets: &[u64]) -> Arc<EvidenceSession> {
    let mut data = vec![b'.'; 1024 * 1024];
    for &offset in offsets {
        let start = offset as usize;
        data[start..start + 8].copy_from_slice(b"password");
    }
    let path = dir.join("disk.raw");
    std::fs::write(&path, &data).expect("write raw image");
    Arc::new(EvidenceSession::open(&path).expect("open raw image"))
}

#[test]
fn search_while_the_ledger_is_written_neither_waits_nor_loses_hits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let case_dir = dir.path().join("case");
    let offsets = [0, 4096, 300_000, 1024 * 1024 - 8];
    let image = planted_image(dir.path(), &offsets);
    let _case = case_with_evidence(&case_dir, "image-1");
    // 另一个台账写者(任务调度、卷发布)正持有写锁:搜索只读台账、只写
    // 检材自己的 search.db,两边都不排队。
    let ledger_writer = case_ledger::open(&case_paths::ledger_path(&case_dir)).expect("ledger");
    ledger_writer
        .execute_batch("BEGIN IMMEDIATE; UPDATE case_profile SET name = 'busy';")
        .expect("hold the ledger write lock");

    let options = keyword_options(GlobalSearchScope::default());
    let matchers = matchers_for(&options);
    let cancel = CancellationToken::new();
    let params = GlobalSearchParams {
        image: &image,
        case_dir: &case_dir,
        image_id: "image-1",
        search_id: "search-1",
        options: &options,
        patterns: &options.keys,
        keyword_count: options.keys.len(),
        matchers: &matchers,
        cancel_flag: &cancel,
        on_progress: &|_: u64, _: u64, _: u64, _: u64| {},
    };

    // 命令层的完整写路径：落 running 行 → 扫描（命中落库 + 哈希回填）→ 收尾。
    let started = Instant::now();
    let running = GlobalSearchMeta {
        calc_hash: true,
        files_total: 0,
        ..sample_meta("search-1", "running")
    };
    global_search_store::create_running(&case_dir, &running).expect("create running");
    let summary = run_global_search(&params).expect("search");
    let finished = GlobalSearchMeta {
        status: summary.status.clone(),
        finished_ms: Some(2000),
        files_total: summary.files_total,
        files_scanned: summary.files_scanned,
        matched_file_count: summary.matched_file_count,
        total_matches: summary.total_matches,
        record_count: summary.record_count,
        read_errors: summary.read_errors,
        truncated: summary.truncated,
        ..running
    };
    global_search_store::finish(&case_dir, &finished).expect("finish");
    let waited = started.elapsed();

    assert!(
        waited < Duration::from_secs(5),
        "search waited {waited:?} for the ledger writer"
    );
    assert_eq!(
        (
            summary.status.as_str(),
            summary.files_total,
            summary.total_matches,
            summary.record_count
        ),
        ("success", 1, 4, 4)
    );
    let stored =
        global_search_store::page(&case_dir, "image-1", &page_query("search-1")).expect("page");
    assert_eq!(
        stored
            .items
            .iter()
            .map(|hit| hit.file_offset)
            .collect::<Vec<_>>(),
        offsets.to_vec()
    );
    assert!(stored.items.iter().all(|hit| hit.sha256.len() == 64));
    assert_eq!(
        global_search_store::history_get(&case_dir, "image-1", "search-1").expect("get"),
        Some(finished)
    );
}

#[test]
fn tree_scope_resolves_on_the_volume_that_holds_the_node() {
    let dir = tempfile::tempdir().expect("tempdir");
    let image = planted_image(dir.path(), &[]);
    let case_dir = dir.path().join("case");
    let mut case = case_with_evidence(&case_dir, "image-1");
    let built_at = "2026-01-01T00:00:00.000Z";
    publish_volume(
        &mut case,
        "image-1",
        "part:0",
        built_at,
        &[
            entry("part:0", "p0-docs", "docs", NodeKind::Dir, "/docs"),
            entry("p0-docs", "p0-a", "a.txt", NodeKind::File, "/docs/a.txt"),
            entry("part:0", "p0-docsx", "docsx", NodeKind::Dir, "/docsx"),
            entry("p0-docsx", "p0-x", "x.txt", NodeKind::File, "/docsx/x.txt"),
            entry("part:0", "p0-root", "root.txt", NodeKind::File, "/root.txt"),
        ],
    );
    publish_volume(
        &mut case,
        "image-1",
        "part:1",
        built_at,
        &[
            entry("part:1", "p1-docs", "docs", NodeKind::Dir, "/docs"),
            entry("p1-docs", "p1-b", "b.txt", NodeKind::File, "/docs/b.txt"),
            entry("part:1", "p1-note", "Note.TXT", NodeKind::File, "/Note.TXT"),
        ],
    );
    let cancel = CancellationToken::new();
    let resolve = |scope: GlobalSearchScope| {
        let options = keyword_options(scope);
        let matchers = matchers_for(&options);
        let params = GlobalSearchParams {
            image: &image,
            case_dir: &case_dir,
            image_id: "image-1",
            search_id: "search-1",
            options: &options,
            patterns: &options.keys,
            keyword_count: options.keys.len(),
            matchers: &matchers,
            cancel_flag: &cancel,
            on_progress: &|_: u64, _: u64, _: u64, _: u64| {},
        };
        resolve_sources(&params).map(|(sources, files_total)| {
            let nodes: Vec<String> = sources.into_iter().map(|source| source.node_id).collect();
            (nodes, files_total)
        })
    };

    // A directory scopes its own volume only (not the same path on part:1,
    // not the `/docsx` sibling); a path resolves case-insensitively to the
    // node, wherever it lives.
    let tree = GlobalSearchScope {
        tree_nodes: vec!["p0-docs".to_string(), "/note.txt".to_string()],
        ..GlobalSearchScope::default()
    };
    assert_eq!(
        resolve(tree.clone()).expect("tree scope"),
        (vec!["p0-a".to_string(), "p1-note".to_string()], 2)
    );
    // Partition scope still applies to tree-scoped nodes.
    let narrowed = GlobalSearchScope {
        partitions: vec!["part:1".to_string()],
        ..tree
    };
    assert_eq!(
        resolve(narrowed).expect("narrowed"),
        (vec!["p1-note".to_string()], 1)
    );
    let missing = GlobalSearchScope {
        tree_nodes: vec!["gone".to_string()],
        ..GlobalSearchScope::default()
    };
    assert!(resolve(missing).is_err());
    // Whole evidence: every file on every indexed volume, plus the disk gap.
    let (nodes, files_total) = resolve(GlobalSearchScope::default()).expect("all");
    assert_eq!(files_total, 6);
    assert_eq!(
        nodes[..5],
        ["p0-a", "p0-x", "p0-root", "p1-b", "p1-note"].map(String::from)
    );
}

#[test]
fn volume_scope_covers_lvm_children() {
    let selected = vec!["part:2".to_string()];
    assert!(volume_in_partition_scope("part:2", &selected));
    assert!(volume_in_partition_scope("lvm:2:0", &selected));
    assert!(volume_in_partition_scope("lvm:2:3", &selected));
    assert!(!volume_in_partition_scope("part:3", &selected));
    assert!(!volume_in_partition_scope("lvm:3:0", &selected));
}

#[test]
fn resolve_encodings_normalizes_and_validates() {
    let encodings =
        resolve_encodings(&["gbk".to_string(), "UTF-16LE".to_string()]).expect("resolve");
    // 按 ALL 优先级排序：UTF-16LE 在 GBK 前
    assert_eq!(
        encodings,
        vec![SearchEncoding::Utf16Le, SearchEncoding::Gbk]
    );
    assert_eq!(
        resolve_encodings(&[]).unwrap(),
        SearchEncoding::ALL.to_vec()
    );
    assert!(resolve_encodings(&["latin1".to_string()]).is_err());
}

#[test]
fn subtree_bounds_form_half_open_prefix() {
    // 目录 /a/b：子树上界 /a/b/\u{10FFFF} 排除兄弟 /a/bc
    let dir_low = "/a/b".to_string();
    let high = format!("{dir_low}/\u{10FFFF}");
    assert!("/a/b" >= dir_low.as_str() && "/a/b" < high.as_str());
    assert!("/a/b/c.txt" >= dir_low.as_str() && "/a/b/c.txt" < high.as_str());
    assert!(!("/a/bc" < high.as_str()));
    assert!(!("/a" >= dir_low.as_str()));
}
