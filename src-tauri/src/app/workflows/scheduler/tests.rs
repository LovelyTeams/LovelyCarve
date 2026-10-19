//! The task runner end to end: a plugin built with the SDK, served over LPP
//! frames in this process, analyzes a folder evidence through the
//! scheduler; its records are published, re-runs replace them and task
//! deletion removes them.

use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use lovelycarve_plugin::{
    abi::{HostApiV1, PluginApiV1},
    runtime, Error, Plugin, Record, Run, Timestamp,
};
use parking_lot::RwLock;

use super::PluginTaskRunner;
use crate::core::{
    analysis::{
        identify::systems::SystemCache,
        results::{
            analysis_query::{self, AnalysisRecordsQuery, AnalysisTreeEvidence},
            record_query,
        },
    },
    extensions::{
        plugin::PluginManager,
        task_scheduler::{
            FunctionRegistry, SchedTaskRecord, SchedTaskStatus, SchedulerConfig, SystemTarget,
            TaskParams, TaskRunner, TaskScheduler,
        },
    },
    filesystem::evidence_session::EvidenceSession,
};

const PLUGIN_ID: &str = "org.test.e2e";

#[derive(Record)]
#[record(id = "e2e.user", label = "用户", title = "{name}")]
struct User {
    #[field(label = "用户名", key)]
    name: String,
    #[field(label = "UID")]
    uid: i64,
    #[field(label = "最后登录", timeline = "login")]
    last_login: Option<Timestamp>,
}

#[derive(Record)]
#[record(id = "e2e.hostfile", label = "主机名文件")]
struct HostFile {
    #[field(label = "内容", key)]
    content: String,
}

#[derive(Record)]
#[record(id = "e2e.line", label = "行")]
struct Line {
    #[field(label = "序号", key)]
    index: i64,
}

/// More than LPP v1's 10 000-record result cap, over many emit batches.
const BULK_RECORDS: i64 = 25_000;

fn users(run: &Run) -> lovelycarve_plugin::Result<()> {
    let view = run.require_view()?;
    let passwd = view
        .open("etc/passwd")?
        .ok_or_else(|| Error::not_found("etc/passwd"))?;
    for (index, line) in passwd.read_lines(1 << 20)?.iter().enumerate() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 {
            run.warn_file(
                &passwd,
                "MALFORMED_LINE",
                format!("第 {} 行字段不足", index + 1),
            )?;
            continue;
        }
        run.record(&User {
            name: fields[0].to_owned(),
            uid: fields[2].parse().unwrap_or(-1),
            last_login: (fields[0] == "root")
                .then(|| Timestamp::parse("2024-04-01T10:00:00+08:00"))
                .flatten(),
        })
        .from(&passwd)
        .note(format!("第 {} 行", index + 1))
        .emit()?;
    }
    if let Some(hostname) = view.read_text("etc/hostname", 256)? {
        run.provide("hostname", hostname.trim())?;
    }
    run.set_summary(format!("{} records", run.emitted()));
    Ok(())
}

fn host_files(run: &Run) -> lovelycarve_plugin::Result<()> {
    for input in run.inputs() {
        run.record(&HostFile {
            content: input.read_text(256)?.trim().to_owned(),
        })
        .from(input)
        .emit()?;
    }
    Ok(())
}

fn bulk(run: &Run) -> lovelycarve_plugin::Result<()> {
    for index in 0..BULK_RECORDS {
        run.emit(&Line { index })?;
    }
    Ok(())
}

fn e2e_plugin() -> Plugin {
    Plugin::new(PLUGIN_ID)
        .record::<User>()
        .record::<HostFile>()
        .record::<Line>()
        .analyzer("users", users)
        .analyzer("hosts", host_files)
        .analyzer("bulk", bulk)
}

unsafe extern "C" fn e2e_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
    unsafe { runtime::entry(host, plugin, e2e_plugin) }
}

const MANIFEST: &str = r#"
id = "org.test.e2e"
name = "端到端"
version = "1.0.0"
protocol = 2
library = "e2e"

[[analyzer]]
id = "users"
name = "用户"
scope = "system"
platforms = ["linux"]
category = "baseinfo"
provides = ["hostname"]

