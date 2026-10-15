//! Task lifecycle, cancellation and resource regressions.

use super::*;

#[test]
fn case_exit_cancels_queued_and_running_loads_without_an_image_identity() {
    let manager = TaskManager::default();
    let first = manager.register("load-a", TaskKind::ImageLoad, None);
    let second = manager.register("load-b", TaskKind::ImageLoad, None);
    let unrelated = manager.register("export", TaskKind::ImageExport, Some("disk".into()));
    assert_eq!(manager.cancel_kind(TaskKind::ImageLoad), 2);
    assert!(first.token().is_cancelled());
    assert!(second.token().is_cancelled());
    assert!(!unrelated.token().is_cancelled());
}
use std::time::Duration;

#[test]
fn dropping_registration_removes_task() {
    let manager = TaskManager::default();
    let registration = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    assert!(manager.contains("hash-1"));
    assert_eq!(registration.record().status(), TaskStatus::Queued);

    drop(registration);

    assert!(!manager.contains("hash-1"));
}

#[test]
fn repeated_registration_is_reference_counted() {
    let manager = TaskManager::default();
    let first = manager.register("snapshot-1", TaskKind::VolumeSnapshot, None);
    let second = manager.register("snapshot-1", TaskKind::VolumeSnapshot, None);
    assert_eq!(first.token().is_cancelled(), second.token().is_cancelled());

    drop(first);
    assert!(manager.contains("snapshot-1"));
    drop(second);
    assert!(!manager.contains("snapshot-1"));
}

#[test]
fn register_if_absent_rejects_duplicate() {
    let manager = TaskManager::default();
    let Some(first) = manager.register_if_absent("snapshot-1", TaskKind::VolumeSnapshot, None)
    else {
        panic!("first registration should succeed");
    };

    assert!(manager
        .register_if_absent("snapshot-1", TaskKind::VolumeSnapshot, None)
        .is_none());
    drop(first);
    assert!(manager
        .register_if_absent("snapshot-1", TaskKind::VolumeSnapshot, None)
        .is_some());
}

#[test]
fn cancellation_does_not_let_old_guard_remove_reused_id() {
    let manager = TaskManager::default();
    let old = manager.register("export-1", TaskKind::ImageExport, None);
    let old_token = old.token();
    assert!(manager.cancel("export-1"));
    assert!(old_token.is_cancelled());

    let replacement = manager.register("export-1", TaskKind::ImageExport, None);
    drop(old);

    assert!(manager.contains("export-1"));
    assert!(!replacement.token().is_cancelled());
}

#[test]
fn cancel_image_only_cancels_owned_tasks() {
    let manager = TaskManager::default();
    let image_one_hash =
        manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let image_one_snapshot = manager.register(
        "snapshot-1",
        TaskKind::SessionSnapshot,
        Some("image-1".to_string()),
    );
    let image_two_hash =
        manager.register("hash-2", TaskKind::ImageHash, Some("image-2".to_string()));

    assert_eq!(manager.cancel_image("image-1"), 2);
    assert!(image_one_hash.token().is_cancelled());
    assert!(image_one_snapshot.token().is_cancelled());
    assert!(!image_two_hash.token().is_cancelled());
    assert!(manager.contains("hash-2"));
}

#[test]
fn cancel_image_kind_preserves_other_work() {
    let manager = TaskManager::default();
    let snapshot = manager.register(
        "snapshot-1",
        TaskKind::SessionSnapshot,
        Some("image-1".to_string()),
    );
    let export = manager.register(
        "export-1",
        TaskKind::ImageExport,
        Some("image-1".to_string()),
    );

    assert_eq!(
        manager.cancel_image_kind("image-1", TaskKind::SessionSnapshot),
        1
    );
    assert!(snapshot.token().is_cancelled());
    assert!(!export.token().is_cancelled());
    assert!(manager.contains("export-1"));
}

#[test]
fn status_and_list_return_active_task_snapshots() {
    let manager = TaskManager::default();
    let first = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let _second = manager.register("scan-1", TaskKind::BitlockerScan, None);

    let Some(status) = manager.status("hash-1") else {
        panic!("hash-1 should be visible");
    };
    assert_eq!(status.id, "hash-1");
    assert_eq!(status.kind, TaskKind::ImageHash);
    assert_eq!(status.image_id.as_deref(), Some("image-1"));
    assert_eq!(status.status, TaskStatus::Queued);
    assert_eq!(
        status.resource_classes,
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageSequentialIo,
        ]
    );

    let tasks = manager.list();
    assert_eq!(tasks.len(), 2);

    drop(first);
    assert!(manager.status("missing").is_none());
}

