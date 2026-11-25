//! 插件包 IPC:列出、重置共享实例、检视与安装(.lcpkg)、卸载、读取说明。
//!
//! 安装分两步:`plugin_inspect` 只读出插件包是什么,`plugin_install` 带上检视
//! 时的 SHA-256 才安装同一份字节。分析器不再直接调用:文件、系统与检材分析一律
//! 建任务(`sched_task_create`),结果进分析结果树。

use std::sync::Arc;

use serde::Serialize;

use crate::host::State;
use crate::{
    app::error::AppError,
    core::{
        analysis::results::{models::RecordColumn, record_query},
        extensions::plugin::{
            AnalyzerScope, InstalledPackage, Locales, PackageInspection, PluginError,
            PluginManager, PluginPermission, PluginState, RejectedPackage, TaskTrigger,
        },
    },
    state::AppState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginListResponse {
    pub packages: Vec<PluginPackageInfo>,
    /// Packages that failed to load, with the reason.
    pub rejected: Vec<RejectedPackage>,
    /// Where installed packages go.
    pub install_directory: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPackageInfo {
    id: String,
    name: String,
    version: String,
    description: Option<String>,
    publisher: Option<String>,
    license: Option<String>,
    /// The package ships a `README.md` (`plugin_readme`).
    has_readme: bool,
    directory: String,
    /// The library file for this platform.
    library: String,
    library_present: bool,
    /// Installed by the user (can be uninstalled), not bundled.
    removable: bool,
    memory_limit: u64,
    fingerprint: String,
    /// State of the shared instance (filesystem browsing).
    state: String,
    error: Option<String>,
    permissions: Vec<PluginPermission>,
    /// SDK the library was built with (`schema.json`).
    sdk: Option<String>,
    schema_hash: Option<String>,
    analyzers: Vec<AnalyzerInfo>,
    filesystems: Vec<String>,
    records: Vec<RecordTypeInfo>,
    /// Translations by locale and key (`record.linux.user`).
    locales: Locales,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerInfo {
    id: String,
    name: String,
    scope: AnalyzerScope,
    platforms: Vec<String>,
    category: String,
    trigger: TaskTrigger,
    provides: Vec<String>,
    requires: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordTypeInfo {
    record_type: String,
    label: String,
    fields: Vec<RecordColumn>,
    properties: Option<crate::core::analysis::results::models::PropertyLayout>,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_list(state: State<'_, AppState>) -> Result<PluginListResponse, AppError> {
    let manager = plugin_manager(&state)?;
    // Fingerprints hash the package directory on first listing.
    tokio::task::spawn_blocking(move || {
        let packages = manager
            .definitions()
            .iter()
            .map(|definition| {
                let manifest = &definition.manifest;
                let plugin_state = manager
                    .state(&manifest.id)
                    .unwrap_or(PluginState::Configured);
                let (state, error) = state_view(&plugin_state);
                let library = manifest.library_path(&definition.directory);
                PluginPackageInfo {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    description: manifest.description.clone(),
                    publisher: manifest.publisher.clone(),
                    license: manifest.license.clone(),
                    has_readme: definition.has_readme(),
                    directory: definition.directory.to_string_lossy().into_owned(),
                    library_present: library.is_file(),
                    library: library.to_string_lossy().into_owned(),
                    removable: definition.removable,
                    memory_limit: manifest.memory_limit().unwrap_or_default(),
                    fingerprint: definition.fingerprint(),
                    state,
                    error,
                    permissions: manifest.permissions.clone(),
                    sdk: definition
                        .described
                        .as_ref()
                        .map(|described| described.sdk.clone()),
                    schema_hash: definition
                        .described
                        .as_ref()
                        .map(|described| described.schema_hash.clone()),
                    analyzers: manifest
                        .analyzers
                        .iter()
                        .map(|analyzer| AnalyzerInfo {
                            id: analyzer.id.clone(),
                            name: analyzer.name.clone(),
                            scope: analyzer.scope,
                            platforms: analyzer.platforms.clone(),
                            category: analyzer.category.clone(),
                            trigger: analyzer.trigger,
                            provides: analyzer.provides.clone(),
                            requires: analyzer.requires.clone(),
                        })
                        .collect(),
                    filesystems: manifest
                        .filesystems
                        .iter()
                        .map(|filesystem| filesystem.id.clone())
                        .collect(),
                    records: definition
                        .record_schemas()
                        .iter()
                        .filter(|schema| schema.record_type != record_query::PROFILE_FACT)
                        .map(|schema| RecordTypeInfo {
                            record_type: schema.record_type.clone(),
                            label: schema.label.clone(),
                            fields: record_query::columns(schema),
                            properties: schema.properties.clone(),
                        })
                        .collect(),
                    locales: definition.locales.as_ref().clone(),
                }
            })
            .collect();
        PluginListResponse {
            packages,
            rejected: manager.rejected(),
            install_directory: manager
                .install_root()
                .map(|root| root.to_string_lossy().into_owned()),
        }
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

/// What a package archive (`.lcpkg`) would install, read without running
/// anything from it: identity, declarations, digests and the relation to the
/// installed version.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_inspect(
    state: State<'_, AppState>,
    archive: String,
) -> Result<PackageInspection, AppError> {
    plugin_manager(&state)?
        .inspect(archive.into())
        .await
        .map_err(plugin_app_error)
}

/// Installs the package archive whose SHA-256 is `sha256` (the one
/// `plugin_inspect` reported), replacing an installed version of the same
/// plugin once the staged library passed its start check.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_install(
    state: State<'_, AppState>,
    archive: String,
    sha256: String,
) -> Result<InstalledPackage, AppError> {
    plugin_manager(&state)?
        .install(archive.into(), &sha256)
        .await
        .map_err(plugin_app_error)
}

/// The package's `README.md`, or `null` without one.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_readme(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<Option<String>, AppError> {
    let definition = plugin_manager(&state)?
        .definition(&plugin_id)
        .map_err(plugin_app_error)?;
    tokio::task::spawn_blocking(move || definition.readme())
        .await
        .map_err(|err| AppError::task_panicked("plugin_readme", err))?
        .map_err(plugin_app_error)
}

/// Removes an installed package. Its published results stay in the cases.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_uninstall(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), AppError> {
    plugin_manager(&state)?
        .uninstall(&plugin_id)
        .await
        .map_err(plugin_app_error)
}

pub(crate) fn plugin_manager(state: &AppState) -> Result<Arc<PluginManager>, AppError> {
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

fn plugin_app_error(error: PluginError) -> AppError {
    match error {
        // The manifest's own explanation, without the variant's prefix.
        PluginError::Manifest(message) => AppError::invalid_input(message),
        error => AppError::from(error),
    }
}
