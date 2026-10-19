//! A live plugin instance: an `lc-plugin-host` process speaking LPP v2 on
//! its stdio, or (tests) a plugin entry point served by the SDK bridge over
//! in-memory pipes. Both go through the same client and frames.

use std::{
    env::consts::EXE_SUFFIX,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lovelycarve_plugin::protocol::Hello;
use rmpv::Value;

use super::{
    client::{HostRpcServices, PluginClient, PluginCommand},
    run_context::RunContext,
    PluginDefinition, PluginError, PluginLogStore, Result, SourceRegistry,
};

const PLUGIN_HOST: &str = "lc-plugin-host";

#[derive(Clone)]
pub struct PluginConnection {
    client: Arc<PluginClient>,
}

impl PluginConnection {
    /// Starts `definition`'s library in its own `lc-plugin-host` process and
    /// checks its `plugin.hello` against the package.
    pub(super) async fn open(
        definition: &PluginDefinition,
        sources: SourceRegistry,
        logs: Option<Arc<PluginLogStore>>,
    ) -> Result<Self> {
        let library = definition.manifest.library_path(&definition.directory);
        if !library.is_file() {
            return Err(PluginError::Unavailable(format!(
                "plugin library {} does not exist",
                library.display()
            )));
        }
        let command = PluginCommand {
            plugin_id: definition.manifest.id.clone(),
            program: plugin_host_executable()?,
            args: vec!["--library".into(), library.into_os_string()],
            working_directory: Some(definition.directory.clone()),
            memory_limit: definition.manifest.memory_limit()?,
            permissions: definition.manifest.permissions.clone(),
        };
        let client = PluginClient::spawn(command, HostRpcServices::new(sources), logs).await?;
        let connection = Self { client };
        connection.verify(definition).await?;
        Ok(connection)
    }

    /// Serves `entry` through the SDK bridge over in-memory pipes: the same
    /// frames and client as a plugin process, without one.
    #[cfg(test)]
    pub(super) async fn loopback(
        entry: lovelycarve_plugin::abi::EntryFn,
        definition: &PluginDefinition,
        sources: SourceRegistry,
    ) -> Result<Self> {
        use tokio_util::io::SyncIoBridge;

        let (host_side, plugin_side) = tokio::io::duplex(1 << 20);
        let (plugin_reader, plugin_writer) = tokio::io::split(plugin_side);
        let runtime = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let input = SyncIoBridge::new_with_handle(plugin_reader, runtime.clone());
            let output = SyncIoBridge::new_with_handle(plugin_writer, runtime);
            if let Err(error) = lovelycarve_plugin::bridge::serve(entry, input, Box::new(output)) {
                tracing::debug!("loopback plugin ended: {error}");
            }
        });
        let (host_reader, host_writer) = tokio::io::split(host_side);
        let client = PluginClient::connect(
            Box::new(host_reader),
            Box::new(host_writer),
            HostRpcServices::new(sources),
            None,
            None,
        )
        .await?;
        let connection = Self { client };
        connection.verify(definition).await?;
        Ok(connection)
    }

    /// Wraps a connected client (tests of the frozen filesystem methods).
    #[cfg(test)]
    pub(super) fn from_client(client: Arc<PluginClient>) -> Self {
        Self { client }
    }

    async fn verify(&self, definition: &PluginDefinition) -> Result<()> {
        if let Err(error) = definition.verify_hello(self.client.hello()) {
            if let Err(shutdown) = self.client.shutdown().await {
                tracing::debug!(plugin_id = %definition.manifest.id, "rejected plugin shutdown failed: {shutdown}");
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn hello(&self) -> &Hello {
        self.client.hello()
    }

    pub fn is_alive(&self) -> bool {
        self.client.is_alive()
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        self.client
            .call_with_timeout(method, params, deadline)
            .await
    }

    /// Blocking call for the synchronous `Volume` bridge; never call it from
    /// an async worker.
    pub fn call_blocking_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        self.client
            .call_blocking_with_timeout(method, params, deadline)
    }

    /// Makes a run's handles reachable from this plugin.
    pub(super) fn attach(&self, run: Arc<RunContext>) {
        self.client.services().attach(run);
    }

    pub(super) fn detach(&self, run_id: &str) -> Option<Arc<RunContext>> {
        self.client.services().detach(run_id)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.client.shutdown().await
    }

    /// Ends the instance without the `plugin.shutdown` call: kills the
    /// `lc-plugin-host` process. Stops one task's worker.
    pub async fn terminate(&self) {
        self.client.kill().await;
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
