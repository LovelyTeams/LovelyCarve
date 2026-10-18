//! 分析结果浏览 IPC:结果树、分页表格与单条详情。
//! 节点名/列定义全部来自插件提交,前端零硬编码。

use crate::host::State;

use crate::{
    app::{error::AppError, workflows::casework::active_case_dir},
    core::analysis::results::analysis_query::{
        self, AnalysisRecord, AnalysisRecordPage, AnalysisRecordsQuery, AnalysisTreeEvidence,
        AnalysisTreeNode,
    },
    core::casework::case::EvidenceRef,
    state::AppState,
};

/// 分析结果树:检材 → 分类 → 功能 →
/// 结果项。未打开案件返回空树。
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

/// 按选中结果项的 run_id 分页读取记录,翻页期间不切换运行。
#[cfg_attr(feature = "desktop", tauri::command)]
#[allow(clippy::too_many_arguments)] // Flat IPC fields also generate the CLI options.
pub async fn analysis_records_page(
    state: State<'_, AppState>,
    image_id: String,
    analyzer: String,
    run_id: String,
    artifact_type: String,
    keyword: Option<String>,
    skip: i64,
    limit: i64,
) -> Result<AnalysisRecordPage, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || {
        let query = AnalysisRecordsQuery {
            image_id: &image_id,
            analyzer: &analyzer,
            run_id: &run_id,
            artifact_type: &artifact_type,
            keyword: keyword.as_deref(),
            skip,
            limit,
        };
        analysis_query::records_page(&case_dir, &query)
    })
    .await
    .map_err(|error| AppError::task_panicked("analysis_records_page", error))?
    .map_err(AppError::from_anyhow)
}

/// 单条记录详情(表格行 → 右侧预览)。结果按检材存放,带上记录所属检材。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn analysis_record_detail(
    state: State<'_, AppState>,
    image_id: String,
    artifact_id: String,
) -> Result<Option<AnalysisRecord>, AppError> {
    let case_dir = active_case_dir(&state).await?;
    tokio::task::spawn_blocking(move || {
        analysis_query::record_detail(&case_dir, &image_id, &artifact_id)
    })
    .await
    .map_err(|error| AppError::task_panicked("analysis_record_detail", error))?
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
