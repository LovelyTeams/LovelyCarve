//! Runtime runs state.

use std::collections::HashMap;
use std::sync::Arc;

/// Live runs by key. Held only for short map operations, never across an
/// `.await`, so progress callbacks on blocking threads and [`RunRegistration`]'s
/// `Drop` can take it synchronously.
pub type RunRegistry<V> = Arc<parking_lot::Mutex<HashMap<String, V>>>;

/// One live entry of a [`RunRegistry`]. Dropping it removes the entry, so a run
/// that ends on any path — an early return, a panic or a dropped future — leaves
/// no "running" entry behind. A run that must persist its final state before it
/// stops counting as live drops the registration right after that write.
#[must_use = "dropping the registration unregisters the run"]
pub struct RunRegistration<V> {
    runs: RunRegistry<V>,
    key: String,
}

impl<V> RunRegistration<V> {
    /// Registers `value` under `key`, replacing an entry already under it.
    pub fn insert(runs: &RunRegistry<V>, key: String, value: V) -> Self {
        runs.lock().insert(key.clone(), value);
        Self {
            runs: Arc::clone(runs),
            key,
        }
    }
}

impl<V> Drop for RunRegistration<V> {
    fn drop(&mut self) {
        self.runs.lock().remove(&self.key);
    }
}

/// 一个在跑的全局搜索任务的进程内登记（见 [`super::AppState::global_searches`]）。
/// 计数由进度回调刷新；权威结果在检材的 `search.db`。
#[derive(Debug, Clone)]
pub struct GlobalSearchRunState {
    pub image_id: String,
    pub started_ms: i64,
    pub files_done: u64,
    pub files_total: u64,
    pub hits: u64,
    pub matched_files: u64,
}

/// 一个在跑的时间线重建的进程内登记（见 [`super::AppState::timeline_builds`]，
/// key = 检材 image id）。计数由进度回调刷新；权威状态在检材 `timeline.db`。
#[derive(Debug, Clone)]
pub struct TimelineBuildRunState {
    pub started_ms: i64,
    pub done: u64,
    pub total: u64,
    pub rows: u64,
}

#[cfg(test)]
mod tests {
    use super::{RunRegistration, RunRegistry};

    #[test]
    fn a_registration_removes_its_entry_on_every_exit() {
        let runs: RunRegistry<u32> = RunRegistry::default();
        let registration = RunRegistration::insert(&runs, "a".to_string(), 1);
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _panicking = RunRegistration::insert(&runs, "b".to_string(), 2);
            panic!("worker panicked");
        }));

        assert!(finished.is_err());
        assert_eq!(runs.lock().keys().cloned().collect::<Vec<_>>(), ["a"]);
        drop(registration);
        assert!(runs.lock().is_empty());
    }
}
