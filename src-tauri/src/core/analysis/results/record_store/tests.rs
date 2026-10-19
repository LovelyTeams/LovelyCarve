use lovelycarve_plugin::{
    protocol::WireRecord, record::SourceRef, FieldKind, FieldSchema, RecordSchema, Severity, Value,
};

use super::*;
use crate::core::analysis::results::record_query::{self, RecordsQuery};

fn user_schema() -> RecordSchema {
    RecordSchema::new("test.user", "用户")
        .title("{name} ({uid})")
        .field(FieldSchema::new("name", "用户名", FieldKind::Text).key())
        .field(FieldSchema::new("uid", "UID", FieldKind::Int))
        .field(FieldSchema::new("size", "大小", FieldKind::Size))
        .field(FieldSchema::new("login", "登录", FieldKind::Timestamp).timeline("login"))
        .field(FieldSchema::new("groups", "组", FieldKind::List))
        .field(FieldSchema::new("admin", "管理员", FieldKind::Bool))
}

fn user(name: &str, uid: i64, login: Option<&str>) -> WireRecord {
    WireRecord {
        record_type: "test.user".to_owned(),
        values: vec![
            Value::from(name),
            Value::from(uid),
            Value::from(4096_u64),
            login.map_or(Value::Nil, Value::from),
            Value::Array(vec![Value::from("wheel"), Value::from("audio")]),
            Value::Boolean(uid == 0),
        ],
        title: None,
        severity: (uid == 0).then_some(Severity::High),
        sources: vec![SourceRef::path("part:1", "etc/passwd").note(format!("uid {uid}"))],
    }
}

fn begin(case_dir: &Path, run_id: &str, job: &str) {
    begin_run(
        case_dir,
        "image",
        &NewRun {
            run_id,
            job_key: job,
            plugin_id: "org.test",
            plugin_version: "1",
            analyzer: "users",
            analyzer_name: "用户",
            category: "baseinfo",
            target: &RunTarget {
                kind: "system".to_owned(),
                id: "linux@part:1".to_owned(),
                label: "Linux · part:1".to_owned(),
            },
            options: &serde_json::json!({ "items": [] }),
            profile: &serde_json::json!({}),
            inputs: &[RunInput {
                path: "etc/passwd".to_owned(),
                root: Some("part:1".to_owned()),
                root_path: Some("etc/passwd".to_owned()),
                node_id: Some("ext4:12".to_owned()),
                size: 64,
            }],
            schema_hash: "fnv1a64:0",
            started_at: "2026-10-19T00:00:00Z",
        },
    )
    .expect("begin run");
}

fn completion(faults: &ReadFaultReport) -> RunCompletion<'_> {
    RunCompletion {
        summary: Some("done"),
        finished_at: "2026-10-19T00:00:05Z",
        read_faults: faults,
    }
}

fn page<'a>(run_id: &'a str, sort: Option<&'a str>, keyword: Option<&'a str>) -> RecordsQuery<'a> {
    RecordsQuery {
        run_id,
        record_type: "test.user",
        keyword,
        sort,
        descending: false,
        skip: 0,
        limit: 100,
    }
}

