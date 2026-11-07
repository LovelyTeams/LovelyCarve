//! LovelyCarve's native desktop app, built with GPUI Kit.
//!
//! It links the Tauri-free backend like the CLI does: the backend runs
//! in-process on its own tokio runtime and views call the command functions
//! directly (see `backend.rs`); backend events reach the UI thread through
//! the event hub. Cases, evidence tabs, the task center and the pages
//! (image information, files, search, timeline, recovery and analysis
//! results) mirror the Tauri frontend; the native-type preview is not part
//! of this app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod assets;
mod backend;
mod bcp;
mod carving;
mod cases;
mod events;
mod evidence;
mod files;
mod format;
mod prefs;
mod raid;
mod search;
mod settings;
mod tasks;
mod theme;
mod timeline;
mod ui;
mod workspace;

use gpui_kit::{
    component::TitleBar, px, size, AppContext as _, Bounds, Focusable as _, WindowBounds,
    WindowOptions,
};
use std::borrow::Cow;

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    if let Err(error) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("tracing unavailable: {error}");
    }
}

fn main() {
    init_tracing();
    let (backend, events) = match backend::Backend::start() {
        Ok(started) => started,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let app_backend = backend.clone();
    let launch = workspace::Launch::from_args(std::env::args().skip(1));
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            gpui_kit::component::set_locale("zh-CN");
            if let Err(error) = cx
                .text_system()
                .add_fonts(vec![Cow::Borrowed(include_bytes!(
                    "../../../../src/assets/fonts/Quicksand-700.ttf"
                ))])
            {
                tracing::warn!("brand font unavailable: {error}");
            }
            cx.set_global(prefs::Preferences::load());
            cx.set_global(app_backend);
            events::EventHub::install(events, cx);
            tasks::TaskStore::install(cx);
            cases::CaseStore::install(cx);
            raid::RaidStore::install(cx);
            evidence::PlatformStore::install(cx);
            workspace::init(cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(600.))),
                ..TitleBar::window_options()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                window.set_window_title("LovelyCarve");
                prefs::apply_theme(window, cx);
                window
                    .observe_window_appearance(|window, cx| {
                        if prefs::Preferences::global(cx).theme == prefs::ThemeChoice::System {
                            prefs::apply_theme(window, cx);
                        }
                    })
                    .detach();
                let workspace = cx.new(|cx| workspace::Workspace::new(window, cx));
                workspace.focus_handle(cx).focus(window, cx);
                workspace.update(cx, |workspace, cx| workspace.launch(launch, window, cx));
                workspace
            });
            if let Err(error) = opened {
                eprintln!("无法打开窗口：{error}");
                cx.quit();
                return;
            }
            // Closing the main window ends the application.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    backend.shutdown();
}
