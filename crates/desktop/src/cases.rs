//! The forensic case session: the active case and the local case registry.
//! Evidence belongs to a case, so the workspace opens and closes evidence
//! tabs when the active case changes.
mod manager;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use lovelycarve_backend::{
    commands::casework::case::{self as case_commands, CaseListItem},
    core::casework::case::{CaseInfo, EvidenceRef},
    host::State,
};

pub use manager::{CaseManager, CaseManagerEvent};

use crate::backend::{Backend, CommandError};

/// A change of the active case.
#[derive(Clone, Debug)]
pub enum CaseEvent {
    /// A case became active (opened, created or switched to).
    Opened(Box<CaseInfo>),
    /// The active case was closed.
    Closed,
}

pub struct CaseStore {
    active: Option<CaseInfo>,
    registered: Vec<CaseListItem>,
    registry_error: Option<String>,
    busy: bool,
}

impl EventEmitter<CaseEvent> for CaseStore {}

struct GlobalCaseStore(Entity<CaseStore>);

impl Global for GlobalCaseStore {}

impl CaseStore {
    pub fn install(cx: &mut App) {
        let store = cx.new(|cx| {
            let mut store = Self {
                active: None,
                registered: Vec::new(),
                registry_error: None,
                busy: false,
            };
            store.refresh_registry(cx);
            store
        });
        cx.set_global(GlobalCaseStore(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCaseStore>().0.clone()
    }

    pub fn active(&self) -> Option<&CaseInfo> {
        self.active.as_ref()
    }

    pub fn registered(&self) -> &[CaseListItem] {
        &self.registered
    }

    pub fn registry_error(&self) -> Option<&str> {
        self.registry_error.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Re-read the local case registry, most recently opened first.
    pub fn refresh_registry(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        cx.spawn(async move |this, cx| {
            let listed = backend
                .run(|host, _| async move { case_commands::case_list(host).await })
                .await;
            this.update(cx, |this, cx| {
                match listed {
                    Ok(cases) => {
                        this.registered = cases;
                        this.registry_error = None;
                    }
                    Err(error) => this.registry_error = Some(error.message.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-read the active case (its evidence and snapshots change as
    /// evidence opens).
    pub fn refresh_active(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        cx.spawn(async move |this, cx| {
            let active = backend
                .run(|_, state| async move { case_commands::case_active(State::new(&state)).await })
                .await;
            match active {
                Ok(active) => {
                    this.update(cx, |this, cx| {
                        this.active = active;
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => tracing::warn!("case_active: {error}"),
            }
        })
        .detach();
    }

    /// Put an evidence row the backend changed into the active case.
    pub fn update_evidence(&mut self, evidence: EvidenceRef, cx: &mut Context<Self>) {
        let Some(case) = self.active.as_mut() else {
            return;
        };
        if let Some(existing) = case
            .evidence
            .iter_mut()
            .find(|existing| existing.image_id == evidence.image_id)
        {
            *existing = evidence;
            cx.notify();
        }
    }

    /// Run a command that yields the new active case, then publish it.
    fn activate<F, Fut>(
        &mut self,
        command: F,
        cx: &mut Context<Self>,
    ) -> Task<Result<CaseInfo, CommandError>>
    where
        F: FnOnce(
                lovelycarve_backend::host::AppHandle,
                std::sync::Arc<lovelycarve_backend::state::AppState>,
            ) -> Fut
            + 'static,
        Fut: std::future::Future<
                Output = Result<CaseInfo, lovelycarve_backend::commands::app_error::AppError>,
            > + Send
            + 'static,
    {
        let backend = Backend::global(cx);
        let run = backend.run(command);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok(info) = &result {
                    this.active = Some(info.clone());
                    cx.emit(CaseEvent::Opened(Box::new(info.clone())));
                }
                this.refresh_registry(cx);
                cx.notify();
            })
            .ok();
            result
        })
    }

    pub fn open(
        &mut self,
        dir: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<CaseInfo, CommandError>> {
        self.activate(
            move |host, state| async move {
                case_commands::case_open(host, State::new(&state), dir).await
            },
            cx,
        )
    }

    pub fn create(
        &mut self,
        dir: String,
        name: String,
        case_number: Option<String>,
        examiner: Option<String>,
        description: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<CaseInfo, CommandError>> {
        self.activate(
            move |host, state| async move {
                case_commands::case_create(
                    host,
                    State::new(&state),
                    dir,
                    name,
                    case_number,
                    description,
                    examiner,
                )
                .await
            },
            cx,
        )
    }

    pub fn close(&mut self, cx: &mut Context<Self>) -> Task<Result<(), CommandError>> {
        let backend = Backend::global(cx);
        let run = backend
            .run(|_, state| async move { case_commands::case_close(State::new(&state)).await });
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if result.is_ok() {
                    this.active = None;
                    cx.emit(CaseEvent::Closed);
                }
                this.refresh_registry(cx);
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// Delete a case's own files (or forget a missing one).
    pub fn remove(
        &mut self,
        dir: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CommandError>> {
        let backend = Backend::global(cx);
        let removed_active = self.active.as_ref().is_some_and(|case| case.dir == dir);
        let run = backend.run(move |host, state| async move {
            case_commands::case_remove(host, State::new(&state), dir).await
        });
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if result.is_ok() && removed_active {
                    this.active = None;
                    cx.emit(CaseEvent::Closed);
                }
                this.refresh_registry(cx);
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// Lock or unlock the active case.
    pub fn set_locked(
        &mut self,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CommandError>> {
        let backend = Backend::global(cx);
        let run = backend.run(move |_, state| async move {
            if locked {
                case_commands::case_lock(State::new(&state)).await
            } else {
                case_commands::case_unlock(State::new(&state)).await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            match result {
                Ok(info) => {
                    this.update(cx, |this, cx| {
                        this.active = Some(info);
                        cx.notify();
                    })
                    .ok();
                    Ok(())
                }
                Err(error) => Err(error),
            }
        })
    }

    /// Unregister an evidence item from the active case.
    pub fn remove_evidence(
        &mut self,
        image_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CommandError>> {
        let backend = Backend::global(cx);
        let run = backend.run(move |_, state| async move {
            case_commands::case_evidence_remove(State::new(&state), image_id).await
        });
        cx.spawn(async move |this, cx| {
            let info = run.await?;
            this.update(cx, |this, cx| {
                this.active = Some(info);
                cx.notify();
            })
            .ok();
            Ok(())
        })
    }
}
