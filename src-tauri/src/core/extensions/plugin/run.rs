//! Analyzer runs on per-task workers.
//!
//! Every scheduled task starts an instance of its own ([`TaskWorker`]): a
//! separate `lc-plugin-host` process with the package's memory limit, so
//! tasks of one plugin run side by side and stopping a task kills only its
//! process. A run's [`RunContext`] is attached to the worker for the
//! duration of `run.start` — the plugin reaches its view, files and record
//! stream through it — and detached when the call ends, also when the
//! caller stops waiting.

use std::{mem::DropGuard, sync::Arc, time::Duration};

use lovelycarve_plugin::protocol::{self, RunFinish, RunStart};

use super::{run_context::RunContext, PluginConnection, PluginError, PluginManager, Result};

/// Longest single run. Progress reports keep the scheduler's watchdog from
/// declaring a long run stuck; this only bounds a plugin that hangs.
const RUN_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// A plugin instance started for one task (see the module docs). When the
/// last reference goes, its process is killed with it.
pub struct TaskWorker {
    plugin_id: String,
    connection: PluginConnection,
    library_sha256: Option<String>,
}

impl TaskWorker {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// The SHA-256 of the library this worker loaded, read once it was
    /// loaded; `None` for an entry point served in this process (tests).
    pub fn library_sha256(&self) -> Option<&str> {
        self.library_sha256.as_deref()
    }

    /// The worker library's `plugin.hello` (verified against the package).
    pub fn hello(&self) -> &lovelycarve_plugin::protocol::Hello {
        self.connection.hello()
    }

    /// Whether the worker can still serve its run.
    pub fn is_alive(&self) -> bool {
        self.connection.is_alive()
    }

    /// Lets the worker exit after its run returned.
    pub async fn shutdown(&self) -> Result<()> {
        self.connection.shutdown().await
    }

    /// Ends the worker now (task cancelled or timed out).
    pub async fn terminate(&self) {
        self.connection.terminate().await;
    }

    /// Runs one analyzer to completion. `context` serves the plugin's host
    /// calls of this run.
    pub async fn run(&self, context: Arc<RunContext>, start: &RunStart) -> Result<RunFinish> {
        if start.run_id != context.run_id() {
            return Err(PluginError::Rpc {
                code: "INVALID_RUN".to_owned(),
                message: "the run context belongs to another run".to_owned(),
            });
        }
        self.connection.attach(Arc::clone(&context));
        // Detached when the run ends, also when the caller stops waiting.
        let _attached = DropGuard::new(start.run_id.as_str(), |run_id| {
            if let Some(context) = self.connection.detach(run_id) {
                context.cancel();
            }
        });
        let params = protocol::to_value(start).map_err(|error| PluginError::Rpc {
            code: "HOST_ERROR".to_owned(),
            message: error.message,
        })?;
        let reply = self
            .connection
            .call_with_timeout(protocol::method::RUN_START, params, RUN_TIMEOUT)
            .await?;
        protocol::from_value(reply).map_err(|error| PluginError::Rpc {
            code: "INVALID_RUN_RESULT".to_owned(),
            message: error.message,
        })
    }
}

impl PluginManager {
    /// Starts a dedicated instance of `plugin_id` for one task.
    pub async fn start_worker(&self, plugin_id: &str) -> Result<TaskWorker> {
        let definition = self.definition(plugin_id)?;
        #[cfg(test)]
        if let Some(entry) = self.loopback_entry(plugin_id) {
            let connection =
                PluginConnection::loopback(entry, &definition, self.sources().clone()).await?;
            return Ok(TaskWorker {
                plugin_id: plugin_id.to_owned(),
                connection,
                library_sha256: None,
            });
        }
        let connection =
            PluginConnection::open(&definition, self.sources().clone(), self.log_store()).await?;
        // The process holds the library open now, so this is the file it
        // runs.
        let digest = tokio::task::spawn_blocking(move || definition.library_sha256())
            .await
            .map_err(|error| PluginError::Unavailable(error.to_string()));
        let library_sha256 = match digest {
            Ok(Ok(digest)) => digest,
            Ok(Err(error)) | Err(error) => {
                connection.terminate().await;
                return Err(error);
            }
        };
        Ok(TaskWorker {
            plugin_id: plugin_id.to_owned(),
            connection,
            library_sha256: Some(library_sha256),
        })
    }
}