[[analyzer]]
id = "hosts"
name = "主机名文件"
scope = "evidence"
inputs = ["**/hostname"]

[[analyzer]]
id = "bulk"
name = "批量"
scope = "evidence"
"#;

struct Fixture {
    directory: tempfile::TempDir,
    image: Arc<EvidenceSession>,
    scheduler: TaskScheduler,
    manager: Arc<PluginManager>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let evidence = directory.path().join("evidence");
        let root = evidence.join("kali-root");
        write(
            &root.join("etc/os-release"),
            "NAME=\"Kali GNU/Linux\"\nVERSION=\"2024.1\"\nID=kali\n",
        );
        write(
            &root.join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nbroken\nkali:x:1000:1000::/home/kali:/bin/zsh\n",
        );
        write(&root.join("etc/hostname"), "forensic-box\n");
        let package = directory.path().join("package");
        write(&package.join("plugin.toml"), MANIFEST);
        let image = Arc::new(EvidenceSession::open(&evidence).expect("folder evidence"));

        let manager = Arc::new(PluginManager::discover(&[package], None).expect("discover"));
        manager.serve_in_process(PLUGIN_ID, e2e_entry);
        let plugins = Arc::new(RwLock::new(Some(Arc::clone(&manager))));
        let images = Arc::new(tokio::sync::Mutex::new(HashMap::from([(
            image.id.clone(),
            Arc::clone(&image),
        )])));
        let systems = Arc::new(SystemCache::default());
        let registry_manager = Arc::clone(&manager);
        let runner_systems = Arc::clone(&systems);
        let scheduler = TaskScheduler::new(
            SchedulerConfig {
                dispatch_interval: Duration::from_secs(3600),
                check_interval: Duration::from_secs(3600),
                offline_timeout: Duration::from_secs(3600),
                update_timeout: Duration::from_secs(3600),
                post_action_debounce: Duration::from_millis(1),
                ..SchedulerConfig::default()
            },
            Arc::new(move || FunctionRegistry::from_definitions(&registry_manager.definitions())),
            Arc::new(move |case_dir: &Path| {
                Arc::new(PluginTaskRunner::new(
                    Arc::clone(&images),
                    Arc::clone(&plugins),
                    Arc::clone(&runner_systems),
                    case_dir.to_owned(),
                )) as Arc<dyn TaskRunner>
            }),
        );
        let source_image = Arc::clone(&image);
        scheduler.set_systems_source(move |image_id| {
            if image_id != source_image.id {
                return Vec::new();
            }
            systems
                .get_or_discover(&source_image)
                .iter()
                .map(|system| SystemTarget {
                    id: system.id.clone(),
                    family: system.family.clone(),
                    label: system.label.clone(),
                })
                .collect()
        });
        let case_dir = directory.path().join("case");
        std::fs::create_dir_all(&case_dir).expect("case directory");
        let ledger = crate::core::casework::case_ledger::test_support::ledger(&case_dir);
        crate::core::casework::case_ledger_evidence::test_support::add(&ledger, &image.id);
        drop(ledger);
        scheduler
            .bind_case(&case_dir.join("case.db"), case_dir.clone())
            .await
            .expect("bind case");
        Self {
            directory,
            image,
            scheduler,
            manager,
        }
    }

    fn case_dir(&self) -> std::path::PathBuf {
        self.directory.path().join("case")
    }

    async fn create(&self, function: &str, params: TaskParams) -> SchedTaskRecord {
        let scheduler = self.scheduler.clone();
        let image_id = self.image.id.clone();
        let function = function.to_owned();
        tokio::task::spawn_blocking(move || scheduler.create_task(&function, &image_id, params))
            .await
            .expect("create task thread")
            .expect("create task")
            .into_record()
    }

    async fn finish(&self, task: &SchedTaskRecord) -> SchedTaskRecord {
        for _ in 0..600 {
            self.scheduler.dispatch_tick().await;
            let current = self.scheduler.get(task.id).expect("task");
            if current.status.is_terminal() {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task {} did not finish", task.id);
    }

    /// The folder's Linux system: found in a directory one level below the
    /// folder's root.
    fn system_id(&self) -> String {
        "linux@root/kali-root".to_owned()
    }
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("directories");
    std::fs::write(path, content).expect("fixture file");
}

#[tokio::test(flavor = "multi_thread")]
async fn system_runs_publish_typed_records_with_provenance_and_facts() {
    let fixture = Fixture::new().await;
    let systems = SystemCache::default().get_or_discover(&fixture.image);
    assert_eq!(
        systems
            .iter()
            .map(|system| system.id.as_str())
            .collect::<Vec<_>>(),
        vec![fixture.system_id().as_str()]
    );
    let task = fixture
        .create(
            "org.test.e2e/users",
            TaskParams {
                system_id: Some(fixture.system_id()),
                ..TaskParams::default()
            },
        )
        .await;
    assert_eq!(
        task.params.target_label.as_deref(),
        Some(systems[0].label.as_str())
    );
    let finished = fixture.finish(&task).await;
    assert_eq!(
        finished.status,
        SchedTaskStatus::Success,
        "{:?}",
        finished.info.error
    );
    let result = finished.result.expect("result");
    assert_eq!(result.record_count, 3, "two users and the hostname fact");
    assert_eq!(result.warning_count, 1);
    assert_eq!(result.timeline_count, 1);
    assert_eq!(result.summary.as_deref(), Some("3 records"));
    let run_id = result.run_id.expect("published run");

    let case_dir = fixture.case_dir();
    let tree = analysis_query::analysis_tree(
        &case_dir,
        &[AnalysisTreeEvidence {
            image_id: fixture.image.id.clone(),
            label: "kali-root".to_owned(),
        }],
    )
    .expect("tree");
    let class = &tree[0].children[0];
    assert_eq!(class.type_name, "baseinfo");
    let function = &class.children[0];
    assert_eq!(
        (
            function.run_id.as_str(),
            function.warning_count,
            function.legacy
        ),
        (run_id.as_str(), 1, false)
    );
    assert_eq!(function.target_label, systems[0].label);
    assert_eq!(
        function
            .children
            .iter()
            .map(|node| (node.artifact_type.as_str(), node.child_count))
            .collect::<Vec<_>>(),
        vec![("e2e.user", 2)],
        "profile facts are not a result item"
    );

    let page = analysis_query::records_page(
        &case_dir,
        &AnalysisRecordsQuery {
            image_id: &fixture.image.id,
            run_id: &run_id,
            record_type: "e2e.user",
            legacy: false,
            keyword: None,
            sort: Some("uid"),
            descending: true,
            skip: 0,
            limit: 10,
        },
    )
    .expect("page");
    assert_eq!(page.total, 2);
    assert_eq!(
        page.columns
            .iter()
            .map(|column| (column.id.as_str(), column.kind))
            .collect::<Vec<_>>(),
        vec![
            ("name", "text"),
            ("uid", "int"),
            ("last_login", "timestamp")
        ]
    );
    let kali = &page.items[0];
    assert_eq!(kali.title, "kali");
    assert_eq!(kali.values["uid"], 1000);
    let source = &kali.sources[0];
    assert_eq!(
        (
            source.root.as_deref(),
            source.path.as_deref(),
            source.note.as_deref()
        ),
        (Some("root"), Some("kali-root/etc/passwd"), Some("第 3 行"))
    );
    assert!(
        source.node_id.is_some(),
        "the source can be located in the file tree"
    );

    let audit = analysis_query::run_audit(&case_dir, &fixture.image.id, &run_id)
        .expect("audit")
        .expect("run");
    assert_eq!(audit.warnings[0].code, "MALFORMED_LINE");
    assert_eq!(audit.warnings[0].path.as_deref(), Some("etc/passwd"));
    assert_eq!(audit.target.id, fixture.system_id());

    let connection = record_query::open(&case_dir, &fixture.image.id)
        .expect("records")
        .expect("records.db");
    let facts = record_query::profile_facts(&connection, &fixture.system_id()).expect("facts");
    assert_eq!(facts["hostname"], "forensic-box");

    // A second run of the same job replaces the first.
    let again = fixture
        .create(
            "org.test.e2e/users",
            TaskParams {
                system_id: Some(fixture.system_id()),
                ..TaskParams::default()
            },
        )
        .await;
    let again = fixture.finish(&again).await;
    let second_run = again.result.and_then(|result| result.run_id).expect("run");
    assert_ne!(second_run, run_id);
    let runs = record_query::published_runs(&connection).expect("runs");
    assert_eq!(
        runs.iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![second_run.as_str()]
    );
    // The replaced run keeps its audit, without records.
    let replaced = record_query::run_audit(&connection, &run_id)
        .expect("audit")
        .expect("superseded run");
    assert_eq!(replaced.state, "superseded");

    // Deleting the task removes its run.
    fixture.scheduler.remove(again.id).await.expect("remove");
    assert!(record_query::published_runs(&connection)
        .expect("runs")
        .is_empty());
    fixture.scheduler.unbind_case().await;
    fixture.manager.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn evidence_runs_get_their_declared_inputs_under_every_volume() {
    let fixture = Fixture::new().await;
    let task = fixture
        .create("org.test.e2e/hosts", TaskParams::default())
        .await;
    let finished = fixture.finish(&task).await;
    assert_eq!(
        finished.status,
        SchedTaskStatus::Success,
        "{:?}",
        finished.info.error
    );
    let run_id = finished
        .result
        .and_then(|result| result.run_id)
        .expect("run");
    let page = analysis_query::records_page(
        &fixture.case_dir(),
        &AnalysisRecordsQuery {
            image_id: &fixture.image.id,
            run_id: &run_id,
            record_type: "e2e.hostfile",
            legacy: false,
            keyword: Some("FORENSIC"),
            sort: None,
            descending: false,
            skip: 0,
            limit: 10,
        },
    )
    .expect("page");
    assert_eq!(page.total, 1, "keywords match ASCII case-insensitively");
    assert_eq!(page.items[0].values["content"], "forensic-box");
    let audit = analysis_query::run_audit(&fixture.case_dir(), &fixture.image.id, &run_id)
        .expect("audit")
        .expect("run");
    assert_eq!(audit.input_count, 1);
    assert_eq!(audit.inputs[0].path, "root/kali-root/etc/hostname");
    fixture.scheduler.unbind_case().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_beyond_the_old_result_cap_publish_every_record() {
    let fixture = Fixture::new().await;
    let task = fixture
        .create("org.test.e2e/bulk", TaskParams::default())
        .await;
    let finished = fixture.finish(&task).await;
    assert_eq!(
        finished.status,
        SchedTaskStatus::Success,
        "{:?}",
        finished.info.error
    );
    let result = finished.result.expect("result");
    assert_eq!(result.record_count, BULK_RECORDS as u64);
    let run_id = result.run_id.expect("run");
    let last = analysis_query::records_page(
        &fixture.case_dir(),
        &AnalysisRecordsQuery {
            image_id: &fixture.image.id,
            run_id: &run_id,
            record_type: "e2e.line",
            legacy: false,
            keyword: None,
            sort: Some("index"),
            descending: true,
            skip: 0,
            limit: 1,
        },
    )
    .expect("page");
    assert_eq!(last.total, BULK_RECORDS as u64);
    assert_eq!(last.items[0].values["index"], BULK_RECORDS - 1);
    fixture.scheduler.unbind_case().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_run_leaves_no_staging_records() {
    let fixture = Fixture::new().await;
    let task = fixture
        .create(
            "org.test.e2e/users",
            TaskParams {
                system_id: Some(fixture.system_id()),
                ..TaskParams::default()
            },
        )
        .await;
    fixture.scheduler.cancel(task.id).await.expect("cancel");
    fixture.scheduler.dispatch_tick().await;
    assert_eq!(
        fixture.scheduler.get(task.id).expect("task").status,
        SchedTaskStatus::Cancel
    );
    if let Some(connection) =
        record_query::open(&fixture.case_dir(), &fixture.image.id).expect("records")
    {
        let staging: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM run WHERE state = 'staging'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(staging, 0);
    }
    fixture.scheduler.unbind_case().await;
}

/// 端到端:真实的 lc-plugin-host 与 linuxinfo 插件库分析一个 Linux 镜像
/// (需要 `just build-plugins` 的构建产物与 `LOVELYCARVE_TEST_LINUX_IMAGE`
/// 指向的磁盘镜像)。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "set LOVELYCARVE_TEST_LINUX_IMAGE to a Linux disk image to run"]
async fn linuxinfo_analyzes_every_linux_system_of_a_real_image() {
    let plugins_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("plugins");
    let manager = Arc::new(PluginManager::discover(&[plugins_root], None).expect("discover"));
    let image_path = std::env::var("LOVELYCARVE_TEST_LINUX_IMAGE").expect("test image env");
    let image = Arc::new(EvidenceSession::open(Path::new(&image_path)).expect("open image"));
    let systems = Arc::new(SystemCache::default());
    let detected = systems.get_or_discover(&image);
    println!("systems: {:#?}", detected);
    let images = Arc::new(tokio::sync::Mutex::new(HashMap::from([(
        image.id.clone(),
        Arc::clone(&image),
    )])));
    let plugins = Arc::new(RwLock::new(Some(Arc::clone(&manager))));
    let registry_manager = Arc::clone(&manager);
    let runner_systems = Arc::clone(&systems);
    let scheduler = TaskScheduler::new(
        SchedulerConfig {
            dispatch_interval: Duration::from_secs(3600),
            check_interval: Duration::from_secs(3600),
            ..SchedulerConfig::default()
        },
        Arc::new(move || FunctionRegistry::from_definitions(&registry_manager.definitions())),
        Arc::new(move |case_dir: &Path| {
            Arc::new(PluginTaskRunner::new(
                Arc::clone(&images),
                Arc::clone(&plugins),
                Arc::clone(&runner_systems),
                case_dir.to_owned(),
            )) as Arc<dyn TaskRunner>
        }),
    );
    let targets: Vec<SystemTarget> = detected
        .iter()
        .map(|system| SystemTarget {
            id: system.id.clone(),
            family: system.family.clone(),
            label: system.label.clone(),
        })
        .collect();
    let source_id = image.id.clone();
    scheduler.set_systems_source(move |image_id| {
        if image_id == source_id {
            targets.clone()
        } else {
            Vec::new()
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = crate::core::casework::case_ledger::test_support::ledger(dir.path());
    crate::core::casework::case_ledger_evidence::test_support::add(&ledger, &image.id);
    drop(ledger);
    scheduler
        .bind_case(&dir.path().join("case.db"), dir.path().to_owned())
        .await
        .expect("bind case");
    let started = std::time::Instant::now();
    let mut tasks = Vec::new();
    for system in detected.iter().filter(|system| system.family == "linux") {
        let scheduler = scheduler.clone();
        let image_id = image.id.clone();
        let system_id = system.id.clone();
        tasks.push(
            tokio::task::spawn_blocking(move || {
                scheduler.create_task(
                    "org.lovelycarve.linuxinfo/linuxbaseinfo",
                    &image_id,
                    TaskParams {
                        system_id: Some(system_id),
                        ..TaskParams::default()
                    },
                )
            })
            .await
            .expect("thread")
            .expect("create")
            .into_record(),
        );
    }
    assert!(!tasks.is_empty(), "no Linux system was detected");
    let deadline = started + Duration::from_secs(600);
    loop {
        scheduler.dispatch_tick().await;
        if tasks
            .iter()
            .all(|task| scheduler.get(task.id).expect("task").status.is_terminal())
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "tasks still running");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("linuxinfo took {:?}", started.elapsed());
    let connection = record_query::open(dir.path(), &image.id)
        .expect("records")
        .expect("records.db");
    for task in &tasks {
        let finished = scheduler.get(task.id).expect("task");
        assert_eq!(
            finished.status,
            SchedTaskStatus::Success,
            "{:?}",
            finished.info.error
        );
        let result = finished.result.clone().expect("result");
        println!(
            "{} timeline: {} warnings: {}",
            finished.params.target_label.clone().unwrap_or_default(),
            result.timeline_count,
            result.warning_count
        );
        let run_id = result.run_id.expect("run");
        for record_type in record_query::run_record_types(&connection, &run_id).expect("types") {
            println!(
                "{} {}: {}",
                finished.params.target_label.clone().unwrap_or_default(),
                record_type.schema.record_type,
                record_type.record_count
            );
        }
    }
    scheduler.unbind_case().await;
    manager.shutdown().await;
}
