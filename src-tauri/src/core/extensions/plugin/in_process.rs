//! In-process plugin libraries (`isolation = "in_process"`): LovelyCarve
//! loads the library and calls it directly on blocking worker threads.
//!
//! Nothing isolates such a plugin. A crash inside it ends the application,
//! and a thread cannot be killed, so timeouts and task cancellation ask the
//! plugin to cancel and stop waiting. The instance is destroyed only once no
//! call is running inside it; the library itself stays loaded.

use std::{
    collections::HashSet,
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

use lovelycarve_plugin::{
    abi::{self, Buffer, Bytes, EntryFn, HostApiV1},
    loader::{self, PluginInstance},
    value,
    wire::MAX_FRAME_SIZE,
};
use parking_lot::Mutex;
use rmpv::Value;

use super::{
    client::{
        dispatch_host_request_with_cancellation, plugin_error_to_rpc, HostRpcServices, PluginHello,
        PluginProgressSink, PluginSinks,
    },
    PluginError, PluginIdentity, PluginLogLevel, PluginLogStore, Result,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// The host table's context. Leaked on purpose: an abandoned plugin call may
/// still reach the host after its client is gone.
struct HostContext {
    services: HostRpcServices,
    plugin_id: String,
    /// Emptied when the client closes, so the sink of a task worker (and the
    /// task state it captures) does not live on in the leaked context.
    progress_sink: Mutex<Option<PluginProgressSink>>,
    /// The leaked callback context must not keep the writer thread alive.
    logs: Option<Weak<PluginLogStore>>,
    runtime: tokio::runtime::Handle,
    open: AtomicBool,
    cancellation: tokio_util::sync::CancellationToken,
}

pub struct InProcessClient {
    identity: PluginIdentity,
    capabilities: Vec<String>,
    instance: PluginInstance,
    host: &'static HostContext,
    runtime: tokio::runtime::Handle,
    alive: AtomicBool,
    destroyed: AtomicBool,
    in_flight: AtomicUsize,
    next_call_id: AtomicU64,
    active_calls: Mutex<HashSet<u64>>,
}

impl InProcessClient {
    /// Loads `library` and creates an instance. Blocking: runs the
    /// library's initialisers. Call from a thread inside the Tokio runtime.
    pub(super) fn load(
        library: &Path,
        plugin_id: &str,
        services: HostRpcServices,
        sinks: PluginSinks,
    ) -> Result<Arc<Self>> {
        let entry = loader::load_library(library).map_err(abi_error)?;
        Self::create(entry, plugin_id, services, sinks)
    }

    pub(super) fn create(
        entry: EntryFn,
        plugin_id: &str,
        services: HostRpcServices,
        sinks: PluginSinks,
    ) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| PluginError::Unavailable(error.to_string()))?;
        let host: &'static HostContext = Box::leak(Box::new(HostContext {
            services,
            plugin_id: plugin_id.to_owned(),
            progress_sink: Mutex::new(sinks.progress),
            logs: sinks.logs.as_ref().map(Arc::downgrade),
            runtime: runtime.clone(),
            open: AtomicBool::new(true),
            cancellation: tokio_util::sync::CancellationToken::new(),
        }));
        let table: &'static HostApiV1 = Box::leak(Box::new(HostApiV1 {
            abi_version: abi::ABI_VERSION,
            struct_size: abi::HOST_API_SIZE,
            context: (host as *const HostContext).cast_mut().cast(),
            call: host_call,
            free_buffer: host_free_buffer,
            log: host_log,
        }));
        // SAFETY: the table and its context are leaked, so they outlive the
        // instance, and both are callable from any thread.
        let instance = unsafe { PluginInstance::create(entry, table) }.map_err(abi_error)?;
        let hello = match instance
            .describe()
            .map_err(abi_error)
            .and_then(PluginHello::from_params)
        {
            Ok(hello) => hello,
            Err(error) => {
                // SAFETY: no call has been made on the new instance.
                unsafe { instance.destroy() };
                return Err(error);
            }
        };
        Ok(Arc::new(Self {
            identity: hello.plugin,
            capabilities: hello.capabilities,
            instance,
            host,
            runtime,
            alive: AtomicBool::new(true),
            destroyed: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            next_call_id: AtomicU64::new(1),
            active_calls: Mutex::default(),
        }))
    }

    pub fn identity(&self) -> &PluginIdentity {
        &self.identity
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Runs the call on a blocking worker. On timeout, or when the caller
    /// stops waiting, the plugin is asked to cancel.
    pub async fn call_with_timeout(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        let call = self.begin_call()?;
        let mut cancel_on_drop = CancelOnDrop {
            client: Arc::clone(self),
            call_id: call.call_id,
            armed: true,
        };
        let method = method.to_owned();
        let task = tokio::task::spawn_blocking(move || call.invoke(&method, &params));
        let outcome = match tokio::time::timeout(deadline, task).await {
            Ok(joined) => joined.map_err(|error| PluginError::Rpc {
                code: "HOST_TASK_FAILED".to_owned(),
                message: error.to_string(),
            })?,
            Err(_) => return Err(PluginError::Timeout),
        };
        cancel_on_drop.armed = false;
        outcome
    }

    /// Runs the call on the current thread; a timer asks the plugin to
    /// cancel at the deadline.
    pub fn call_blocking_with_timeout(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        let call = self.begin_call()?;
        let timed_out = Arc::new(AtomicBool::new(false));
        let timer = self.runtime.spawn({
            let client = Arc::clone(self);
            let timed_out = Arc::clone(&timed_out);
            let call_id = call.call_id;
            async move {
                tokio::time::sleep(deadline).await;
                timed_out.store(true, Ordering::Release);
                client.instance.cancel(call_id);
            }
        });
        let outcome = call.invoke(method, &params);
        timer.abort();
        match outcome {
            Err(PluginError::Rpc { code, .. })
                if code == "CANCELLED" && timed_out.load(Ordering::Acquire) =>
            {
                Err(PluginError::Timeout)
            }
            outcome => outcome,
        }
    }

    /// Closes the plugin's sessions, then stops new calls, cancels the
    /// running ones and destroys the instance once they return.
    pub async fn shutdown(self: &Arc<Self>) -> Result<()> {
        if !self.is_alive() {
            return Ok(());
        }
        let outcome = self
            .call_with_timeout("plugin.shutdown", Value::Map(Vec::new()), SHUTDOWN_TIMEOUT)
            .await
            .map(drop);
        self.close();
        let started = Instant::now();
        while self.in_flight.load(Ordering::SeqCst) > 0 && started.elapsed() < DRAIN_TIMEOUT {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            tracing::warn!(
                plugin_id = %self.identity.id,
                "in-process plugin calls are still running; the instance is released when they return"
            );
        }
        outcome
    }

    /// Stops the instance without the `plugin.shutdown` call (a task worker
    /// that was cancelled, paused or timed out): new calls fail, running
    /// calls are asked to cancel, and the instance is destroyed once the last
    /// one returns.
    pub fn terminate(&self) {
        self.close();
    }

    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Later host calls fail, so abandoned calls unwind quickly.
        self.host.open.store(false, Ordering::Release);
        self.host.cancellation.cancel();
        *self.host.progress_sink.lock() = None;
        for call_id in self.active_calls.lock().iter() {
            self.instance.cancel(*call_id);
        }
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            self.destroy_instance();
        }
    }

    fn begin_call(self: &Arc<Self>) -> Result<CallGuard> {
        // Paired with `close`: either `close` sees this call in flight, or
        // this call sees that the client is closed.
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        if !self.alive.load(Ordering::SeqCst) {
            self.release_in_flight();
            return Err(PluginError::ConnectionClosed);
        }
        let call_id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        self.active_calls.lock().insert(call_id);
        Ok(CallGuard {
            client: Arc::clone(self),
            call_id,
        })
    }

    fn release_in_flight(&self) {
        if self.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 && !self.alive.load(Ordering::SeqCst)
        {
            self.destroy_instance();
        }
    }

    fn destroy_instance(&self) {
        if !self.destroyed.swap(true, Ordering::SeqCst) {
            // SAFETY: the client is closed and no call is in flight, so the
            // instance is never used again.
            unsafe { self.instance.destroy() };
        }
    }
}

