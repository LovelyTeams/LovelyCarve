//! The embedding host seen by commands and workflows: shared state access,
//! event delivery, per-user paths and raw binary results. The types are
//! Tauri-free, so the standalone CLI calls the same command functions as the
//! desktop. Desktop builds adapt them to Tauri's IPC in `host/tauri_adapter.rs`.
use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;

#[cfg(feature = "desktop")]
mod tauri_adapter;
#[cfg(feature = "desktop")]
pub(crate) use tauri_adapter::desktop_handle;
#[cfg(feature = "desktop")]
pub(crate) use tauri_adapter::desktop_supports_update;

/// Borrowed access to state owned by the host for the duration of one command.
/// Mirrors `tauri::State`: it dereferences to `T` and is cheap to copy.
pub struct State<'r, T: Send + Sync + 'static>(&'r T);

impl<'r, T: Send + Sync + 'static> State<'r, T> {
    pub fn new(inner: &'r T) -> Self {
        Self(inner)
    }

    pub fn inner(&self) -> &'r T {
        self.0
    }
}

impl<T: Send + Sync + 'static> Deref for State<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0
    }
}

impl<T: Send + Sync + 'static> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for State<'_, T> {}

/// Delivers one event: its name and the payload already encoded as JSON.
type EventSink = dyn Fn(&str, String) -> Result<(), String> + Send + Sync;

/// Handle to the application embedding the backend. Background work clones it
/// to report events after the originating command has returned. The desktop
/// forwards events to its webviews; the CLI writes them to stderr on request.
#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<HostServices>,
}

struct HostServices {
    app_data_dir: Option<PathBuf>,
    events: Box<EventSink>,
    #[cfg(feature = "desktop")]
    desktop: Option<tauri_adapter::DesktopServices>,
}

/// Why an event could not be delivered. Callers only log it: events report
/// progress, never the outcome of a command.
#[derive(Debug)]
pub enum EmitError {
    Encode(serde_json::Error),
    Deliver(String),
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(formatter, "encode event payload: {error}"),
            Self::Deliver(error) => write!(formatter, "deliver event: {error}"),
        }
    }
}

impl std::error::Error for EmitError {}

impl AppHandle {
    pub fn new(
        app_data_dir: Option<PathBuf>,
        events: impl Fn(&str, String) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(HostServices {
                app_data_dir,
                events: Box::new(events),
                #[cfg(feature = "desktop")]
                desktop: None,
            }),
        }
    }

    /// Encode `payload` once and hand it to the host's event sink.
    pub fn emit<S: Serialize>(&self, event: &str, payload: S) -> Result<(), EmitError> {
        let payload = serde_json::to_string(&payload).map_err(EmitError::Encode)?;
        (self.inner.events)(event, payload).map_err(EmitError::Deliver)
    }

    /// The per-user application data directory (case registry, user plugins).
    /// `None` when the platform reports no data directory.
    pub fn app_data_dir(&self) -> Option<&Path> {
        self.inner.app_data_dir.as_deref()
    }

    /// Only a running desktop host can install and restart an app update.
    pub fn is_desktop(&self) -> bool {
        #[cfg(feature = "desktop")]
        return self.inner.desktop.is_some();
        #[cfg(not(feature = "desktop"))]
        false
    }
}

/// The data directory both the desktop app and the CLI use: the platform data
/// directory joined with the bundle identifier, matching Tauri's
/// `app_data_dir()` for this application.
pub fn default_app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|directory| directory.join(APP_IDENTIFIER))
}

/// Bundle identifier from `tauri.conf.json`, exported by the build script.
pub const APP_IDENTIFIER: &str = env!("LOVELYCARVE_APP_IDENTIFIER");

/// The viewer a leased media preview belongs to: a desktop window, which
/// releases its leases when it closes, or the CLI process, which releases them
/// on exit.
#[derive(Clone)]
pub struct LeaseOwner {
    label: String,
    closed: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl LeaseOwner {
    /// An owner that lives as long as the process.
    pub fn process(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            closed: None,
        }
    }

    /// An owner whose leases are abandoned once `closed` reports true.
    pub fn window(
        label: impl Into<String>,
        closed: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            closed: Some(Arc::new(closed)),
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_closed(&self) -> bool {
        self.closed.as_ref().is_some_and(|closed| closed())
    }
}

/// Raw bytes returned by a command. The desktop sends them as an
/// `ArrayBuffer` instead of a JSON integer array; the CLI encodes them itself.
#[derive(Debug)]
pub struct Response(Vec<u8>);

impl Response {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
