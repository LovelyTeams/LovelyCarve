//! The in-process backend: `AppState`, the host handle commands receive, a
//! tokio runtime for the async command functions and the channel that carries
//! backend events to the UI thread.
//!
//! Command functions are async and expect a tokio runtime; GPUI runs its own
//! executors. [`Backend::run`] spawns a command on the backend runtime and
//! returns a future any executor can await, so views call commands from
//! `cx.spawn` without blocking the UI thread.
use std::{future::Future, sync::Arc, time::Duration};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use gpui_kit::{App, Global, SharedString};
use lovelycarve_lib::{
    commands::app_error::{AppError, ErrorCode},
    host::AppHandle,
    state::AppState,
};

/// How long shutdown may take before the process exits regardless.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// One backend event: its name and its JSON payload.
#[derive(Clone, Debug)]
pub struct BackendEvent {
    pub name: String,
    pub payload: serde_json::Value,
}

/// A failed command as the interface reports it.
#[derive(Clone, Debug)]
pub struct CommandError {
    pub message: SharedString,
    pub cancelled: bool,
}

impl CommandError {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
        }
    }
}

impl From<AppError> for CommandError {
    fn from(error: AppError) -> Self {
        Self {
            cancelled: matches!(error.code, ErrorCode::Cancelled),
            message: error.message.into(),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct Inner {
    runtime: tokio::runtime::Runtime,
    state: Arc<AppState>,
    host: AppHandle,
}

/// A cheap handle to the backend, stored as a GPUI global.
#[derive(Clone)]
pub struct Backend(Arc<Inner>);

impl Global for Backend {}

impl Backend {
    /// Start the runtime, build the state and prepare it for commands.
    /// Backend events arrive on the returned receiver.
    pub fn start() -> Result<(Self, UnboundedReceiver<BackendEvent>), String> {
        // Deep parser recursion needs more than the default worker stack, as
        // in the CLI's backend thread.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("lovelycarve-backend")
            .thread_stack_size(16 * 1024 * 1024)
            .build()
            .map_err(|error| format!("无法启动后台运行时：{error}"))?;
        let (sender, receiver) = unbounded();
        let host = AppHandle::new(
            lovelycarve_lib::host::default_app_data_dir(),
            move |name, payload| {
                let payload = serde_json::from_str(&payload).map_err(|error| error.to_string())?;
                sender
                    .unbounded_send(BackendEvent {
                        name: name.to_owned(),
                        payload,
                    })
                    .map_err(|error| error.to_string())
            },
        );
        let state = Arc::new(AppState::new());
        {
            // Initialization registers runtime-bound event bridges.
            let _context = runtime.enter();
            lovelycarve_lib::embed::initialize(&state, &host)
                .map_err(|error| format!("后台初始化失败：{error}"))?;
        }
        Ok((
            Self(Arc::new(Inner {
                runtime,
                state,
                host,
            })),
            receiver,
        ))
    }

    pub fn global(cx: &App) -> Self {
        cx.global::<Self>().clone()
    }

    /// Run a command on the backend runtime. The returned future resolves
    /// with the command's result and can be awaited on any executor.
    pub fn run<T, F, Fut>(
        &self,
        command: F,
    ) -> impl Future<Output = Result<T, CommandError>> + use<T, F, Fut>
    where
        F: FnOnce(AppHandle, Arc<AppState>) -> Fut + 'static,
        Fut: Future<Output = Result<T, AppError>> + Send + 'static,
        T: Send + 'static,
    {
        let task = self
            .0
            .runtime
            .spawn(command(self.0.host.clone(), Arc::clone(&self.0.state)));
        async move {
            match task.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(CommandError::from(error)),
                Err(error) => Err(CommandError::new(format!("后台任务异常退出：{error}"))),
            }
        }
    }

    /// Stop the backend in order before the process exits. Blocks the
    /// calling thread for at most [`SHUTDOWN_TIMEOUT`].
    pub fn shutdown(&self) {
        let state = Arc::clone(&self.0.state);
        let finished = self.0.runtime.block_on(async move {
            tokio::time::timeout(SHUTDOWN_TIMEOUT, lovelycarve_lib::embed::shutdown(&state)).await
        });
        if finished.is_err() {
            tracing::warn!("backend shutdown timed out after {SHUTDOWN_TIMEOUT:?}");
        }
    }
}
