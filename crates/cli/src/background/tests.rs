use std::time::Duration;

use serde_json::{json, Value};

use super::*;
use crate::{
    app::workflows::task_center::{self, TaskEntryEvent},
    core::extensions::{
        plugin::TaskTrigger,
        task_scheduler::{SchedTaskRecord, TaskInfo, TaskParams},
    },
    task_manager::{TaskKind, TaskManager, TaskOutcome},
};

/// Feeds the task center's entries of `tasks` to `background`, as the CLI
/// host does with `task-entry` events.
fn follow(tasks: &TaskManager, background: &Background) {
    let background = background.clone();
    tasks.set_event_sink(move |event| {
        if let Some(entry) = task_center::native_entry(&event.task) {
            let payload = serde_json::to_value(TaskEntryEvent::changed(entry)).unwrap();
            background.observe(TASK_ENTRY_EVENT, &payload);
        }
    });
}

fn statuses(report: &Report) -> Vec<(&str, SchedTaskStatus)> {
    report
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.status))
        .collect()
}

fn job(id: i64, status: SchedTaskStatus, trigger: TaskTrigger, error: Option<&str>) -> Value {
    let record = SchedTaskRecord {
        id,
        name: "org.example/analyzer".into(),
        display_name: "Analyzer".into(),
        type_name: "system".into(),
        evidence_id: 1,
        progress: 0,
        params: TaskParams {
            target_label: Some("linux@part:2".into()),
            ..TaskParams::default()
        },
        result: None,
        dependence: String::new(),
        status,
        info: TaskInfo {
            plugin_id: "org.example".into(),
            analyzer: "analyzer".into(),
            image_id: "image".into(),
            trigger,
            parent_tid: None,
            error: error.map(str::to_owned),
        },
        can_pause: false,
        single_instance: true,
        internal: false,
        spend_time_secs: None,
        remain_time_secs: None,
        start_at: None,
        create_at: "2026-10-26T00:00:00Z".into(),
        update_at: "2026-10-26T00:00:00Z".into(),
    };
    serde_json::to_value(TaskEntryEvent::changed(task_center::job_entry(record))).unwrap()
}

/// The outcome of a foreground command.
fn ended(succeeded: bool) -> Result<(), CliError> {
    if succeeded {
        Ok(())
    } else {
        Err(CliError::invalid("failed"))
    }
}

#[test]
fn a_task_that_outlives_its_command_fails_the_next_wait() {
    let tasks = TaskManager::default();
    let background = Background::default();
    follow(&tasks, &background);
    let foreground = background.foreground();
    let registration = tasks.register("index-1", TaskKind::VolumeSnapshot, Some("image".into()));
    registration.update(|view| view.subject = vec!["part:0".into()]);
    foreground.finish(&ended(true));
    assert_eq!(background.take(), Report::default(), "still running");
    registration.finish(TaskOutcome::Failed {
        error: "walk failed".into(),
    });
    drop(registration);
    let report = background.take();
    assert_eq!(
        report.tasks,
        vec![Ended {
            id: "index-1".into(),
            kind: "volumeSnapshot".into(),
            subject: vec!["part:0".into()],
            status: SchedTaskStatus::Failed,
            error: Some("walk failed".into()),
        }]
    );
    let error = report.clone().verdict(None).unwrap_err();
    assert_eq!(error.code, "tasksFailed");
    assert_eq!(
        error.message,
        "1 个后台任务失败：\n- volumeSnapshot part:0（index-1）：walk failed"
    );
    assert_eq!(error.details, Some(serde_json::to_value(&report).unwrap()));
    assert_eq!(background.take(), Report::default(), "reported once");
}

#[test]
fn a_command_reports_its_own_tasks_unless_it_passed_over_a_failure() {
    let tasks = TaskManager::default();
    let background = Background::default();
    follow(&tasks, &background);
    let run = |id: &str, outcome: TaskOutcome, succeeded: bool| {
        let foreground = background.foreground();
        let registration = tasks.register(id, TaskKind::ImageHash, Some("image".into()));
        registration.finish(outcome);
        drop(registration);
        foreground.finish(&ended(succeeded));
    };
    // The command's error is its task's failure.
    run(
        "hash-1",
        TaskOutcome::Failed {
            error: "short read".into(),
        },
        false,
    );
    // The command's result is its task's success.
    run("hash-2", TaskOutcome::Succeeded, true);
    assert_eq!(background.take(), Report::default());
    // No result carried this failure, so the wait does.
    run(
        "hash-3",
        TaskOutcome::Failed {
            error: "disk full".into(),
        },
        true,
    );
    assert_eq!(
        statuses(&background.take()),
        [("hash-3", SchedTaskStatus::Failed)]
    );
    // A command dropped unfinished counts as failed.
    let foreground = background.foreground();
    tasks
        .register("hash-4", TaskKind::ImageHash, Some("image".into()))
        .finish(TaskOutcome::Failed {
            error: "interrupted".into(),
        });
    drop(foreground);
    assert_eq!(background.take(), Report::default());
}