#[test]
fn staged_records_are_typed_validated_and_published_with_provenance() {
    let directory = tempfile::tempdir().expect("case");
    begin(directory.path(), "run_a", "org.test/users#linux@part:1");
    let mut writer =
        RecordWriter::open(directory.path(), "image", "run_a", &[user_schema()]).expect("writer");
    assert_eq!(
        writer
            .insert_batch(&[
                user("root", 0, Some("2024-04-01T10:00:00+08:00")),
                user("kali", 1000, Some("2024-04-02 09:00:00")),
                user("daemon", 1, None),
            ])
            .expect("batch"),
        3
    );
    // A refused batch stores nothing.
    let mut wrong = user("bad", 7, None);
    wrong.values[1] = Value::from("seven");
    let refused = writer.insert_batch(&[user("ok", 8, None), wrong]);
    assert!(matches!(
        refused,
        Err(RecordWriteError::Rejected {
            code: "INVALID_RECORD",
            ..
        })
    ));
    let mut unknown = user("x", 9, None);
    unknown.record_type = "test.other".to_owned();
    assert!(matches!(
        writer.insert_batch(&[unknown]),
        Err(RecordWriteError::Rejected {
            code: "UNKNOWN_RECORD_TYPE",
            ..
        })
    ));
    writer
        .warn(
            "MALFORMED_LINE",
            "第 2 行字段不足",
            Some("etc/passwd"),
            Some("ext4:12"),
        )
        .expect("warning");
    assert_eq!(
        (writer.records(), writer.warnings(), writer.time_points()),
        (3, 1, 2)
    );
    drop(writer);

    // Staging runs are invisible.
    let connection = record_query::open(directory.path(), "image")
        .expect("open")
        .expect("records.db");
    assert!(record_query::published_runs(&connection)
        .expect("runs")
        .is_empty());

    let faults = ReadFaultReport::from_regions(vec![crate::core::evidence::image::BadRegion {
        offset: 512,
        len: 512,
    }]);
    let published = publish(directory.path(), "image", "run_a", &completion(&faults))
        .expect("publish")
        .expect("published");
    assert!(published.superseded.is_empty());
    let runs = record_query::published_runs(&connection).expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (
            runs[0].record_count,
            runs[0].warning_count,
            runs[0].synthetic_bytes
        ),
        (3, 1, 512)
    );

    let sorted =
        record_query::records_page(&connection, &page("run_a", Some("uid"), None)).expect("page");
    assert_eq!(
        sorted
            .items
            .iter()
            .map(|record| record.title.as_str())
            .collect::<Vec<_>>(),
        vec!["root (0)", "daemon (1)", "kali (1000)"]
    );
    let root = &sorted.items[0];
    assert_eq!(root.severity.as_deref(), Some("high"));
    assert_eq!(
        serde_json::Value::Object(root.values.clone()),
        serde_json::json!({
            "name": "root",
            "uid": 0,
            "size": 4096,
            "login": "2024-04-01T10:00:00+08:00",
            "groups": ["wheel", "audio"],
            "admin": true,
        })
    );
    assert_eq!(root.sources[0].root.as_deref(), Some("part:1"));
    assert_eq!(root.sources[0].note.as_deref(), Some("uid 0"));
    assert_eq!(sorted.columns[3].kind, "timestamp");
    assert_eq!(sorted.columns[3].timeline.as_deref(), Some("login"));

    // Timestamps sort by instant, local times by wall clock; NULL first.
    let by_login = record_query::records_page(&connection, &page("run_a", Some("login"), None))
        .expect("by login");
    assert_eq!(
        by_login
            .items
            .iter()
            .map(|record| record.values["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["daemon", "root", "kali"]
    );
    // Keywords search the title and every text-like column, lists included.
    let audio = record_query::records_page(&connection, &page("run_a", None, Some("AUDIO")))
        .expect("keyword");
    assert_eq!(audio.total, 3);
    let kali = record_query::records_page(&connection, &page("run_a", None, Some("kal")))
        .expect("keyword");
    assert_eq!(kali.total, 1);
    let percent = record_query::records_page(&connection, &page("run_a", None, Some("%")))
        .expect("literal percent");
    assert_eq!(percent.total, 0);

    let (detail, columns) =
        record_query::record_detail(&connection, root.record_id.parse().expect("id"))
            .expect("detail")
            .expect("record");
    assert_eq!(&detail, root);
    assert_eq!(columns, sorted.columns);

    let audit = record_query::run_audit(&connection, "run_a")
        .expect("audit")
        .expect("run");
    assert_eq!(audit.state, "published");
    assert_eq!(audit.warnings[0].node_id.as_deref(), Some("ext4:12"));
    assert_eq!(audit.inputs[0].path, "etc/passwd");
    assert_eq!(audit.bad_regions.len(), 1);
    assert_eq!(audit.record_types[0].count, 3);

    // Only the zoned timestamp is a timeline point.
    assert_eq!(
        record_query::count_time_points(directory.path(), "image").expect("points"),
        1
    );
}

#[test]
fn publication_replaces_the_job_and_refuses_discarded_runs() {
    let directory = tempfile::tempdir().expect("case");
    let job = "org.test/users#linux@part:1";
    let faults = ReadFaultReport::default();
    for run_id in ["run_old", "run_new"] {
        begin(directory.path(), run_id, job);
        let mut writer = RecordWriter::open(directory.path(), "image", run_id, &[user_schema()])
            .expect("writer");
        writer
            .insert_batch(&[user("root", 0, None)])
            .expect("batch");
    }
    publish(directory.path(), "image", "run_old", &completion(&faults))
        .expect("publish old")
        .expect("published");
    let replaced = publish(directory.path(), "image", "run_new", &completion(&faults))
        .expect("publish new")
        .expect("published");
    assert_eq!(replaced.superseded, vec!["run_old".to_owned()]);
    let connection = record_query::open(directory.path(), "image")
        .expect("open")
        .expect("records.db");
    assert_eq!(
        record_query::published_runs(&connection)
            .expect("runs")
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        vec!["run_new"]
    );
    // The superseded run keeps its audit but no records.
    let old = record_query::run_audit(&connection, "run_old")
        .expect("audit")
        .expect("old run");
    assert_eq!(old.state, "superseded");
    assert_eq!(
        record_query::records_page(&connection, &page("run_old", None, None))
            .expect("old page")
            .total,
        0
    );
    // Publishing again, or publishing a discarded run, does nothing.
    assert_eq!(
        publish(directory.path(), "image", "run_new", &completion(&faults)).expect("again"),
        None
    );
    begin(directory.path(), "run_gone", job);
    discard(directory.path(), "image", "run_gone").expect("discard");
    assert_eq!(
        publish(directory.path(), "image", "run_gone", &completion(&faults)).expect("discarded"),
        None
    );
    assert!(remove_run(directory.path(), "image", "run_new").expect("remove"));
    assert!(!remove_run(directory.path(), "image", "run_new").expect("again"));
    let records: i64 = connection
        .query_row("SELECT COUNT(*) FROM record", [], |row| row.get(0))
        .expect("count");
    assert_eq!(records, 0, "removal cascades to the records");
}

#[test]
fn staging_runs_of_an_exited_process_are_cleared() {
    let directory = tempfile::tempdir().expect("case");
    begin(directory.path(), "run_crashed", "org.test/users#");
    let mut writer = RecordWriter::open(directory.path(), "image", "run_crashed", &[user_schema()])
        .expect("writer");
    writer
        .insert_batch(&[user("root", 0, None)])
        .expect("batch");
    drop(writer);
    let connection = record_query::open(directory.path(), "image")
        .expect("open")
        .expect("records.db");
    connection
        .execute("UPDATE run SET owner = 'exited-process'", [])
        .expect("simulate a crashed process");
    begin(directory.path(), "run_next", "org.test/users#");
    let runs: Vec<String> = connection
        .prepare("SELECT run_id FROM run")
        .expect("statement")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("runs");
    assert_eq!(runs, vec!["run_next".to_owned()]);
    let records: i64 = connection
        .query_row("SELECT COUNT(*) FROM record", [], |row| row.get(0))
        .expect("count");
    assert_eq!(records, 0);
}

fn fact_schema() -> RecordSchema {
    RecordSchema::new(record_query::PROFILE_FACT, "检材画像")
        .title("{key}")
        .field(FieldSchema::new("key", "键", FieldKind::Text).key())
        .field(FieldSchema::new("value", "值", FieldKind::Json))
        .field(FieldSchema::new("system", "系统", FieldKind::Text))
}

/// Publishes one run on `target` (empty: the whole evidence) holding `facts`.
fn publish_facts(case_dir: &Path, run_id: &str, target: &str, facts: &[(&str, &str)]) {
    begin_run(
        case_dir,
        "image",
        &NewRun {
            run_id,
            job_key: &format!("org.test/facts#{target}"),
            plugin_id: "org.test",
            plugin_version: "1",
            analyzer: "facts",
            analyzer_name: "画像",
            category: "baseinfo",
            target: &RunTarget {
                kind: if target.is_empty() {
                    "evidence"
                } else {
                    "system"
                }
                .to_owned(),
                id: target.to_owned(),
                label: target.to_owned(),
            },
            options: &serde_json::json!({}),
            profile: &serde_json::json!({}),
            inputs: &[],
            schema_hash: "fnv1a64:0",
            started_at: "2026-10-19T00:00:00Z",
        },
    )
    .expect("begin run");
    let mut writer =
        RecordWriter::open(case_dir, "image", run_id, &[fact_schema()]).expect("writer");
    let records: Vec<WireRecord> = facts
        .iter()
        .map(|(key, value)| WireRecord {
            record_type: record_query::PROFILE_FACT.to_owned(),
            values: vec![Value::from(*key), Value::from(*value), Value::Nil],
            title: None,
            severity: None,
            sources: Vec::new(),
        })
        .collect();
    writer.insert_batch(&records).expect("facts");
    publish(
        case_dir,
        "image",
        run_id,
        &completion(&ReadFaultReport::default()),
    )
    .expect("publish")
    .expect("published");
}

#[test]
fn profile_facts_of_a_target_include_the_whole_evidence() {
    let directory = tempfile::tempdir().expect("case");
    publish_facts(
        directory.path(),
        "run_evidence",
        "",
        &[("timezone", "UTC"), ("hostname", "evidence")],
    );
    publish_facts(
        directory.path(),
        "run_kali",
        "linux@part:1",
        &[("hostname", "kali")],
    );
    publish_facts(
        directory.path(),
        "run_other",
        "linux@part:2",
        &[("hostname", "other"), ("users", "3")],
    );
    let connection = record_query::open(directory.path(), "image")
        .expect("open")
        .expect("records.db");
    let facts = |target: &str| -> Vec<(String, serde_json::Value)> {
        record_query::profile_facts(&connection, target)
            .expect("facts")
            .into_iter()
            .collect()
    };
    let text = |key: &str, value: &str| (key.to_owned(), serde_json::json!(value));
    assert_eq!(
        facts("linux@part:1"),
        vec![text("hostname", "kali"), text("timezone", "UTC")]
    );
    assert_eq!(
        facts(""),
        vec![text("hostname", "evidence"), text("timezone", "UTC")]
    );
    assert_eq!(
        facts("linux@part:2"),
        vec![
            text("hostname", "other"),
            text("timezone", "UTC"),
            text("users", "3")
        ]
    );
}

#[test]
fn readers_never_create_the_store() {
    let directory = tempfile::tempdir().expect("case");
    assert!(record_query::open(directory.path(), "image")
        .expect("open")
        .is_none());
    assert_eq!(
        record_query::max_published_at(directory.path(), "image").expect("watermark"),
        ""
    );
    discard(directory.path(), "image", "run_x").expect("discard without a store");
    assert!(!crate::core::casework::case_paths::evidence_db_path(
        directory.path(),
        "image",
        EvidenceDb::Records
    )
    .exists());
}
