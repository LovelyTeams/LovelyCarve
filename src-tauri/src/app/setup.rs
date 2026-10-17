//! Backend startup shared by the desktop app and the CLI: preview scratch
//! space, plugin discovery and runtime event bridges. The desktop `setup`
//! hook adds its Tauri-only resources on top.
use crate::app::workflows::scheduler::PluginTaskRunner;
use crate::{
    core::{
        extensions::{
            plugin::PluginManager,
            task_scheduler::{
                FunctionRegistry, RegistrySource, RunnerFactory, SchedulerConfig, TaskRunner,
                TaskScheduler,
            },
        },
        filesystem::evidence_session::EvidenceSession,
        preview::media_cache::MediaScratch,
    },
    host::AppHandle,
    state::AppState,
};
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::sync::Mutex;

/// Prepare `state` for commands: create the preview scratch directory,
/// discover plugins and route task, scheduler and plugin events to `host`.
/// Returns the scratch directory so the desktop can expose it to its asset
/// protocol.
pub(crate) fn initialize(
    state: &AppState,
    host: &AppHandle,
) -> Result<Arc<MediaScratch>, Box<dyn std::error::Error>> {
    let scratch = Arc::new(MediaScratch::open(
        &std::env::temp_dir().join("lovelycarve-preview-cache"),
    )?);
    *state.media_scratch.lock() = Some(Arc::clone(&scratch));
    discover_plugins(state, host);
    let events = host.clone();
    state.scheduler.set_event_sink(move |event| {
        if let Err(err) = events.emit(
            crate::core::extensions::task_scheduler::SCHED_TASK_EVENT,
            event,
        ) {
            tracing::debug!(
                "emit {} failed: {err}",
                crate::core::extensions::task_scheduler::SCHED_TASK_EVENT
            );
        }
    });
    let events = host.clone();
    state.tasks.set_event_sink(move |event| {
        if let Err(err) = events.emit(crate::task_manager::TASK_LIFECYCLE_EVENT, event) {
            tracing::debug!(
                "emit {} failed: {err}",
                crate::task_manager::TASK_LIFECYCLE_EVENT
            );
        }
    });
    let events = host.clone();
    state.tasks.set_progress_sink(move |event| {
        if let Err(err) = events.emit(crate::task_manager::TASK_PROGRESS_EVENT, event) {
            tracing::debug!(
                "emit {} failed: {err}",
                crate::task_manager::TASK_PROGRESS_EVENT
            );
        }
    });
    Ok(scratch)
}

fn discover_plugins(state: &AppState, host: &AppHandle) {
    let mut plugin_roots = host
        .app_data_dir()
        .map(|directory| vec![directory.join("plugins")])
        .unwrap_or_default();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            plugin_roots.push(directory.join("plugins"));
        }
    }
    if let Some(configured) = std::env::var_os("LOVELYCARVE_PLUGIN_DIR") {
        plugin_roots.push(configured.into());
    }
    if cfg!(debug_assertions) {
        match std::env::current_dir() {
            Ok(directory) => plugin_roots.push(directory.join("plugins")),
            Err(err) => {
                tracing::debug!("plugin discovery: current directory unavailable: {err}")
            }
        }
        // `tauri dev` 以 src-tauri 为 cwd,仓库根的 plugins/ 需要
        // 从编译期 manifest 目录回溯才能命中(PLUGIN_PROTOCOL.md
        // 声明的开发期发现路径)。
        if let Some(repository_root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
            let candidate = repository_root.join("plugins");
            if !plugin_roots.contains(&candidate) {
                plugin_roots.push(candidate);
            }
        }
    }
    match PluginManager::discover(&plugin_roots) {
        Ok(manager) => {
            tracing::info!(
                plugins = manager.definitions().len(),
                "LPP plugin discovery complete"
            );
            let events = host.clone();
            // 共享实例(直接分析)的进度;调度任务的进度经各自的
            // worker 回到所属任务,不走这里。
            manager.set_progress_sink(move |event| {
                if let Err(err) = events.emit("plugin-progress", event) {
                    tracing::debug!("emit plugin-progress failed: {err}");
                }
            });
            match crate::core::extensions::plugin::PluginLogStore::beside_executable() {
                Ok(store) => manager.set_log_store(store),
                Err(err) => tracing::warn!("plugin log files are unavailable: {err}"),
            }
            *state.plugins.write() = Some(Arc::new(manager));
        }
        Err(err) => tracing::warn!("LPP plugin discovery failed: {err}"),
    }
}

/// Desktop setup hook: register the host handle for commands and let the
/// asset protocol stream materialised media previews.
#[cfg(feature = "desktop")]
pub(crate) fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager as _;
    let host = crate::host::desktop_handle(app.handle());
    let scratch = initialize(&app.state::<AppState>(), &host)?;
    app.asset_protocol_scope()
        .allow_directory(scratch.path(), true)?;
    app.manage(host);
    Ok(())
}

/// 组装任务调度器:function 注册表来自已发现的插件清单(插件发现即信任,
/// 全部可派发),执行器把任务交给 LPP 插件子进程。
pub(crate) fn build_task_scheduler(
    images: Arc<Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
) -> TaskScheduler {
    let registry_plugins = Arc::clone(&plugins);
    let registry_source: RegistrySource = Arc::new(move || {
        let manager = registry_plugins.read().clone();
        match manager {
            Some(manager) => FunctionRegistry::from_definitions(manager.definitions()),
            None => FunctionRegistry::default(),
        }
    });
    let runner_factory: RunnerFactory = Arc::new(move |case_dir| {
        Arc::new(PluginTaskRunner::new(
            Arc::clone(&images),
            Arc::clone(&plugins),
            case_dir.to_owned(),
        )) as Arc<dyn TaskRunner>
    });
    TaskScheduler::new(
        SchedulerConfig::from_env(|key| std::env::var(key).ok()),
        registry_source,
        runner_factory,
    )
}
