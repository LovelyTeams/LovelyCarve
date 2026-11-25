//! Desktop delivery of `lovelycarve://` links
//! (`core::casework::evidence_link`). The system starts the app with a link
//! as its only argument (Windows, Linux) or hands it to the running app
//! (macOS). A process started with a link first offers it to the desktop
//! processes already running (`host::link_relay`); the process that takes
//! it, or this one when none does, queues it for its main window, which takes
//! the queue with `plugin:evidence-links|take` whenever it is told the queue
//! changed.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use serde::Serialize;
use tauri::{Emitter, Manager};

use super::link_relay::{self, Relay, Workspace};
use super::plugin_install_link::PluginInstallLink;
use crate::{
    core::{
        casework::{
            case::CaseState,
            evidence_link::{EvidenceLink, LinkTarget},
        },
        common::ids::{ImageId, NodeId},
    },
    state::AppState,
};

/// Tells the main window that links are waiting.
const RECEIVED_EVENT: &str = "evidence-link-received";
/// Links waiting for the main window; the oldest give way.
const QUEUE_LIMIT: usize = 16;

#[derive(Default)]
struct Inbox(Mutex<VecDeque<String>>);

impl Inbox {
    fn push(&self, url: String) {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() >= QUEUE_LIMIT {
            queue.pop_front();
        }
        queue.push_back(url);
    }

    fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

/// The relay record this process publishes until it exits.
#[derive(Default)]
struct Published(Mutex<Option<Relay>>);

/// A link as the main window receives it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Received {
    url: String,
    link: Option<Place>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_install: Option<PluginInstallLink>,
    error: Option<String>,
}

/// The fields of a valid link, as the frontend locates a file.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Place {
    case_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<ImageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_id: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    length: Option<u64>,
}

impl Received {
    fn of(url: String) -> Self {
        if PluginInstallLink::matches(&url) {
            let (plugin_install, error) = match PluginInstallLink::parse(&url) {
                Ok(install) => (Some(install), None),
                Err(error) => (None, Some(error.to_string())),
            };
            return Self {
                url,
                link: None,
                plugin_install,
                error,
            };
        }
        match EvidenceLink::parse(&url) {
            Ok(link) => Self {
                link: Some(Place::of(link)),
                plugin_install: None,
                error: None,
                url,
            },
            Err(error) => Self {
                link: None,
                plugin_install: None,
                error: Some(error.to_string()),
                url,
            },
        }
    }
}

impl Place {
    fn of(link: EvidenceLink) -> Self {
        let mut place = Self {
            case_id: link.case_id().to_owned(),
            image_id: None,
            node_id: None,
            root_id: None,
            path: None,
            offset: None,
            length: None,
        };
        match link.target().clone() {
            LinkTarget::Case => {}
            LinkTarget::Evidence { image_id } => place.image_id = Some(image_id),
            LinkTarget::Node {
                image_id,
                node_id,
                range,
            } => {
                place.image_id = Some(image_id);
                place.node_id = Some(node_id);
                place.offset = range.map(|range| range.offset);
                place.length = range.map(|range| range.length);
            }
            LinkTarget::Source {
                image_id,
                root_id,
                path,
            } => {
                place.image_id = Some(image_id);
                place.root_id = Some(root_id);
                place.path = Some(path);
            }
        }
        place
    }
}

#[tauri::command]
fn take(inbox: tauri::State<'_, Arc<Inbox>>) -> Vec<Received> {
    inbox.take().into_iter().map(Received::of).collect()
}

pub(crate) fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("evidence-links")
        .setup(|app, _| {
            let inbox = Arc::new(Inbox::default());
            // `lib::run` already offered this link to the running processes.
            if let Some(url) = link_relay::launch_link() {
                inbox.push(url);
            }
            app.manage(inbox);
            app.manage(Published::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![take])
        .build()
}

/// Whether this process was started with a link that a running desktop
/// process took; the process then has nothing left to do.
pub(crate) fn handed_over() -> bool {
    let Some(url) = link_relay::launch_link() else {
        return false;
    };
    link_relay::directory().is_some_and(|directory| link_relay::forward(&directory, &url))
}

/// Once every plugin is set up: point the scheme at this executable and take
/// links from other desktop processes and from macOS.
pub(crate) fn start(app: &tauri::AppHandle) {
    let Some(inbox) = app
        .try_state::<Arc<Inbox>>()
        .map(|inbox| Arc::clone(&inbox))
    else {
        tracing::warn!("evidence links are unavailable: the plugin is not registered");
        return;
    };
    register_scheme(app);
    publish(app, &inbox);
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_deep_link::DeepLinkExt as _;
        let handle = app.clone();
        app.deep_link().on_open_url(move |event| {
            for url in event.urls() {
                deliver(&handle, &inbox, url.to_string());
            }
        });
    }
}

/// Withdraw this process's relay record; the app is exiting.
pub(crate) fn stop(app: &tauri::AppHandle) {
    if let Some(published) = app.try_state::<Published>() {
        drop(
            published
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }
}

/// Installers register the scheme; portable and development builds register
/// it for the current user when it points elsewhere.
#[cfg(any(windows, target_os = "linux"))]
fn register_scheme(app: &tauri::AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt as _;
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("evidence-link-scheme".into())
        .spawn(move || {
            let links = app.deep_link();
            match links.is_registered(crate::core::casework::evidence_link::SCHEME) {
                Ok(true) => return,
                Ok(false) => {}
                Err(error) => tracing::debug!("check the lovelycarve:// registration: {error}"),
            }
            if let Err(error) = links.register_all() {
                tracing::warn!("register lovelycarve:// links: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("register lovelycarve:// links: {error}");
    }
}

/// The application bundle declares the scheme.
#[cfg(not(any(windows, target_os = "linux")))]
fn register_scheme(_app: &tauri::AppHandle) {}

fn publish(app: &tauri::AppHandle, inbox: &Arc<Inbox>) {
    let Some(directory) = link_relay::directory() else {
        tracing::warn!("links from other processes are unavailable: no data directory");
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        tracing::warn!("links from other processes are unavailable: no application state");
        return;
    };
    let active_case = Arc::clone(&state.active_case);
    let handle = app.clone();
    let queue = Arc::clone(inbox);
    let relay = Relay::start(
        &directory,
        move || workspace(&active_case),
        move |url| deliver(&handle, &queue, url),
    );
    match (relay, app.try_state::<Published>()) {
        (Ok(relay), Some(published)) => {
            *published.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(relay);
        }
        (Ok(_), None) => tracing::warn!("links from other processes are unavailable"),
        (Err(error), _) => tracing::warn!("links from other processes are unavailable: {error}"),
    }
}

/// What this process has open, as a link offer asks. Opening or closing a
/// case holds the lock for a moment; offers wait briefly before giving up.
fn workspace(active_case: &tokio::sync::Mutex<Option<CaseState>>) -> Workspace {
    for _ in 0..20 {
        if let Ok(case) = active_case.try_lock() {
            return case.as_ref().map_or(Workspace::Empty, |case| {
                Workspace::Case(case.profile().case_uid.clone())
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Workspace::Busy
}

fn deliver(app: &tauri::AppHandle, inbox: &Inbox, url: String) {
    inbox.push(url);
    if let Err(error) = app.emit_to("main", RECEIVED_EVENT, ()) {
        tracing::warn!("announce a received link: {error}");
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    for (step, result) in [
        ("unminimize", window.unminimize()),
        ("show", window.show()),
        ("focus", window.set_focus()),
    ] {
        if let Err(error) = result {
            tracing::debug!("bring the main window forward ({step}): {error}");
        }
    }
}
