mod connection;
mod process;
mod router;
use router::*;
mod host_requests;
#[cfg(test)]
pub(super) use host_requests::dispatch_host_request;
pub(super) use host_requests::{dispatch_host_request_with_cancellation, plugin_error_to_rpc};

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use parking_lot::Mutex;
use rmpv::Value;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    process::{Child, Command},
    sync::{mpsc, oneshot, Semaphore},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use lovelycarve_plugin::wire::PROTOCOL_VERSION;

use super::{
    codec::{read_frame, write_frame},
    evidence_registry::{EvidenceNode, EvidenceRoot, MAX_EVIDENCE_READ, MAX_EVIDENCE_READV_RANGES},
    file_registry::{MAX_FILE_READ, MAX_FILE_READV_RANGES},
    process_guard::ProcessGuard,
    EvidenceRegistry, FileHandle, FileRegistry, LppMessage, MaterializedFileLease, PluginError,
    PluginPermission, Result, RpcError, SourceRegistry, MAX_FRAME_SIZE,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OUTBOUND_REQUESTS: usize = 256;
const MAX_INBOUND_REQUESTS: usize = 16;
const STDERR_CHUNK_SIZE: usize = 8 * 1024;

type BoxReader = Box<dyn AsyncRead + Send + Unpin>;
type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;
type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;
type InboundRequests = Arc<Mutex<HashMap<u64, CancellationToken>>>;
pub(super) type PluginProgressSink = Arc<dyn Fn(PluginProgressEvent) + Send + Sync>;

/// Where a running plugin instance reports progress and log lines. Both are
/// optional: an instance without a sink only reaches `tracing`.
#[derive(Clone, Default)]
pub(super) struct PluginSinks {
    pub progress: Option<PluginProgressSink>,
    pub logs: Option<Arc<super::log_file::PluginLogStore>>,
}

#[derive(Clone)]
struct RouterContext {
    pending: PendingRequests,
    outbound: mpsc::Sender<LppMessage>,
    services: HostRpcServices,
    plugin_id: String,
    progress_sink: Option<PluginProgressSink>,
    inbound: InboundRequests,
    inbound_slots: Arc<Semaphore>,
}

#[derive(Clone)]
pub(super) struct HostRpcServices {
    sources: SourceRegistry,
    files: FileRegistry,
    evidences: EvidenceRegistry,
    permissions: Arc<HashSet<PluginPermission>>,
}

impl HostRpcServices {
    pub(super) fn new(
        sources: SourceRegistry,
        files: FileRegistry,
        evidences: EvidenceRegistry,
        permissions: impl IntoIterator<Item = PluginPermission>,
    ) -> Self {
        Self {
            sources,
            files,
            evidences,
            permissions: Arc::new(permissions.into_iter().collect()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginProgressEvent {
    pub plugin_id: String,
    pub current: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PluginCommand {
    pub plugin_id: String,
    pub program: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    pub working_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PluginIdentity {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// `plugin.hello` params: the protocol range, identity and capabilities a
/// plugin advertises (sent by `lc-plugin-host`, returned by an in-process
/// library's `describe`).
#[derive(Debug, Deserialize)]
pub(super) struct PluginHello {
    protocol_min: u16,
    protocol_max: u16,
    pub(super) plugin: PluginIdentity,
    #[serde(default)]
    pub(super) capabilities: Vec<String>,
}

impl PluginHello {
    pub(super) fn from_params(params: Value) -> Result<Self> {
        let hello: Self = rmpv::ext::from_value(params)
            .map_err(|error| PluginError::Handshake(error.to_string()))?;
        if hello.protocol_min > PROTOCOL_VERSION || hello.protocol_max < PROTOCOL_VERSION {
            return Err(PluginError::Handshake(format!(
                "plugin supports LPP {}..{}, host supports {PROTOCOL_VERSION}",
                hello.protocol_min, hello.protocol_max
            )));
        }
        Ok(hello)
    }
}

#[derive(Serialize)]
struct HostHello<'a> {
    protocol: u16,
    host: HostIdentity<'a>,
    limits: HostLimits,
}

#[derive(Serialize)]
struct HostIdentity<'a> {
    name: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct HostLimits {
    max_frame: usize,
    max_read: usize,
    max_readv_ranges: usize,
}

pub struct PluginClient {
    identity: PluginIdentity,
    capabilities: Vec<String>,
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
    pub fn identity(&self) -> &PluginIdentity {
        &self.identity
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

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
        if self
            .outbound
            .send(LppMessage::request(request_id, method, params))
            .await
            .is_err()
        {
            self.pending.lock().remove(&request_id);
            return Err(PluginError::ConnectionClosed);
        }
        match deadline {
            Some(deadline) => match timeout(deadline, response_rx).await {
                Ok(response) => response.map_err(|_| PluginError::ConnectionClosed)?,
                Err(_) => {
                    self.pending.lock().remove(&request_id);
                    if self
                        .outbound
                        .send(LppMessage::Cancel {
                            v: PROTOCOL_VERSION,
                            request_id,
                        })
                        .await
                        .is_err()
                    {
                        self.alive.store(false, Ordering::Release);
                    }
                    Err(PluginError::Timeout)
                }
            },
            None => response_rx
                .await
                .map_err(|_| PluginError::ConnectionClosed)?,
        }
    }

    /// Bridge the existing synchronous `Volume` trait to the async RPC loop.
    /// Filesystem commands already perform blocking disk work on dedicated
    /// worker threads; calling this from a Tokio worker is rejected explicitly.
    pub fn call_blocking(&self, method: &str, params: Value) -> Result<Value> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(PluginError::BlockingOnAsyncRuntime);
        }
        self.runtime.block_on(self.call(method, params))
    }

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
                "plugin.shutdown",
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
                    tracing::warn!(plugin_id = %self.identity.id, "plugin stderr task failed: {error}");
                }
                Err(_) => {
                    task.abort();
                    tracing::warn!(plugin_id = %self.identity.id, "plugin stderr drain timed out");
                }
            }
        }
        rpc_result
    }

    /// Ends the plugin process at once, without the `plugin.shutdown`
    /// handshake (a task worker that was cancelled, paused or timed out).
    /// Every waiting call fails with `ConnectionClosed`.
    pub async fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        fail_pending(&self.pending);
        if let Some(child) = &self.child {
            let mut child = child.lock().await;
            if let Err(error) = child.kill().await {
                tracing::debug!(plugin_id = %self.identity.id, "plugin process kill failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
