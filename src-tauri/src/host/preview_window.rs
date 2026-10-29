//! Desktop window adapter. Reuse the running WebView2 environment so popping
//! out a preview does not negotiate another browser process for the same profile.
use serde::Deserialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewWindowOptions {
    label: String,
    url: String,
    title: String,
    position: Option<Position>,
    background_color: tauri::window::Color,
}

#[derive(Deserialize)]
struct Position {
    x: f64,
    y: f64,
}

#[tauri::command]
async fn open(app: tauri::AppHandle, options: PreviewWindowOptions) -> Result<(), String> {
    // Window creation pumps Windows messages. Serialize requests across file
    // lists, including different preview labels, to avoid reentrant creation.
    let gate = app.state::<CreationGate>();
    let _creation = gate.0.lock().await;
    if !options.label.starts_with("file-preview-")
        || !options.url.starts_with("index.html?view=file-preview&")
        || options
            .position
            .as_ref()
            .is_some_and(|position| !position.x.is_finite() || !position.y.is_finite())
    {
        return Err("Invalid preview window options".to_owned());
    }
    if let Some(window) = app.get_webview_window(&options.label) {
        window.unminimize().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }

    #[cfg(windows)]
    {
        let main = app
            .get_webview_window("main")
            .ok_or("Main window is closed")?;
        let app = app.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // The native environment stays on its COM apartment. with_webview
        // dispatches to the UI thread; this is not a WebView2 event callback.
        main.with_webview(move |webview| {
            let result = preview_builder(&app, options)
                .with_environment(webview.environment())
                .build()
                .map(|_| ())
                .map_err(|error| error.to_string());
            if sender.send(result).is_err() {
                tracing::warn!("preview window caller disconnected during creation");
            }
        })
        .map_err(|error| error.to_string())?;
        receiver.await.map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    preview_builder(&app, options)
        .build()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn preview_builder(
    app: &tauri::AppHandle,
    options: PreviewWindowOptions,
) -> WebviewWindowBuilder<'_, tauri::Wry, tauri::AppHandle> {
    let builder =
        WebviewWindowBuilder::new(app, options.label, WebviewUrl::App(options.url.into()))
            .title(options.title)
            .inner_size(960.0, 680.0)
            .min_inner_size(520.0, 360.0)
            .resizable(true)
            .decorations(false)
            .background_color(options.background_color);
    if let Some(position) = options.position {
        builder.position(position.x, position.y)
    } else {
        builder
    }
}

struct CreationGate(tokio::sync::Mutex<()>);

pub(crate) fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("preview-window")
        .setup(|app, _| {
            app.manage(CreationGate(tokio::sync::Mutex::new(())));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![open])
        .build()
}