impl Drop for InProcessClient {
    fn drop(&mut self) {
        // Every call holds the client, so none is in flight here.
        self.close();
    }
}

/// One admitted call; releases its in-flight slot when it ends.
struct CallGuard {
    client: Arc<InProcessClient>,
    call_id: u64,
}

impl CallGuard {
    fn invoke(&self, method: &str, params: &Value) -> Result<Value> {
        self.client
            .instance
            .call(self.call_id, method, params, MAX_FRAME_SIZE)
            .map_err(abi_error)
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.client.active_calls.lock().remove(&self.call_id);
        self.client.release_in_flight();
    }
}

/// Cancels a call whose caller stopped waiting (timeout or dropped future).
struct CancelOnDrop {
    client: Arc<InProcessClient>,
    call_id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.client.instance.cancel(self.call_id);
        }
    }
}

fn abi_error(error: lovelycarve_plugin::Error) -> PluginError {
    PluginError::Rpc {
        code: error.code,
        message: error.message,
    }
}

unsafe extern "C" fn host_call(
    context: *mut c_void,
    method: Bytes,
    params: Bytes,
    out: *mut Buffer,
) -> i32 {
    if context.is_null() || out.is_null() {
        return abi::STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: `context` is the leaked `HostContext` of this table.
    let host = unsafe { &*context.cast::<HostContext>() };
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Value> {
        if !host.open.load(Ordering::Acquire) {
            return Err(PluginError::ConnectionClosed);
        }
        let method =
            std::str::from_utf8(unsafe { method.as_slice() }).map_err(|_| PluginError::Rpc {
                code: "INVALID_PARAMS".to_owned(),
                message: "method name is not UTF-8".to_owned(),
            })?;
        let params = value::decode(unsafe { params.as_slice() }).map_err(abi_error)?;
        let _runtime = host.runtime.enter();
        let progress_sink = host.progress_sink.lock().clone();
        dispatch_host_request_with_cancellation(
            method,
            params,
            &host.services,
            &host.plugin_id,
            &progress_sink,
            &host.cancellation,
        )
    }));
    let (status, payload) = match outcome {
        Ok(Ok(result)) => (abi::STATUS_OK, result),
        Ok(Err(error)) => {
            let error = plugin_error_to_rpc(error);
            (
                abi::STATUS_ERROR,
                lovelycarve_plugin::Error::new(error.code, error.message).to_value(),
            )
        }
        Err(_) => (abi::STATUS_PANIC, Value::Nil),
    };
    match value::encode(&payload) {
        Ok(bytes) if bytes.len() <= MAX_FRAME_SIZE => {
            unsafe { out.write(Buffer::from_vec(bytes)) };
            status
        }
        Ok(_) => {
            let error = lovelycarve_plugin::Error::new(
                "RESOURCE_LIMIT",
                "response exceeds the LPP frame limit; use smaller pages or reads",
            );
            match value::encode(&error.to_value()) {
                Ok(bytes) => {
                    unsafe { out.write(Buffer::from_vec(bytes)) };
                    abi::STATUS_ERROR
                }
                Err(_) => abi::STATUS_INVALID_ARGUMENT,
            }
        }
        Err(_) => abi::STATUS_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn host_free_buffer(_context: *mut c_void, buffer: *mut Buffer) {
    if !buffer.is_null() {
        // SAFETY: the buffer was filled by `host_call` from a host vector.
        unsafe { (*buffer).free_vec() };
    }
}

unsafe extern "C" fn host_log(context: *mut c_void, level: u32, message: Bytes) {
    if context.is_null() {
        return;
    }
    let host = unsafe { &*context.cast::<HostContext>() };
    let message = String::from_utf8_lossy(unsafe { message.as_slice() });
    let plugin_id = host.plugin_id.as_str();
    if let Some(logs) = host.logs.as_ref().and_then(Weak::upgrade) {
        logs.record(plugin_id, PluginLogLevel::from_abi(level), &message);
    }
    match level {
        abi::LOG_ERROR => tracing::error!(target: "plugin", %plugin_id, "{message}"),
        abi::LOG_WARN => tracing::warn!(target: "plugin", %plugin_id, "{message}"),
        abi::LOG_INFO => tracing::info!(target: "plugin", %plugin_id, "{message}"),
        _ => tracing::debug!(target: "plugin", %plugin_id, "{message}"),
    }
}

#[cfg(test)]
mod tests {
    use lovelycarve_plugin::{
        abi::PluginApiV1, runtime, AnalysisResult, File, FileAnalyzer, Plugin,
    };

    use super::*;
    use crate::core::{
        extensions::plugin::{EvidenceRegistry, FileHandle, FileRegistry, SourceRegistry},
        filesystem::evidence_session::EvidenceSession,
    };

    const PLUGIN_ID: &str = "org.test.inproc";

    /// Reads the file, waits for cancellation or panics, chosen by name.
    struct Behaviour;

    impl FileAnalyzer for Behaviour {
        fn analyzer(&self) -> &str {
            "test.inproc"
        }

        fn analyze(&self, file: &File) -> lovelycarve_plugin::Result<AnalysisResult> {
            match file.name() {
                "wait" => {
                    while !file.context().is_cancelled() {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(lovelycarve_plugin::Error::cancelled())
                }
                "panic" => panic!("plugin bug"),
                _ => Ok(AnalysisResult {
                    summary: Some(String::from_utf8_lossy(&file.read(0, 64)?).into_owned()),
                    ..AnalysisResult::default()
                }),
            }
        }
    }

    fn behaviour_plugin() -> Plugin {
        Plugin::new(PLUGIN_ID, "In-process test", "1").file_analyzer(Behaviour)
    }

    unsafe extern "C" fn behaviour_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
        unsafe { runtime::entry(host, plugin, behaviour_plugin) }
    }

    static COUNTED_DROPS: AtomicUsize = AtomicUsize::new(0);

    /// Counts instance teardown; used by a single test.
    struct Counted;

    impl Drop for Counted {
        fn drop(&mut self) {
            COUNTED_DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl FileAnalyzer for Counted {
        fn analyzer(&self) -> &str {
            "test.inproc"
        }

        fn analyze(&self, _file: &File) -> lovelycarve_plugin::Result<AnalysisResult> {
            Ok(AnalysisResult::default())
        }
    }

    fn counted_plugin() -> Plugin {
        Plugin::new(PLUGIN_ID, "Counted", "1").file_analyzer(Counted)
    }

    unsafe extern "C" fn counted_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
        unsafe { runtime::entry(host, plugin, counted_plugin) }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        files: FileRegistry,
        image: Arc<EvidenceSession>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temp directory");
            let path = directory.path().join("image.raw");
            std::fs::write(&path, b"hello world!").expect("raw image");
            Self {
                image: Arc::new(EvidenceSession::open(&path).expect("open image")),
                files: FileRegistry::default(),
                _directory: directory,
            }
        }

        fn client(&self, entry: EntryFn) -> Arc<InProcessClient> {
            let services = HostRpcServices::new(
                SourceRegistry::default(),
                self.files.clone(),
                EvidenceRegistry::default(),
                [],
            );
            InProcessClient::create(entry, PLUGIN_ID, services, PluginSinks::default())
                .expect("in-process client")
        }

        fn file(&self, name: &str) -> FileHandle {
            self.files
                .register(
                    PLUGIN_ID,
                    "image".to_owned(),
                    Arc::clone(&self.image),
                    "diskgap:0x0:0xc".to_owned(),
                    name.to_owned(),
                )
                .expect("file handle")
        }
    }

    fn analyze_params(file: &FileHandle) -> Value {
        value::map([
            ("analyzer", Value::from("test.inproc")),
            (
                "file",
                value::map([
                    ("file_id", Value::from(file.file_id.as_str())),
                    ("name", Value::from(file.name.as_str())),
                    ("size", Value::from(file.size)),
                ]),
            ),
        ])
    }

    fn summary(result: &Value) -> Option<&str> {
        value::optional_str(result, "summary")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_reads_go_straight_to_the_host_registries() {
        let fixture = Fixture::new();
        let client = fixture.client(behaviour_entry);
        assert_eq!(client.identity().id, PLUGIN_ID);
        assert_eq!(client.capabilities(), ["file_analyzer"]);
        let result = client
            .call_with_timeout(
                "analyzer.analyze",
                analyze_params(&fixture.file("a.bin")),
                Duration::from_secs(5),
            )
            .await
            .expect("analysis");
        assert_eq!(summary(&result), Some("hello world!"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timeouts_cancel_the_call_and_keep_the_instance() {
        let fixture = Fixture::new();
        let client = fixture.client(behaviour_entry);
        let waiting = client
            .call_with_timeout(
                "analyzer.analyze",
                analyze_params(&fixture.file("wait")),
                Duration::from_millis(50),
            )
            .await;
        assert!(matches!(waiting, Err(PluginError::Timeout)));

        let blocking = Arc::clone(&client);
        let params = analyze_params(&fixture.file("wait"));
        let blocked = tokio::task::spawn_blocking(move || {
            blocking.call_blocking_with_timeout(
                "analyzer.analyze",
                params,
                Duration::from_millis(50),
            )
        })
        .await
        .expect("blocking call");
        assert!(matches!(blocked, Err(PluginError::Timeout)));

        let result = client
            .call_with_timeout(
                "analyzer.analyze",
                analyze_params(&fixture.file("a.bin")),
                Duration::from_secs(5),
            )
            .await
            .expect("instance still serves calls");
        assert_eq!(summary(&result), Some("hello world!"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_panics_become_errors() {
        let fixture = Fixture::new();
        let client = fixture.client(behaviour_entry);
        let panicked = client
            .call_with_timeout(
                "analyzer.analyze",
                analyze_params(&fixture.file("panic")),
                Duration::from_secs(5),
            )
            .await;
        assert!(matches!(panicked, Err(PluginError::Rpc { code, .. }) if code == "PLUGIN_PANIC"));
        assert!(client.is_alive());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_destroys_the_idle_instance() {
        let fixture = Fixture::new();
        let client = fixture.client(counted_entry);
        client
            .call_with_timeout(
                "analyzer.analyze",
                analyze_params(&fixture.file("a.bin")),
                Duration::from_secs(5),
            )
            .await
            .expect("analysis");
        client.shutdown().await.expect("shutdown");
        assert!(!client.is_alive());
        assert_eq!(COUNTED_DROPS.load(Ordering::SeqCst), 1);
        assert!(matches!(
            client
                .call_with_timeout("plugin.shutdown", Value::Nil, Duration::from_secs(1))
                .await,
            Err(PluginError::ConnectionClosed)
        ));
    }
}
