//! 分析结果浏览 IPC:结果树、分页表格、单条详情与运行审计。
//! 节点名与列定义来自插件声明;旧案件的结果只读展示。

use crate::host::State;

use crate::{
    app::{error::AppError, workflows::casework::active_case_dir},
    core::analysis::results::{
        analysis_query::{self, AnalysisRecordsQuery, AnalysisTreeEvidence},
        models::{AnalysisRecordDetail, AnalysisRecordsPage, AnalysisTreeNode},
        record_query::RunAudit,
    },
    core::casework::case::EvidenceRef,
    state::AppState,
};

/// 分析结果树:检材 → 分类 → 分析器运行 → 记录类型。未打开案件返回空树。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn analysis_result_tree(
    state: State<'_, AppState>,
) -> Result<Vec<AnalysisTreeNode>, AppError> {
    let (case_dir, evidence) = {
        let guard = state.active_case.lock().await;
        match guard.as_ref() {
            Some(case) => (Some(case.dir.clone()), case.evidence_refs()),
            None => (None, Vec::new()),
        }
    };
    let Some(case_dir) = case_dir else {
        return Ok(Vec::new());
    };
    let evidence = evidence
        .into_iter()
        .map(|evidence| {
            let label = evidence_label(&evidence);
            AnalysisTreeEvidence {
                image_id: evidence.image_id,
                label,
            }
        })
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || analysis_query::analysis_tree(&case_dir, &evidence))
        .await
        .map_err(|error| AppError::task_panicked("analysis_result_tree", error))?
        .map_err(AppError::from_anyhow)
}

/// 按选中节点的 runId 分页读取一种记录,翻页期间不切换运行。
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)] // Flat IPC fields also generate the CLI options.
pub async fn analysis_records_page(
    state: State<'_, AppState>,
    image_id: String,
    run_id: String,
    record_type: String,
    legacy: Option<bool>,
    keyword: Option<String>,
    sort: Option<String>,
    descending: Option<bool>,
    skip: i64,
    limit: i64,
) -> Result<AnalysisRecordsPage, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || {
        let query = AnalysisRecordsQuery {
            image_id: &image_id,
            run_id: &run_id,
            record_type: &record_type,
            legacy: legacy.unwrap_or(false),
            keyword: keyword.as_deref(),
            sort: sort.as_deref(),
            descending: descending.unwrap_or(false),
            skip,
            limit,
        };
        analysis_query::records_page(&case_dir, &query)
    })
    .await
    .map_err(|error| AppError::task_panicked("analysis_records_page", error))?
    .map_err(AppError::from_anyhow)
}

/// 单条记录详情(表格行 → 右侧预览),带它所属类型的列。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn analysis_record_detail(
    state: State<'_, AppState>,
    image_id: String,
    record_id: String,
) -> Result<Option<AnalysisRecordDetail>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || {
        analysis_query::record_detail(&case_dir, &image_id, &record_id)
    })
    .await
    .map_err(|error| AppError::task_panicked("analysis_record_detail", error))?
    .map_err(AppError::from_anyhow)
}

/// 一次运行的审计:目标、输入、选项、画像事实、告警与合成字节。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn analysis_run_get(
    state: State<'_, AppState>,
    image_id: String,
    run_id: String,
) -> Result<Option<RunAudit>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || analysis_query::run_audit(&case_dir, &image_id, &run_id))
        .await
        .map_err(|error| AppError::task_panicked("analysis_run_get", error))?
        .map_err(AppError::from_anyhow)
}

/// 检材节点展示名:镜像文件名(与案件管理列表一致)。
fn evidence_label(evidence: &EvidenceRef) -> String {
    evidence
        .path
        .rsplit(['\\', '/'])
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(&evidence.path)
        .to_owned()
}
