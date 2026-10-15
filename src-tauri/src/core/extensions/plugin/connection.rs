//! A live plugin instance in one of the two isolation modes. Both carry the
//! same LPP methods and payloads, so callers never branch on the mode.

use std::{
    env::consts::EXE_SUFFIX,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rmpv::Value;

use super::{
    client::{HostRpcServices, PluginSinks},
    in_process::InProcessClient,
    PluginClient, PluginCommand, PluginDefinition, PluginError, PluginIdentity, PluginIsolation,
    Result,
};

const PLUGIN_HOST: &str = "lc-plugin-host";

#[derive(Clone)]
pub enum PluginConnection {
    /// The library runs in `lc-plugin-host`, reached over LPP stdio frames.
    Process(Arc<PluginClient>),
    /// The library is loaded into LovelyCarve and called directly.
    InProcess(Arc<InProcessClient>),
}

impl PluginConnection {
    /// Starts `definition`'s library in its declared isolation mode.
    pub(super) async fn open(
        definition: &PluginDefinition,
        services: HostRpcServices,
        sinks: PluginSinks,
    ) -> Result<Self> {
        let library = definition.manifest.library_path(&definition.directory);
        if !library.is_file() {
            return Err(PluginError::Unavailable(format!(
                "plugin library {} does not exist",
                library.display()
            )));
        }
        match definition.manifest.library.isolation {
            PluginIsolation::Process => {
                let command = PluginCommand {
                    plugin_id: definition.manifest.id.clone(),
                    program: plugin_host_executable()?,
                    args: vec!["--library".into(), library.into_os_string()],
                    working_directory: Some(definition.directory.clone()),
                };
                Ok(Self::Process(
                    PluginClient::spawn(command, services, sinks).await?,
                ))
            }
            PluginIsolation::InProcess => {
                let plugin_id = definition.manifest.id.clone();
                // Loading runs the library's initialisers; keep them off the
                // async workers.
                let client = tokio::task::spawn_blocking(move || {
                    InProcessClient::load(&library, &plugin_id, services, sinks)
                })
                .await
                .map_err(|error| PluginError::Unavailable(error.to_string()))??;
                Ok(Self::InProcess(client))
            }
        }
    }

    pub fn isolation(&self) -> PluginIsolation {
        match self {
            Self::Process(_) => PluginIsolation::Process,
            Self::InProcess(_) => PluginIsolation::InProcess,
        }
    }

    pub fn identity(&self) -> &PluginIdentity {
        match self {
            Self::Process(client) => client.identity(),
            Self::InProcess(client) => client.identity(),
        }
    }

    pub fn capabilities(&self) -> &[String] {
        match self {
            Self::Process(client) => client.capabilities(),
            Self::InProcess(client) => client.capabilities(),
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Self::Process(client) => client.is_alive(),
            Self::InProcess(client) => client.is_alive(),
        }
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        match self {
            Self::Process(client) => client.call_with_timeout(method, params, deadline).await,
            Self::InProcess(client) => client.call_with_timeout(method, params, deadline).await,
        }
    }

    /// Blocking call for the synchronous `Volume` bridge; never call it from
    /// an async worker.
    pub fn call_blocking_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        match self {
            Self::Process(client) => client.call_blocking_with_timeout(method, params, deadline),
            Self::InProcess(client) => client.call_blocking_with_timeout(method, params, deadline),
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self {
            Self::Process(client) => client.shutdown().await,
            Self::InProcess(client) => client.shutdown().await,
        }
    }

    /// Ends the instance without the `plugin.shutdown` call: kills the
    /// `lc-plugin-host` process, or cancels the in-process calls and closes
    /// the instance. Stops one task's worker.
    pub async fn terminate(&self) {
        match self {
            Self::Process(client) => client.kill().await,
            Self::InProcess(client) => client.terminate(),
        }
    }
}

/// `lc-plugin-host` next to the running executable, overridable with
/// `LOVELYCARVE_PLUGIN_HOST`. Debug builds also use the plugin workspace's
/// build output, so `tauri dev`, tests and examples find it.
fn plugin_host_executable() -> Result<PathBuf> {
    let file_name = format!("{PLUGIN_HOST}{EXE_SUFFIX}");
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("LOVELYCARVE_PLUGIN_HOST") {
        candidates.push(PathBuf::from(configured));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(&file_name));
        }
    }
    if cfg!(debug_assertions) {
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins")
            .join("target");
        candidates.push(target.join("debug").join(&file_name));
        candidates.push(target.join("release").join(&file_name));
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            PluginError::Unavailable(format!(
                "{file_name} was not found next to LovelyCarve (set LOVELYCARVE_PLUGIN_HOST or run `just build-plugins`)"
            ))
        })
}
