//! The LPP v2 connection to one plugin instance: length-prefixed MessagePack
//! frames over the stdio of an `lc-plugin-host` process, or over in-memory
//! pipes to a plugin entry point served by the SDK bridge (tests).
//!
//! The host sends `run.start`, `plugin.shutdown` and the frozen `fs.*`
//! methods (even request ids); the plugin calls back with `run.*`, `file.*`,
//! `view.*` and the frozen `source.*` methods (odd ids), served on blocking
//! workers by [`host_requests`]. Both directions may be in flight at once,
//! so a plugin can read evidence while the host waits for its run.

mod connection;
mod host_requests;
mod process;
mod router;
use router::*;

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use lovelycarve_plugin::{protocol::Hello, wire::PROTOCOL_VERSION};
use parking_lot::{Mutex, RwLock};
use rmpv::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    process::{Child, Command},
    sync::{mpsc, oneshot, Semaphore},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::{
    codec::{read_frame, write_frame},
    process_guard::ProcessGuard,
    run_context::RunContext,
    LppMessage, PluginError, PluginPermission, Result, RpcError, SourceRegistry,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTBOUND_REQUESTS: usize = 256;
const MAX_INBOUND_REQUESTS: usize = 16;
const STDERR_CHUNK_SIZE: usize = 8 * 1024;

type BoxReader = Box<dyn AsyncRead + Send + Unpin>;
type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;
type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;
type InboundRequests = Arc<Mutex<HashMap<u64, CancellationToken>>>;

#[derive(Clone)]
struct RouterContext {
    pending: PendingRequests,
    outbound: mpsc::Sender<LppMessage>,
    services: HostRpcServices,
    inbound: InboundRequests,
    inbound_slots: Arc<Semaphore>,
}

/// What a connection's plugin may reach on the host: the registered byte
/// sources (filesystem plugins) and the runs attached to it.
#[derive(Clone, Default)]
pub(super) struct HostRpcServices {
    sources: SourceRegistry,
    runs: Arc<RwLock<HashMap<String, Arc<RunContext>>>>,
}

impl HostRpcServices {
    pub(super) fn new(sources: SourceRegistry) -> Self {
        Self {
            sources,
            runs: Arc::default(),
        }
    }

    pub(super) fn attach(&self, run: Arc<RunContext>) {
        self.runs.write().insert(run.run_id().to_owned(), run);
    }

    pub(super) fn detach(&self, run_id: &str) -> Option<Arc<RunContext>> {
        self.runs.write().remove(run_id)
    }

    /// The run that issued `id` (a run id, or a handle `<run>.<n>`).
    fn run(&self, id: &str) -> Result<Arc<RunContext>> {
        let run_id = id.split_once('.').map_or(id, |(run_id, _)| run_id);
        self.runs
            .read()
            .get(run_id)
            .cloned()
            .ok_or_else(|| PluginError::Rpc {
                code: "INVALID_HANDLE".to_owned(),
                message: "the handle belongs to no active run of this plugin".to_owned(),
            })
    }

    /// Cancels the host calls of every attached run.
    fn cancel_runs(&self) {
        for run in self.runs.read().values() {
            run.cancel();
        }
    }
}

/// How to start a plugin process.
#[derive(Clone, Debug)]
pub struct PluginCommand {
    pub plugin_id: String,
    pub program: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    pub working_directory: Option<PathBuf>,
    pub memory_limit: u64,
    pub permissions: Vec<PluginPermission>,
}

pub struct PluginClient {
    hello: Hello,
    services: HostRpcServices,
    outbound: mpsc::Sender<LppMessage>,
    pending: PendingRequests,
    next_request_id: AtomicU64,
    alive: Arc<AtomicBool>,
    runtime: tokio::runtime::Handle,
    child: Option<tokio::sync::Mutex<Child>>,
    outbound_slots: Arc<Semaphore>,
    _process_guard: Option<ProcessGuard>,
    stderr_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl PluginClient {
    /// The plugin's `plugin.hello`.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub(super) fn services(&self) -> &HostRpcServices {
        &self.services
    }

    #[cfg(test)]
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_inner(method, params, None).await
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        self.call_inner(method, params, Some(deadline)).await
    }

    async fn call_inner(
        &self,
        method: &str,
        params: Value,
        deadline: Option<Duration>,
    ) -> Result<Value> {
        if !self.is_alive() {
            return Err(PluginError::ConnectionClosed);
        }
        let _request_slot = Arc::clone(&self.outbound_slots)
            .try_acquire_owned()
            .map_err(|_| {
                PluginError::ResourceLimit(format!(
                    "more than {MAX_OUTBOUND_REQUESTS} host requests are pending"
                ))
            })?;
        let request_id = self.next_request_id.fetch_add(2, Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().insert(request_id, response_tx);
        // A dropped call (cancelled task) asks the plugin to stop it.
        let mut cancel_on_drop = CancelOnDrop {
            outbound: self.outbound.clone(),
            pending: Arc::clone(&self.pending),
            request_id,
            armed: true,
        };
        if self
            .outbound
            .send(LppMessage::request(request_id, method, params))
            .await
            .is_err()
        {
            cancel_on_drop.armed = false;
            self.pending.lock().remove(&request_id);
            return Err(PluginError::ConnectionClosed);
        }
        let outcome = match deadline {
            Some(deadline) => match timeout(deadline, response_rx).await {
                Ok(response) => response.map_err(|_| PluginError::ConnectionClosed)?,
                Err(_) => return Err(PluginError::Timeout),
            },
            None => response_rx
                .await
                .map_err(|_| PluginError::ConnectionClosed)?,
        };
        cancel_on_drop.armed = false;
        outcome
    }

    /// Bridge the existing synchronous `Volume` trait to the async RPC loop.
    /// Filesystem commands already perform blocking disk work on dedicated
    /// worker threads; calling this from a Tokio worker is rejected explicitly.
    pub fn call_blocking_with_timeout(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(PluginError::BlockingOnAsyncRuntime);
        }
        self.runtime
            .block_on(self.call_with_timeout(method, params, deadline))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub async fn shutdown(&self) -> Result<()> {
        let rpc_result = if self.is_alive() {
            self.call_with_timeout(
                lovelycarve_plugin::protocol::method::SHUTDOWN,
                Value::Map(Vec::new()),
                Duration::from_secs(5),
            )
            .await
            .map(|_| ())
        } else {
            Ok(())
        };
        self.alive.store(false, Ordering::Release);
        fail_pending(&self.pending);
        self.services.cancel_runs();

        if let Some(child) = &self.child {
            let mut child = child.lock().await;
            match timeout(Duration::from_secs(2), child.wait()).await {
                Ok(status) => {
                    status?;
                }
                Err(_) => {
                    child.kill().await?;
                    child.wait().await?;
                }
            }
        }
        if let Some(mut task) = self.stderr_task.lock().await.take() {
            match timeout(Duration::from_secs(2), &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(plugin_id = %self.hello.plugin, "plugin stderr task failed: {error}");
                }
                Err(_) => {
                    task.abort();
                    tracing::warn!(plugin_id = %self.hello.plugin, "plugin stderr drain timed out");
                }
            }
        }
        rpc_result
    }

    /// Ends the plugin process at once, without the `plugin.shutdown`
    /// handshake (a task that was cancelled or timed out). Every waiting
    /// call fails with `ConnectionClosed`.
    pub async fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        fail_pending(&self.pending);
        self.services.cancel_runs();
        if let Some(child) = &self.child {
            let mut child = child.lock().await;
            if let Err(error) = child.kill().await {
                tracing::debug!(plugin_id = %self.hello.plugin, "plugin process kill failed: {error}");
            }
        }
    }
}

/// Sends `Cancel` for a host call whose caller stopped waiting (timeout or
/// a dropped future) and forgets its response.
struct CancelOnDrop {
    outbound: mpsc::Sender<LppMessage>,
    pending: PendingRequests,
    request_id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.pending.lock().remove(&self.request_id);
        if self
            .outbound
            .try_send(LppMessage::Cancel {
                v: PROTOCOL_VERSION,
                request_id: self.request_id,
            })
            .is_err()
        {
            tracing::debug!(
                request_id = self.request_id,
                "plugin cancel not sent: the connection is closed or busy"
            );
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
