//! Backend startup shared by the desktop app and the CLI: preview scratch
//! space, plugin discovery and runtime event bridges. The desktop `setup`
//! hook adds its Tauri-only resources on top.
use crate::app::workflows::scheduler::PluginTaskRunner;
use crate::app::workflows::task_center::{self, TaskEntryEvent};
use crate::{
    core::{
        analysis::identify::systems::SystemCache,
        extensions::{
            plugin::PluginManager,
            task_scheduler::{
                FunctionRegistry, RegistrySource, RunnerFactory, SchedEvent, SchedulerConfig,
                SystemTarget, TaskRunner, TaskScheduler,
            },
        },
        filesystem::evidence_session::EvidenceSession,
        preview::media_cache::MediaScratch,
    },
    host::AppHandle,
    state::AppState,
    task_manager::TaskKind,
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
    route_task_events(state, host);
    Ok(scratch)
}

/// Every task change reaches the task center as one `task-entry` event:
/// plugin jobs from the scheduler, native tasks from the registry (whose
/// lifecycle and progress events also keep going out for their pages).
fn route_task_events(state: &AppState, host: &AppHandle) {
    let events = host.clone();
    state.scheduler.set_event_sink(move |event| {
        let event = match event {
            SchedEvent::TaskChanged { task } => {
                TaskEntryEvent::changed(task_center::job_entry(*task))
            }
            SchedEvent::TaskRemoved { task_id, .. } => TaskEntryEvent::Removed {
                id: task_center::plugin_entry_id(task_id),
            },
            SchedEvent::Notice { notice } => TaskEntryEvent::Notice { notice },
        };
        emit(&events, task_center::TASK_ENTRY_EVENT, &event);
    });
    let events = host.clone();
    let tasks = state.tasks.clone();
    let carve_scans = Arc::clone(&state.carve_scans);
    state.tasks.set_event_sink(move |event| {
        emit(&events, crate::task_manager::TASK_LIFECYCLE_EVENT, &event);
        if event.task.kind == TaskKind::CarveScan {
            // A scan's entry is its session: show it stopping.
            let Ok(scans) = carve_scans.try_lock() else {
                return;
            };
            let scan = scans
                .values()
                .map(|session| session.read().snapshot())
                .find(|scan| scan.task_id == event.task.id);
            drop(scans);
            if let Some(scan) = scan {
                task_center::publish_carve(&events, &tasks, &scan);
            }
        } else if let Some(entry) = task_center::native_entry(&event.task) {
            emit(
                &events,
                task_center::TASK_ENTRY_EVENT,
                &TaskEntryEvent::changed(entry),
            );
        }
    });
    let events = host.clone();
    state.tasks.set_progress_sink(move |event| {
        emit(&events, crate::task_manager::TASK_PROGRESS_EVENT, &event);
        if let Some(entry) = task_center::native_entry(&event.task) {
            emit(
                &events,
                task_center::TASK_ENTRY_EVENT,
                &TaskEntryEvent::changed(entry),
            );
        }
    });
}

fn emit(host: &AppHandle, event: &str, payload: &impl serde::Serialize) {
    if let Err(err) = host.emit(event, payload) {
        tracing::debug!("emit {event} failed: {err}");
    }
}

fn discover_plugins(state: &AppState, host: &AppHandle) {
    // Installed packages go to the per-user directory; bundled ones sit next
    // to the executable.
    let install_root = host
        .app_data_dir()
        .map(|directory| directory.join("plugins"));
    let mut plugin_roots = Vec::new();
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
    match PluginManager::discover(&plugin_roots, install_root) {
        Ok(manager) => {
            tracing::info!(
                plugins = manager.definitions().len(),
                rejected = manager.rejected().len(),
                "plugin discovery complete"
            );
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

/// 组装任务调度器:function 注册表来自已发现的插件包(发现即信任,全部可
/// 派发),执行器把任务交给 lc-plugin-host 插件进程;系统作用域的任务从
/// 已识别的系统里选目标。
pub(crate) fn build_task_scheduler(
    images: Arc<Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
    systems: Arc<SystemCache>,
) -> TaskScheduler {
    let registry_plugins = Arc::clone(&plugins);
    let registry_source: RegistrySource = Arc::new(move || {
        let manager = registry_plugins.read().clone();
        match manager {
            Some(manager) => FunctionRegistry::from_definitions(&manager.definitions()),
            None => FunctionRegistry::default(),
        }
    });
    let runner_images = Arc::clone(&images);
    let runner_systems = Arc::clone(&systems);
    let runner_factory: RunnerFactory = Arc::new(move |case_dir| {
        Arc::new(PluginTaskRunner::new(
            Arc::clone(&runner_images),
            Arc::clone(&plugins),
            Arc::clone(&runner_systems),
            case_dir.to_owned(),
        )) as Arc<dyn TaskRunner>
    });
    let scheduler = TaskScheduler::new(
        SchedulerConfig::from_env(|key| std::env::var(key).ok()),
        registry_source,
        runner_factory,
    );
    // Called on blocking workers (task creation, evidence online).
    scheduler.set_systems_source(move |image_id| {
        let Some(image) = images.blocking_lock().get(image_id).cloned() else {
            return Vec::new();
        };
        systems
            .get_or_discover(&image)
            .iter()
            .map(|system| SystemTarget {
                id: system.id.clone(),
                family: system.family.clone(),
                label: system.label.clone(),
            })
            .collect()
    });
    scheduler
}
