//! progress implementation.
use super::*;

pub(crate) fn emit_progress(app: &AppHandle, payload: SnapshotProgressPayload) {
    if let Err(err) = app.emit(SNAPSHOT_PROGRESS_EVENT, payload) {
        tracing::debug!("case: emit snapshot progress failed: {err}");
    }
}

/// Reports a snapshot phase to the task center: the volume, the stage
/// (reading directories, then indexing), the entry count and, at the end,
/// how the build ended.
pub(crate) fn emit_unified_snapshot_progress(
    tasks: &TaskManager,
    payload: &SnapshotProgressPayload,
) {
    if payload.task_id.is_empty() {
        return;
    }
    tasks.update(&payload.task_id, |view| {
        view.subject = vec![payload.volume_key.clone()];
        match payload.phase.as_str() {
            "started" | "walking" => view.stage = Some(TaskStage::Filesystem),
            "indexing" => view.stage = Some(TaskStage::Index),
            // Endings keep the stage they ended in.
            _ => {}
        }
        if payload.entries > 0 {
            view.counts.insert(TaskCount::Entries, payload.entries);
        }
        if payload.truncated {
            view.flags.insert(TaskFlag::Truncated);
        }
    });
    let message = payload
        .message
        .clone()
        .or_else(|| (payload.phase != "walking").then(|| format!("snapshot:{}", payload.phase)));
    tasks.emit_progress(
        &payload.task_id,
        TaskProgressUnit::Entries,
        payload.entries,
        None,
        message,
    );
    let outcome = match payload.phase.as_str() {
        "done" => TaskOutcome::Succeeded,
        "cancelled" => TaskOutcome::Cancelled,
        "error" => TaskOutcome::Failed {
            error: payload.message.clone().unwrap_or_default(),
        },
        _ => return,
    };
    tasks.finish(&payload.task_id, outcome);
}

/// Emits one progress event to both the task center and the snapshot chip.
pub(crate) fn report(app: &AppHandle, tasks: &TaskManager, payload: SnapshotProgressPayload) {
    emit_unified_snapshot_progress(tasks, &payload);
    emit_progress(app, payload);
}