#[test]
fn automatic_tasks_are_background_work_even_inside_a_failed_command() {
    let tasks = TaskManager::default();
    let background = Background::default();
    follow(&tasks, &background);
    let foreground = background.foreground();
    let registration = tasks.register(
        "autodetect:image",
        TaskKind::AutoDetect,
        Some("image".into()),
    );
    registration.set_automatic();
    registration.finish(TaskOutcome::Succeeded);
    drop(registration);
    foreground.finish(&ended(false));
    assert_eq!(
        statuses(&background.take()),
        [("autodetect:image", SchedTaskStatus::Success)]
    );
}

#[test]
fn cancellations_are_reported_and_speculative_indexes_are_ignored() {
    let tasks = TaskManager::default();
    let background = Background::default();
    follow(&tasks, &background);
    let export = tasks.register("export-2", TaskKind::DirectoryExport, Some("image".into()));
    let snapshot = tasks.register(
        "session-snapshot",
        TaskKind::SessionSnapshot,
        Some("image".into()),
    );
    snapshot.finish(TaskOutcome::Failed {
        error: "unsupported".into(),
    });
    drop(snapshot);
    assert!(tasks.cancel("export-2"));
    drop(export);
    let report = background.take();
    assert_eq!(statuses(&report), [("export-2", SchedTaskStatus::Cancel)]);
    assert_eq!(report.clone().verdict(None).unwrap(), report);
}

#[test]
fn plugin_jobs_count_once_they_ran_here_and_offline_is_not_an_outcome() {
    use SchedTaskStatus::{Created, Failed, Offline, Running};
    let background = Background::default();
    let observe = |payload: Value| background.observe(TASK_ENTRY_EVENT, &payload);
    // A job of an earlier process, marked offline when the case opened.
    observe(job(
        1,
        Offline,
        TaskTrigger::Manually,
        Some("receive request offline"),
    ));
    observe(job(
        1,
        Failed,
        TaskTrigger::Manually,
        Some("receive request offline"),
    ));
    // A job that runs here, drops offline and is queued again.
    observe(job(2, Created, TaskTrigger::Auto, None));
    observe(job(2, Running, TaskTrigger::Auto, None));
    observe(job(2, Offline, TaskTrigger::Auto, Some("worker offline")));
    assert_eq!(background.take(), Report::default());
    observe(job(2, Created, TaskTrigger::Auto, None));
    observe(job(2, Failed, TaskTrigger::Auto, Some("plugin crashed")));
    // Scheduling notices are not task changes.
    observe(json!({"kind": "notice", "notice": {"kind": "allTasksFinished"}}));
    assert_eq!(
        background.take().tasks,
        vec![Ended {
            id: "plugin:2".into(),
            kind: "plugin".into(),
            subject: vec!["org.example/analyzer".into(), "linux@part:2".into()],
            status: Failed,
            error: Some("plugin crashed".into()),
        }]
    );
    // A job deleted after it failed is not reported.
    observe(job(3, Running, TaskTrigger::Manually, None));
    observe(job(3, Failed, TaskTrigger::Manually, Some("bad input")));
    observe(
        serde_json::to_value(TaskEntryEvent::Removed {
            id: "plugin:3".into(),
        })
        .unwrap(),
    );
    assert_eq!(background.take(), Report::default());
}

#[test]
fn a_task_whose_end_was_never_reported_is_not_taken_for_a_success() {
    let tasks = TaskManager::default();
    let background = Background::default();
    follow(&tasks, &background);
    let registration = tasks.register("scan-1", TaskKind::ImageExport, Some("image".into()));
    background.lock().abandon_running();
    let report = background.take();
    assert_eq!(statuses(&report), [("scan-1", SchedTaskStatus::Offline)]);
    assert_eq!(
        report.verdict(None).unwrap_err().message,
        "1 个后台任务失败：\n- imageExport（scan-1）：任务已结束，但没有报告结果"
    );
    drop(registration);
    assert_eq!(background.take(), Report::default());
}

#[tokio::test]
async fn wait_returns_when_nothing_runs_with_what_ended_meanwhile() {
    let state = AppState::new();
    let background = Background::default();
    follow(&state.tasks, &background);
    let registration =
        state
            .tasks
            .register("index-2", TaskKind::VolumeSnapshot, Some("image".into()));
    let worker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        registration.finish(TaskOutcome::Succeeded);
    });
    let report = background.wait(&state).await.unwrap();
    worker.await.unwrap();
    assert_eq!(statuses(&report), [("index-2", SchedTaskStatus::Success)]);
    let lead = Report {
        tasks: vec![Ended {
            status: SchedTaskStatus::Failed,
            error: None,
            ..report.tasks[0].clone()
        }],
    }
    .verdict(Some("会话已关闭"))
    .unwrap_err();
    assert_eq!(
        lead.message,
        "会话已关闭；1 个后台任务失败：\n- volumeSnapshot（index-2）：失败"
    );
}
