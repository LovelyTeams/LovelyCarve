//! Runtime runs state.

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
