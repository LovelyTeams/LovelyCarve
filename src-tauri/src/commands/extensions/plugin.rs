use crate::host::State;
use serde::Serialize;

use crate::{
    app::{access::loaded_image_app_error, error::AppError},
    core::{
        analysis::results::models::PluginAnalysisResult,
        analysis::results::plugin_result_store::{self, NewPluginAnalysis, StoredPluginAnalysis},
        extensions::plugin::{
            FileAnalysisRequest, PluginIsolation, PluginManager, PluginPermission, PluginState,
        },
    },
    state::AppState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    id: String,
    name: String,
    version: String,
    directory: String,
    /// The plugin library file.
    library: String,
    /// `process` (inside lc-plugin-host) or `inProcess`.
    isolation: String,
    fingerprint: String,
    state: String,
    error: Option<String>,
    permissions: Vec<PluginPermission>,
    capabilities: Vec<PluginCapabilityInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilityInfo {
    kind: String,
    name: String,
    priority: i32,
    extensions: Vec<String>,
    mime_types: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnalysisRunResponse {
    run_id: Option<String>,
    persisted: bool,
    plugin_id: String,
    plugin_version: String,
    analyzer: String,
    image_id: String,
    node_id: String,
    file_name: String,
    started_at: String,
    completed_at: String,
    result: PluginAnalysisResult,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_list(state: State<'_, AppState>) -> Result<Vec<PluginInfo>, AppError> {
    let manager = plugin_manager(&state)?;
    // Fingerprints are computed lazily on first listing and hash the plugin
    // directory, so build the list on a blocking thread.
    tokio::task::spawn_blocking(move || {
        manager
            .definitions()
            .iter()
            .map(|definition| {
                let plugin_state = manager
                    .state(&definition.manifest.id)
                    .unwrap_or(PluginState::Configured);
                let (state, error) = state_view(&plugin_state);
                let mut capabilities = definition
                    .manifest
                    .filesystem_capabilities()
                    .map(|capability| PluginCapabilityInfo {
                        kind: "filesystem".to_owned(),
                        name: capability.filesystem.clone(),
                        priority: capability.priority,
                        extensions: Vec::new(),
                        mime_types: Vec::new(),
                    })
                    .collect::<Vec<_>>();
                capabilities.extend(definition.manifest.file_analyzer_capabilities().map(
                    |capability| PluginCapabilityInfo {
                        kind: "fileAnalyzer".to_owned(),
                        name: capability.analyzer.clone(),
                        priority: capability.priority,
                        extensions: capability.extensions.clone(),
                        mime_types: capability.mime_types.clone(),
                    },
                ));
                capabilities.extend(definition.manifest.evidence_analyzer_capabilities().map(
                    |capability| {
                        PluginCapabilityInfo {
                            kind: "evidenceAnalyzer".to_owned(),
                            name: capability
                                .display
                                .clone()
                                .unwrap_or_else(|| capability.analyzer.clone()),
                            priority: capability.priority,
                            extensions: Vec::new(),
                            mime_types: Vec::new(),
                        }
                    },
                ));
                let isolation = match definition.manifest.library.isolation {
                    PluginIsolation::Process => "process",
                    PluginIsolation::InProcess => "inProcess",
                };
                PluginInfo {
                    id: definition.manifest.id.clone(),
                    name: definition.manifest.name.clone(),
                    version: definition.manifest.version.clone(),
                    directory: definition.directory.to_string_lossy().into_owned(),
                    library: definition
                        .manifest
                        .library_path(&definition.directory)
                        .to_string_lossy()
                        .into_owned(),
                    isolation: isolation.to_owned(),
                    fingerprint: definition.fingerprint(),
                    state,
                    error,
                    permissions: definition.manifest.permissions.clone(),
                    capabilities,
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|err| AppError::task_panicked("plugin_list", err))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_restart(state: State<'_, AppState>, plugin_id: String) -> Result<(), AppError> {
    plugin_manager(&state)?
        .restart(&plugin_id)
        .await
        .map_err(plugin_app_error)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_analyze_file(
    state: State<'_, AppState>,
    plugin_id: String,
    analyzer: String,
    image_id: String,
    node_id: String,
    file_name: String,
) -> Result<PluginAnalysisRunResponse, AppError> {
    let manager = plugin_manager(&state)?;
    let context = AnalysisCaseContext::capture(&state).await;
    let image = loaded_image_app_error(&state, &image_id).await?;
    let definition = manager
        .definitions()
        .iter()
        .find(|definition| definition.manifest.id == plugin_id);
    let plugin_version = definition
        .map(|definition| definition.manifest.version.clone())
        .ok_or_else(|| AppError::not_found("插件不存在"))?;
    // 结果树的功能级标签来自插件注册的 function 元数据。
    let function = state
        .scheduler
        .registry()
        .all()
        .iter()
        .find(|function| function.plugin_id == plugin_id && function.analyzer == analyzer)
        .cloned();
    let display_name = function
        .as_ref()
        .map(|function| function.display_name.clone())
        .unwrap_or_else(|| analyzer.clone());
    let type_name = function
        .map(|function| function.type_name)
        .unwrap_or_else(|| "analysis".to_owned());
    let started_at = chrono::Utc::now().to_rfc3339();
    let result = manager
        .analyze_file(FileAnalysisRequest {
            plugin_id: plugin_id.clone(),
            analyzer: analyzer.clone(),
            image_id: image_id.clone(),
            image,
            node_id: node_id.clone(),
            name: file_name.clone(),
        })
        .await
        .map_err(plugin_app_error)?;
    let completed_at = chrono::Utc::now().to_rfc3339();
    let _publication = context.lock_publication(&state, &image_id).await?;
    let run_id = if let Some(case_dir) = context.case_dir {
        let result_for_store = result.clone();
        let plugin_id_for_store = plugin_id.clone();
        let plugin_version_for_store = plugin_version.clone();
        let analyzer_for_store = analyzer.clone();
        let display_name_for_store = display_name.clone();
        let type_name_for_store = type_name.clone();
        let image_id_for_store = image_id.clone();
        let node_id_for_store = node_id.clone();
        let file_name_for_store = file_name.clone();
        let started_at_for_store = started_at.clone();
        let completed_at_for_store = completed_at.clone();
        Some(
            tokio::task::spawn_blocking(move || {
                plugin_result_store::persist(
                    &case_dir,
                    NewPluginAnalysis {
                        plugin_id: &plugin_id_for_store,
                        plugin_version: &plugin_version_for_store,
                        analyzer: &analyzer_for_store,
                        display_name: &display_name_for_store,
                        type_name: &type_name_for_store,
                        image_id: &image_id_for_store,
                        node_id: &node_id_for_store,
                        file_name: &file_name_for_store,
                        started_at: &started_at_for_store,
                        completed_at: &completed_at_for_store,
                        result: &result_for_store,
                    },
                )
            })
            .await
            .map_err(|error| AppError::task_panicked("plugin result persistence", error))?
            .map_err(AppError::from_anyhow)?,
        )
    } else {
        None
    };
    Ok(PluginAnalysisRunResponse {
        persisted: run_id.is_some(),
        run_id,
        plugin_id,
        plugin_version,
        analyzer,
        image_id,
        node_id,
        file_name,
        started_at,
        completed_at,
        result,
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_analysis_list(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<Vec<StoredPluginAnalysis>, AppError> {
    let case_dir = state
        .active_case
        .lock()
        .await
        .as_ref()
        .map(|active_case| active_case.dir.clone());
    let Some(case_dir) = case_dir else {
        return Ok(Vec::new());
    };
    tokio::task::spawn_blocking(move || {
        plugin_result_store::list_for_node(&case_dir, &image_id, &node_id)
    })
    .await
    .map_err(|error| AppError::task_panicked("plugin result query", error))?
    .map_err(AppError::from_anyhow)
}

/// Captured before analysis; the gate also fences writes against case closure
/// and evidence withdrawal, including reopening the same case directory.
struct AnalysisCaseContext {
    generation: u64,
    case_dir: Option<std::path::PathBuf>,
}

impl AnalysisCaseContext {
    async fn capture(state: &AppState) -> Self {
        let generation = state.image_load_context.lock().await;
        let case_dir = state
            .active_case
            .lock()
            .await
            .as_ref()
            .map(|case| case.dir.clone());
        Self {
            generation: *generation,
            case_dir,
        }
    }

    async fn lock_publication<'a>(
        &self,
        state: &'a AppState,
        image_id: &str,
    ) -> Result<tokio::sync::MutexGuard<'a, u64>, AppError> {
        let generation = state.image_load_context.lock().await;
        let case_dir = state
            .active_case
            .lock()
            .await
            .as_ref()
            .map(|case| case.dir.clone());
        if *generation != self.generation
            || case_dir != self.case_dir
            || !state.images.lock().await.contains_key(image_id)
            || state.removed_case_evidence.lock().await.contains(image_id)
        {
            return Err(AppError::invalid_input(
                "分析期间案件或检材已改变，请重新分析",
            ));
        }
        Ok(generation)
    }
}

fn plugin_manager(state: &AppState) -> Result<std::sync::Arc<PluginManager>, AppError> {
    state
        .plugins
        .read()
        .clone()
        .ok_or_else(|| AppError::not_found("插件运行时尚未初始化"))
}

fn state_view(state: &PluginState) -> (String, Option<String>) {
    match state {
        PluginState::Configured => ("configured".to_owned(), None),
        PluginState::Starting => ("starting".to_owned(), None),
        PluginState::Running => ("running".to_owned(), None),
        PluginState::Crashed(error) => ("crashed".to_owned(), Some(error.clone())),
    }
}

fn plugin_app_error(error: crate::core::extensions::plugin::PluginError) -> AppError {
    AppError::from_anyhow(anyhow::Error::new(error))
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::core::{
        casework::case::{self, CreateCaseOptions},
        filesystem::evidence_session::EvidenceSession,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn publication_rejects_case_switch_reopen_and_evidence_withdrawal() {
        let temporary = tempfile::tempdir().expect("fixture");
        let state = AppState::new();
        let open_case = |name: &str| {
            case::create_case(
                &temporary.path().join(name),
                CreateCaseOptions {
                    name: name.into(),
                    case_number: None,
                    description: None,
                    examiner: None,
                },
            )
            .expect("case")
        };
        *state.active_case.lock().await = Some(open_case("a"));
        let path = temporary.path().join("image.raw");
        std::fs::write(&path, vec![0; 4096]).expect("raw");
        let image = Arc::new(EvidenceSession::open(&path).expect("image"));
        let image_id = image.id.clone();
        state.images.lock().await.insert(image_id.clone(), image);
        let context = AnalysisCaseContext::capture(&state).await;
        drop(
            context
                .lock_publication(&state, &image_id)
                .await
                .expect("same context"),
        );
        *state.active_case.lock().await = Some(open_case("b"));
        assert!(context.lock_publication(&state, &image_id).await.is_err());
        let context = AnalysisCaseContext::capture(&state).await;
        *state.image_load_context.lock().await += 1;
        assert!(context.lock_publication(&state, &image_id).await.is_err());
        let context = AnalysisCaseContext::capture(&state).await;
        state
            .removed_case_evidence
            .lock()
            .await
            .insert(image_id.clone());
        assert!(context.lock_publication(&state, &image_id).await.is_err());
        assert!(
            !crate::core::casework::case_paths::evidence_db_path(
                &temporary.path().join("b"),
                &image_id,
                crate::core::casework::case_paths::EvidenceDb::Analysis
            )
            .exists(),
            "stale analysis never creates derived outputs"
        );
    }
}
