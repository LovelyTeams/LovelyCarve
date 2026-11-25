//! 插件市场 IPC:索引源的读写、某个源的插件清单、下载并检视一个版本、释放下载,
//! 以及从一个目录的插件包生成 `index.json`。
//!
//! 下载得到的插件包仍经 `plugin_install` 安装(带检视时的 SHA-256),之后或取消时
//! 调 `plugin_market_release` 删掉下载文件。发布页安装使用固定的官方索引。

use serde::Serialize;

use super::plugin::plugin_manager;
use crate::host::State;
use crate::{
    app::error::AppError,
    core::extensions::plugin_market::{
        generate_index, GeneratedIndex, MarketCatalog, MarketDownload, PluginMarket,
    },
    state::AppState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSources {
    /// Index locations in the user's order, each in its stored spelling.
    pub sources: Vec<String>,
}

fn plugin_market(state: &AppState) -> Result<&PluginMarket, AppError> {
    state
        .plugin_market
        .get()
        .ok_or_else(|| AppError::not_found("插件市场尚未初始化"))
}

/// The configured index sources.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_sources(state: State<'_, AppState>) -> Result<MarketSources, AppError> {
    Ok(MarketSources {
        sources: plugin_market(&state)?.sources()?,
    })
}

/// Replaces the index sources: `https://` addresses or absolute local paths
/// of a market directory or index file.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_set_sources(
    state: State<'_, AppState>,
    sources: Vec<String>,
) -> Result<MarketSources, AppError> {
    Ok(MarketSources {
        sources: plugin_market(&state)?.set_sources(&sources).await?,
    })
}

/// A configured source's packages against the installed plugins; `refresh`
/// reads the index again instead of using the one read in the last ten
/// minutes.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_list(
    state: State<'_, AppState>,
    source: String,
    refresh: Option<bool>,
) -> Result<MarketCatalog, AppError> {
    let manager = plugin_manager(&state)?;
    Ok(plugin_market(&state)?
        .catalog(&manager, &source, refresh.unwrap_or(false))
        .await?)
}

/// Downloads `plugin_id` `version` for this platform from a configured
/// source, checks its size and SHA-256 against the index and inspects it.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_download(
    state: State<'_, AppState>,
    source: String,
    plugin_id: String,
    version: String,
) -> Result<MarketDownload, AppError> {
    let manager = plugin_manager(&state)?;
    Ok(plugin_market(&state)?
        .download(&manager, &source, &plugin_id, &version)
        .await?)
}

/// Downloads and inspects a version from the fixed official market index.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_download_official(
    state: State<'_, AppState>,
    plugin_id: String,
    version: String,
) -> Result<MarketDownload, AppError> {
    let manager = plugin_manager(&state)?;
    Ok(plugin_market(&state)?
        .download_official(&manager, &plugin_id, &version)
        .await?)
}

/// Removes a download once it was installed or declined.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_release(
    state: State<'_, AppState>,
    sha256: String,
) -> Result<(), AppError> {
    Ok(plugin_market(&state)?.release(&sha256)?)
}

/// Writes `<directory>/index.json` for the `.lcpkg` files in `directory`.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn plugin_market_index(
    directory: String,
    title: String,
    description: Option<String>,
) -> Result<GeneratedIndex, AppError> {
    tokio::task::spawn_blocking(move || {
        generate_index(
            std::path::Path::new(&directory),
            &title,
            description.as_deref(),
        )
    })
    .await
    .map_err(|err| AppError::task_panicked("plugin_market_index", err))?
    .map_err(AppError::from)
}
