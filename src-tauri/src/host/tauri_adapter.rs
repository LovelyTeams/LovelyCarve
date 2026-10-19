//! Desktop IPC adapters: Tauri resolves the host types as command arguments
//! and sends [`Response`] bytes without JSON encoding.
use tauri::ipc::{CommandArg, CommandItem, InvokeError, InvokeResponseBody, IpcResponse};
use tauri::{Emitter, Manager, Runtime};

use super::{AppHandle, HostServices, LeaseOwner, Response, State};

pub(super) struct DesktopServices {
    app: tauri::AppHandle,
}

pub(crate) fn desktop_supports_update() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
        && !matches!(
            tauri::utils::platform::bundle_type(),
            Some(tauri::utils::config::BundleType::Msi)
        )
}

impl AppHandle {
    pub(crate) fn updater_builder(&self) -> anyhow::Result<tauri_plugin_updater::UpdaterBuilder> {
        use tauri_plugin_updater::UpdaterExt;
        let desktop = self.inner.desktop.as_ref().ok_or_else(|| {
            anyhow::anyhow!("in-app installation requires the desktop application")
        })?;
        Ok(desktop.app.updater_builder())
    }

    pub(crate) fn exit_after_update(&self) {
        if let Some(desktop) = &self.inner.desktop {
            desktop.app.exit(0);
        }
    }
}

impl<'r, 'de: 'r, T: Send + Sync + 'static, R: Runtime> CommandArg<'de, R> for State<'r, T> {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let state = <tauri::State<'r, T> as CommandArg<'de, R>>::from_command(command)?;
        Ok(State::new(state.inner()))
    }
}

impl<'de, R: Runtime> CommandArg<'de, R> for AppHandle {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let handle = <tauri::State<'de, AppHandle> as CommandArg<'de, R>>::from_command(command)?;
        Ok(handle.inner().clone())
    }
}

/// The calling window owns the lease; closing it abandons materialisation.
impl<'de, R: Runtime> CommandArg<'de, R> for LeaseOwner {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let window = <tauri::WebviewWindow<R> as CommandArg<'de, R>>::from_command(command)?;
        let label = window.label().to_owned();
        let app = window.app_handle().clone();
        let watched = label.clone();
        Ok(LeaseOwner::window(label, move || {
            app.get_webview_window(&watched).is_none()
        }))
    }
}

impl IpcResponse for Response {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        Ok(InvokeResponseBody::Raw(self.into_bytes()))
    }
}

/// Build the host handle for a running desktop app: events go to every
/// webview and paths come from Tauri's resolver.
pub(crate) fn desktop_handle(app: &tauri::AppHandle) -> AppHandle {
    let app_data_dir = match app.path().app_data_dir() {
        Ok(directory) => Some(directory),
        Err(error) => {
            tracing::warn!("resolve app data dir: {error}");
            None
        }
    };
    let emitter = app.clone();
    let events = move |event: &str, payload: String| {
        emitter
            .emit_str(event, payload)
            .map_err(|error| error.to_string())
    };
    AppHandle {
        inner: std::sync::Arc::new(HostServices {
            app_data_dir,
            events: Box::new(events),
            desktop: Some(DesktopServices { app: app.clone() }),
        }),
    }
}