#[test]
fn lifecycle_sink_observes_queued_and_finished() {
    let manager = TaskManager::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    manager.set_event_sink(move |event| {
        sink_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    });

    let registration = manager.register(
        "snapshot-1",
        TaskKind::VolumeSnapshot,
        Some("image-1".to_string()),
    );
    drop(registration);

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, TaskLifecyclePhase::Queued);
    assert_eq!(events[0].task.id, "snapshot-1");
    assert_eq!(events[0].task.status, TaskStatus::Queued);
    assert_eq!(events[1].phase, TaskLifecyclePhase::Finished);
    assert_eq!(events[1].task.id, "snapshot-1");
}

#[test]
fn lifecycle_sink_observes_cancellation_requested() {
    let manager = TaskManager::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    manager.set_event_sink(move |event| {
        sink_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    });

    let registration = manager.register("hash-1", TaskKind::ImageHash, None);
    assert!(manager.cancel("hash-1"));
    drop(registration);

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, TaskLifecyclePhase::Queued);
    assert_eq!(events[1].phase, TaskLifecyclePhase::CancellationRequested);
    assert_eq!(events[1].task.status, TaskStatus::CancellationRequested);
}

#[test]
fn progress_sink_observes_active_task_snapshot() {
    let manager = TaskManager::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    manager.set_progress_sink(move |event| {
        sink_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    });

    let registration = manager.register("hash-1", TaskKind::ImageHash, None);

    assert!(manager.emit_progress(
        "hash-1",
        TaskProgressUnit::Bytes,
        16,
        Some(64),
        Some("chunk".to_string()),
    ));
    assert!(!manager.emit_progress("missing", TaskProgressUnit::Entries, 1, None, None));

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].task.id, "hash-1");
    assert_eq!(events[0].unit, TaskProgressUnit::Bytes);
    assert_eq!(events[0].done, 16);
    assert_eq!(events[0].total, Some(64));
    assert_eq!(events[0].message.as_deref(), Some("chunk"));

    drop(registration);
}

#[tokio::test]
async fn acquired_resources_mark_task_running() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let task = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));

    let permit = manager.acquire_resources(&task).await;

    assert!(permit.is_ok());
    assert_eq!(task.record().status(), TaskStatus::Running);
}

#[tokio::test]
async fn lifecycle_sink_observes_running_after_resource_acquire() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    manager.set_event_sink(move |event| {
        sink_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    });

    let task = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let permit = manager.acquire_resources(&task).await;

    assert!(permit.is_ok());
    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, TaskLifecyclePhase::Queued);
    assert_eq!(events[1].phase, TaskLifecyclePhase::Running);
    assert_eq!(events[1].task.status, TaskStatus::Running);
    assert_eq!(
        events[1].task.resource_classes,
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageSequentialIo,
        ]
    );
}

#[test]
fn resource_classes_describe_task_kind_lanes() {
    assert_eq!(
        TaskKind::ImageHash.resource_classes(),
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageSequentialIo,
        ]
    );
    assert_eq!(
        TaskKind::QrScan.resource_classes(),
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageRandomIo,
            TaskResourceClass::Cpu,
        ]
    );
    assert_eq!(
        TaskKind::GlobalSearch.resource_classes(),
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageRandomIo,
            TaskResourceClass::Cpu,
        ]
    );

    #[cfg(feature = "ocr-cuda")]
    assert_eq!(
        TaskKind::OcrScan.resource_classes(),
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageRandomIo,
            TaskResourceClass::Gpu,
        ]
    );

    #[cfg(not(feature = "ocr-cuda"))]
    assert_eq!(
        TaskKind::OcrScan.resource_classes(),
        vec![
            TaskResourceClass::GlobalBackground,
            TaskResourceClass::ImageRandomIo,
            TaskResourceClass::Cpu,
        ]
    );
}

#[tokio::test]
async fn global_background_limit_blocks_excess_tasks() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let first = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let first_permit = manager.acquire_resources(&first).await;
    assert!(first_permit.is_ok());

    let second = manager.register(
        "scan-1",
        TaskKind::BitlockerScan,
        Some("image-2".to_string()),
    );
    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&second).await });
    tokio::task::yield_now().await;

    assert!(!waiter.is_finished());
    drop(first_permit);

    let second_permit = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(second_permit, Ok(Ok(_))));
}

