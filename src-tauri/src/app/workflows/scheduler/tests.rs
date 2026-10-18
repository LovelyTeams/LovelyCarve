use crate::core::extensions::task_scheduler::*;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};
/// 端到端:真实插件经 `PluginTaskRunner` 并行执行三条检材级任务(其中两条
/// 属于同一插件),各自落库并把进度推到 100(需要 `just build-plugins` 的
/// 构建产物与 `LOVELYCARVE_TEST_LINUX_IMAGE` 指向的磁盘镜像)。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "set LOVELYCARVE_TEST_LINUX_IMAGE to a Linux disk image to run"]
async fn plugin_tasks_run_and_persist_side_by_side_on_a_real_image() {
    use std::path::Path;

    use super::PluginTaskRunner;
    use crate::core::{
        analysis::results::plugin_result_store, extensions::plugin::PluginManager,
        filesystem::evidence_session::EvidenceSession,
    };

    let plugins_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("plugins");
    let manager = Arc::new(PluginManager::discover(&[plugins_root]).expect("discover plugins"));
    let image_path = std::env::var("LOVELYCARVE_TEST_LINUX_IMAGE").expect("test image env");
    let image = Arc::new(EvidenceSession::open(Path::new(&image_path)).expect("open image"));
    let images = Arc::new(tokio::sync::Mutex::new(HashMap::from([(
        image.id.clone(),
        Arc::clone(&image),
    )])));
    let plugins = Arc::new(RwLock::new(Some(Arc::clone(&manager))));
    let registry_manager = Arc::clone(&manager);
    let scheduler = TaskScheduler::new(
        SchedulerConfig {
            dispatch_interval: Duration::from_secs(3600),
            check_interval: Duration::from_secs(3600),
            offline_timeout: Duration::from_secs(3600),
            update_timeout: Duration::from_secs(3600),
            post_action_debounce: Duration::from_millis(1),
            ..SchedulerConfig::default()
        },
        Arc::new(move || FunctionRegistry::from_definitions(registry_manager.definitions())),
        Arc::new(move |case_dir: &Path| {
            Arc::new(PluginTaskRunner::new(
                Arc::clone(&images),
                Arc::clone(&plugins),
                case_dir.to_owned(),
            )) as Arc<dyn TaskRunner>
        }),
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = crate::core::casework::case_ledger::test_support::ledger(dir.path());
    crate::core::casework::case_ledger_evidence::test_support::add(&ledger, &image.id);
    drop(ledger);
    let ledger_path = dir.path().join("case.db");
    scheduler
        .bind_case(&ledger_path, dir.path().to_owned())
        .await
        .expect("bind case");

    let tasks = [
        "org.lovelycarve.linuxinfo/linuxbaseinfo",
        "org.lovelycarve.linuxlogs/dpkg.actions",
        "org.lovelycarve.linuxlogs/alternatives",
    ]
    .map(|function| {
        scheduler
            .create_task(function, &image.id, TaskParams::default())
            .expect("create task")
            .into_record()
    });
    let started = std::time::Instant::now();
    scheduler.dispatch_tick().await;
    assert!(
        tasks
            .iter()
            .all(|task| scheduler.get(task.id).expect("task").status != SchedTaskStatus::Created),
        "every task is dispatched in the same round"
    );
    let deadline = started + Duration::from_secs(300);
    loop {
        scheduler.dispatch_tick().await;
        let statuses = tasks
            .iter()
            .map(|task| scheduler.get(task.id).expect("task").status)
            .collect::<Vec<_>>();
        if statuses.iter().all(|status| status.is_terminal()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tasks still {statuses:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("three plugin tasks took {:?}", started.elapsed());

    for task in &tasks {
        let finished = scheduler.get(task.id).expect("task");
        assert_eq!(
            finished.status,
            SchedTaskStatus::Success,
            "{:?}",
            finished.info.error
        );
        assert_eq!(finished.progress, 100);
        let result = finished.result.expect("result");
        println!("{}: {} artifacts", finished.name, result.artifact_count);
        assert!(result.run_id.is_some());
    }
    let runs = plugin_result_store::list_for_node(dir.path(), &image.id, "").expect("runs");
    assert_eq!(runs.len(), tasks.len());
    scheduler.unbind_case().await;
    manager.shutdown().await;
}

#[test]
fn prepared_outputs_publish_and_delete_only_the_task_run() {
    use super::{Analysis, PluginTaskRunner};
    use crate::core::{
        analysis::results::{
            models::{PluginAnalysisResult, PluginAnalysisTimelineEvent},
            plugin_result_store,
        },
        casework::{
            case_ledger, case_ledger_evidence,
            case_paths::{self, EvidenceDb},
        },
        extensions::plugin::TaskTrigger,
        timeline::timeline_store,
    };
    let directory = tempfile::tempdir().expect("case");
    let ledger = case_ledger::test_support::ledger(directory.path());
    let evidence = case_ledger_evidence::test_support::add(&ledger, "image");
    let store = TaskStore::open(&directory.path().join("case.db")).expect("store");
    let mut task = store
        .insert_task(&NewTask {
            name: "org.test/shared".into(),
            display_name: "Shared".into(),
            type_name: "analysis".into(),
            evidence_id: evidence.id,
            plugin_id: "org.test".into(),
            analyzer: "shared".into(),
            trigger: TaskTrigger::Manually,
            parent_tid: None,
            params: TaskParams::default(),
            dependence: String::new(),
            can_pause: false,
            single_instance: false,
            internal: false,
        })
        .expect("task")
        .into_record();
    let runner = PluginTaskRunner::new(Arc::default(), Arc::default(), directory.path().into());
    let prepare = || Analysis {
        result: PluginAnalysisResult {
            timeline: vec![PluginAnalysisTimelineEvent {
                timestamp: "2026-10-18T00:00:00Z".into(),
                event_type: "demo.event".into(),
                description: "event".into(),
                attributes: HashMap::new(),
            }],
            ..PluginAnalysisResult::default()
        },
        plugin_version: "1".into(),
        node_id: String::new(),
        file_name: String::new(),
        started_at: "2026-10-18T00:00:00Z".into(),
        completed_at: "2026-10-18T00:00:01Z".into(),
    };
    runner.prepared.lock().insert((task.id, 1), prepare());
    runner.discard_execution(task.id, 1);
    assert!(
        !case_paths::evidence_db_path(directory.path(), "image", EvidenceDb::Analysis).exists()
    );
    let timeline = timeline_store::open_writer(directory.path(), "image").expect("timeline");
    timeline_store::meta_begin(&timeline, 0).expect("begin");
    let mut meta = timeline_store::meta_get(&timeline)
        .expect("meta")
        .expect("meta");
    meta.status = "success".into();
    timeline_store::meta_finish(&timeline, &meta, 0).expect("ready");
    drop(timeline);
    runner.prepared.lock().insert((task.id, 2), prepare());
    task.result = Some(
        runner
            .publish_results(&task, 2, TaskResult::default())
            .expect("first run"),
    );
    runner.prepared.lock().insert((task.id, 3), prepare());
    let later = runner
        .publish_results(&task, 3, TaskResult::default())
        .expect("later run");
    runner.remove_results(&task).expect("delete first run");
    let runs = plugin_result_store::list_for_node(directory.path(), "image", "").expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(Some(runs[0].run_id.clone()), later.run_id);
    let timeline = timeline_store::open_reader(directory.path(), "image")
        .expect("open")
        .expect("timeline");
    let projected: Vec<String> = timeline
        .prepare("SELECT tid FROM timeline")
        .expect("statement")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(projected, vec![runs[0].run_id.clone()]);
    task.result = None;
    runner
        .remove_results(&task)
        .expect("unpublished task has nothing to delete");
    assert_eq!(
        plugin_result_store::list_for_node(directory.path(), "image", "")
            .expect("remaining")
            .len(),
        1
    );
}
