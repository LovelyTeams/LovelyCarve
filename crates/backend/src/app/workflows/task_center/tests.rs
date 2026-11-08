//! How each kind of task becomes a task center entry.

use super::*;
use crate::core::carving::carve_session::CarveScanSession;
use crate::core::extensions::task_scheduler::{TaskInfo, TaskParams};
use crate::core::search::global_search_store::test_support::sample_meta;
use crate::task_manager::TaskFailure;

struct Failure;

impl TaskFailure for Failure {
    fn is_cancellation(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        "short read".to_owned()
    }
}

fn entry_of(tasks: &TaskManager, id: &str) -> Option<TaskEntry> {
    tasks
        .list()
        .iter()
        .chain(&tasks.finished())
        .find(|task| task.id == id)
        .and_then(native_entry)
}

#[test]
fn native_tasks_carry_their_status_outcome_and_actions() {
    let tasks = TaskManager::default();
    let hash = tasks.register("hash-1", TaskKind::ImageHash, Some("image".into()));
    hash.update(|view| view.subject = vec!["MD5".to_owned()]);
    let queued = entry_of(&tasks, "hash-1").expect("listed");
    assert_eq!(
        (queued.status, queued.actions.as_slice(), queued.cancelling),
        (
            SchedTaskStatus::Queued,
            [TaskAction::Cancel].as_slice(),
            false
        )
    );
    hash.finish_with(&Err::<(), _>(Failure));
    drop(hash);
    let failed = entry_of(&tasks, "hash-1").expect("kept");
    assert_eq!(failed.status, SchedTaskStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("short read"));
    assert!(failed.actions.is_empty());
    assert!(failed.finished_at_ms.is_some());

    let export = tasks.register("export-1", TaskKind::ImageExport, Some("image".into()));
    tasks.cancel("export-1");
    let stopping = entry_of(&tasks, "export-1").expect("stopping");
    assert_eq!(
        (stopping.status, stopping.cancelling, stopping.actions.len()),
        (SchedTaskStatus::Running, true, 0)
    );
    drop(export);
    assert_eq!(
        entry_of(&tasks, "export-1").expect("ended").status,
        SchedTaskStatus::Cancel
    );

    drop(tasks.register("load-1", TaskKind::ImageLoad, None));
    assert_eq!(
        entry_of(&tasks, "load-1").expect("ended").status,
        SchedTaskStatus::Offline,
        "a task that reported no outcome went away"
    );
    let docker = tasks.register("docker-1", TaskKind::DockerPrepare, Some("image".into()));
    assert!(entry_of(&tasks, "docker-1")
        .expect("listed")
        .actions
        .is_empty());
    drop(docker);

    // Hidden kinds, and carving scans (listed by their sessions), are not
    // entries of their own.
    let flat = tasks.register("flat-1", TaskKind::FlatList, Some("image".into()));
    let carve = tasks.register("carve-1", TaskKind::CarveScan, Some("image".into()));
    assert!(entry_of(&tasks, "flat-1").is_none());
    assert!(entry_of(&tasks, "carve-1").is_none());
    drop((flat, carve));
}

fn job(status: SchedTaskStatus, trigger: TaskTrigger) -> SchedTaskRecord {
    SchedTaskRecord {
        id: 7,
        name: "org.lovelycarve.linuxinfo/linuxbaseinfo".to_owned(),
        display_name: "Linux 基本信息".to_owned(),
        type_name: "baseinfo".to_owned(),
        evidence_id: 1,
        progress: 40,
        params: TaskParams::default(),
        result: None,
        dependence: String::new(),
        status,
        info: TaskInfo {
            plugin_id: "org.lovelycarve.linuxinfo".to_owned(),
            analyzer: "linuxbaseinfo".to_owned(),
            image_id: "image".to_owned(),
            trigger,
            parent_tid: None,
            error: None,
        },
        can_pause: false,
        single_instance: true,
        internal: false,
        spend_time_secs: Some(3),
        remain_time_secs: None,
        start_at: Some("2026-10-19T08:00:01.000Z".to_owned()),
        create_at: "2026-10-19T08:00:00.000Z".to_owned(),
        update_at: "2026-10-19T08:00:04.000Z".to_owned(),
    }
}

#[test]
fn plugin_jobs_are_entries_with_the_actions_their_status_allows() {
    let running = job_entry(job(SchedTaskStatus::Running, TaskTrigger::Auto));
    assert_eq!(running.id, "plugin:7");
    assert_eq!(running.actions, vec![TaskAction::Cancel]);
    assert!(running.automatic);
    assert_eq!(running.created_at_ms, 1_792_396_800_000);
    assert_eq!(running.started_at_ms, Some(1_792_396_801_000));
    assert_eq!(running.finished_at_ms, None);
    assert_eq!(
        running.progress,
        Some(TaskAmount {
            unit: TaskProgressUnit::Percent,
            done: 40,
            total: Some(100),
        })
    );

    let failed = job_entry(job(SchedTaskStatus::Failed, TaskTrigger::Manually));
    assert_eq!(
        failed.actions,
        vec![TaskAction::Restart, TaskAction::Remove]
    );
    assert!(!failed.automatic);
    assert_eq!(failed.finished_at_ms, Some(1_792_396_804_000));
    let offline = job_entry(job(SchedTaskStatus::Offline, TaskTrigger::Auto));
    assert_eq!(
        offline.actions,
        vec![TaskAction::Cancel, TaskAction::Start, TaskAction::Restart]
    );
    // Older rows wrote naive UTC times.
    let older = job_entry(SchedTaskRecord {
        create_at: "2026-10-19 08:00:00".to_owned(),
        ..job(SchedTaskStatus::Created, TaskTrigger::Manually)
    });
    assert_eq!(older.created_at_ms, 1_792_396_800_000);
}

