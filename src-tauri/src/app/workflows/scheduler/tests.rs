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
        Arc::new(move || FunctionRegistry::from_definitions(&registry_manager.definitions())),
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