#[tokio::test]
async fn cpu_limit_blocks_excess_cpu_tasks() {
    let manager = TaskManager::with_all_resource_limits(2, 1, 2, 1, 1);
    let first = manager.register("scan-1", TaskKind::BitlockerScan, None);
    let first_permit = manager.acquire_resources(&first).await;
    assert!(first_permit.is_ok());

    let second = manager.register("scan-2", TaskKind::BitlockerScan, None);
    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&second).await });
    tokio::task::yield_now().await;

    assert!(!waiter.is_finished());
    drop(first_permit);

    let second_permit = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(second_permit, Ok(Ok(_))));
}

#[tokio::test]
async fn same_image_sequential_io_is_serial() {
    let manager = TaskManager::with_resource_limits(2, 1);
    let first = manager.register(
        "export-1",
        TaskKind::ImageExport,
        Some("image-1".to_string()),
    );
    let first_permit = manager.acquire_resources(&first).await;
    assert!(first_permit.is_ok());

    let second = manager.register(
        "export-2",
        TaskKind::ImageExport,
        Some("image-1".to_string()),
    );
    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&second).await });
    tokio::task::yield_now().await;

    assert!(!waiter.is_finished());
    drop(first_permit);

    let second_permit = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(second_permit, Ok(Ok(_))));
}

#[tokio::test]
async fn same_image_random_io_has_independent_limit() {
    let manager = TaskManager::with_all_resource_limits(3, 1, 2, 2, 1);
    let first = manager.register("flat-1", TaskKind::FlatList, Some("image-1".to_string()));
    let second = manager.register("flat-2", TaskKind::FlatList, Some("image-1".to_string()));
    let third = manager.register("flat-3", TaskKind::FlatList, Some("image-1".to_string()));

    let first_permit = manager.acquire_resources(&first).await;
    let second_permit = manager.acquire_resources(&second).await;
    assert!(first_permit.is_ok());
    assert!(second_permit.is_ok());

    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&third).await });
    tokio::task::yield_now().await;

    assert!(!waiter.is_finished());
    drop(first_permit);

    let third_permit = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(third_permit, Ok(Ok(_))));
}

#[tokio::test]
async fn different_images_can_acquire_until_global_limit() {
    let manager = TaskManager::with_resource_limits(2, 1);
    let first = manager.register(
        "export-1",
        TaskKind::ImageExport,
        Some("image-1".to_string()),
    );
    let second = manager.register(
        "export-2",
        TaskKind::ImageExport,
        Some("image-2".to_string()),
    );

    let first_permit = manager.acquire_resources(&first).await;
    let second_permit = manager.acquire_resources(&second).await;

    assert!(first_permit.is_ok());
    assert!(second_permit.is_ok());
}

#[tokio::test]
async fn waiting_resource_acquisition_observes_cancellation() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let first = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let first_permit = manager.acquire_resources(&first).await;
    assert!(first_permit.is_ok());

    let second = manager.register("hash-2", TaskKind::ImageHash, Some("image-2".to_string()));
    let second_token = second.token();
    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&second).await });
    tokio::task::yield_now().await;

    assert!(!waiter.is_finished());
    second_token.cancel();

    let result = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(result, Ok(Err(_))));
    drop(first_permit);
}

#[tokio::test]
async fn unbudgeted_tasks_run_while_background_permits_are_taken() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let hash = manager.register("hash-1", TaskKind::ImageHash, Some("image-1".to_string()));
    let hash_permit = manager.acquire_resources(&hash).await;
    assert!(hash_permit.is_ok());

    let detect = manager.register(
        "autodetect:image-1",
        TaskKind::AutoDetect,
        Some("image-1".to_string()),
    );
    let detect_permit =
        tokio::time::timeout(Duration::from_secs(5), manager.acquire_resources(&detect)).await;

    assert!(matches!(detect_permit, Ok(Ok(_))));
    assert_eq!(detect.record().status(), TaskStatus::Running);
    assert!(TaskKind::AutoDetect.resource_classes().is_empty());
}

#[tokio::test]
async fn permit_drop_wakes_waiting_task() {
    let manager = TaskManager::with_resource_limits(1, 1);
    let first = manager.register(
        "snapshot-1",
        TaskKind::VolumeSnapshot,
        Some("image-1".to_string()),
    );
    let first_permit = manager.acquire_resources(&first).await;
    assert!(first_permit.is_ok());

    let second = manager.register(
        "snapshot-2",
        TaskKind::VolumeSnapshot,
        Some("image-2".to_string()),
    );
    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move { waiter_manager.acquire_resources(&second).await });
    tokio::task::yield_now().await;

    drop(first_permit);

    let second_permit = waiter
        .await
        .map_err(|err| format!("waiter task crashed: {err}"));
    assert!(matches!(second_permit, Ok(Ok(_))));
}