#[test]
fn carving_sessions_are_entries_with_pause_and_resume() {
    let mut session = CarveScanSession::new(
        "scan-1".into(),
        "task-1".into(),
        "image".into(),
        "整个镜像".into(),
        4096,
    );
    session.status = CarveSessionStatus::Running;
    session.bytes_scanned = 1024;
    let running = carve_entry(&session.snapshot(), false);
    assert_eq!(running.id, "task-1");
    assert_eq!(running.actions, vec![TaskAction::Cancel, TaskAction::Pause]);
    assert_eq!(
        running.progress,
        Some(TaskAmount {
            unit: TaskProgressUnit::Bytes,
            done: 1024,
            total: Some(4096),
        })
    );
    let TaskSource::Builtin {
        kind,
        view,
        scan_id,
    } = &running.source
    else {
        panic!("a carving scan is a native task");
    };
    assert_eq!(*kind, TaskKind::CarveScan);
    assert_eq!(scan_id.as_deref(), Some("scan-1"));
    assert_eq!(view.stage, Some(TaskStage::Signature));
    assert_eq!(view.counts.get(&TaskCount::Found), Some(&0));

    session.status = CarveSessionStatus::Paused;
    let paused = carve_entry(&session.snapshot(), false);
    assert_eq!(
        (paused.status, paused.actions),
        (
            SchedTaskStatus::Pause,
            vec![TaskAction::Cancel, TaskAction::Resume]
        )
    );
    let stopping = carve_entry(&session.snapshot(), true);
    assert!(stopping.cancelling && stopping.actions.is_empty());

    session.truncated = true;
    session.finish(CarveSessionStatus::Completed, None);
    let completed = carve_entry(&session.snapshot(), true);
    assert_eq!(completed.status, SchedTaskStatus::Success);
    assert!(!completed.cancelling && completed.actions.is_empty());
    assert!(completed.finished_at_ms.is_some());
    let TaskSource::Builtin { view, .. } = &completed.source else {
        panic!("a carving scan is a native task");
    };
    assert!(view.flags.contains(&TaskFlag::Truncated));
}

#[test]
fn stored_searches_and_timeline_builds_are_entries() {
    let mut meta = sample_meta("search-1", "cancelled");
    meta.total_matches = 12;
    meta.matched_file_count = 3;
    meta.files_scanned = 4;
    let search = search_entry(meta);
    assert_eq!(search.status, SchedTaskStatus::Cancel);
    let TaskSource::Builtin { kind, view, .. } = &search.source else {
        panic!("a search is a native task");
    };
    assert_eq!(*kind, TaskKind::GlobalSearch);
    assert_eq!(
        view.subject,
        vec!["password".to_owned(), r"\d{4}".to_owned()]
    );
    assert_eq!(
        (
            view.counts.get(&TaskCount::Hits),
            view.counts.get(&TaskCount::MatchedFiles)
        ),
        (Some(&12), Some(&3))
    );
    assert_eq!(stored_status("failed"), SchedTaskStatus::Failed);

    let timeline = timeline_entry(
        "image",
        TimelineMetaRow {
            status: "success".to_owned(),
            error: None,
            started_ms: 10,
            finished_ms: Some(20),
            file_rows: 5,
            event_rows: 2,
            source_file_watermark: String::new(),
            source_event_watermark: String::new(),
        },
    );
    assert_eq!(timeline.id, timeline_task_id("image"));
    assert_eq!(
        (timeline.status, timeline.finished_at_ms),
        (SchedTaskStatus::Success, Some(20))
    );
}

#[test]
fn entries_name_their_source_on_the_wire() {
    let tasks = TaskManager::default();
    let hash = tasks.register("hash-1", TaskKind::ImageHash, Some("image".into()));
    let entry = entry_of(&tasks, "hash-1").expect("listed");
    let wire = serde_json::to_value(TaskEntryEvent::changed(entry)).expect("serialize");
    assert_eq!(wire["kind"], "changed");
    assert_eq!(wire["entry"]["source"], "builtin");
    assert_eq!(wire["entry"]["kind"], "imageHash");
    assert_eq!(wire["entry"]["status"], "queued");
    assert_eq!(wire["entry"]["actions"], serde_json::json!(["cancel"]));
    assert!(wire["entry"]["view"]["counts"].is_object());
    drop(hash);

    let plugin = serde_json::to_value(job_entry(job(
        SchedTaskStatus::Success,
        TaskTrigger::Manually,
    )))
    .expect("serialize");
    assert_eq!(plugin["source"], "plugin");
    assert_eq!(plugin["job"]["id"], 7);
    assert_eq!(plugin["progress"]["unit"], "percent");

    let carve = CarveScanSession::new(
        "scan-1".into(),
        "task-1".into(),
        "image".into(),
        "整个镜像".into(),
        4096,
    )
    .snapshot();
    let carve = serde_json::to_value(carve_entry(&carve, false)).expect("serialize");
    assert_eq!(carve["scanId"], "scan-1");
    assert_eq!(carve["createdAtMs"], carve["startedAtMs"]);
}
