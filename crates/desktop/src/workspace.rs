//! The main window: a title bar with the evidence tabs and the case, task
//! and settings entries, and below it the active evidence tab or the
//! welcome screen. Evidence always belongs to the active case: opening or
//! switching a case replaces the tabs with that case's evidence.
mod sources;
mod welcome;
mod welcome_motion;

use std::{path::PathBuf, time::Duration};

use gpui_kit::{
    actions,
    component::{
        badge::Badge,
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        tab::{Tab, TabBar},
        v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, TitleBar,
        WindowExt as _,
    },
    div, img,
    prelude::FluentBuilder as _,
    px, Animation, AnimationExt as _, App, AppContext as _, Context, Div, Entity, ExternalPaths,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _,
    Render, Styled as _, Subscription, Task, Window,
};

use crate::{
    cases::{CaseEvent, CaseManager, CaseManagerEvent, CaseStore},
    evidence::{EvidenceTab, EvidenceTabEvent},
    settings,
    tasks::{
        self,
        center::{HubEvent, TaskCenter},
        TaskNotice, TaskStore,
    },
    ui,
};
use welcome::Welcome;

type ExtraIcon = gpui_kit::assets::IconName;

actions!(
    lovelycarve,
    [
        AddEvidence,
        CloseTab,
        NextTab,
        PreviousTab,
        ShowCaseManager,
        ShowTaskCenter
    ]
);

const CONTEXT: &str = "Workspace";
/// Where the task hub card rests under the title bar, and how far above it
/// the card starts its drop.
const HUB_TOP: f32 = 44.;
const HUB_DROP: f32 = 8.;

pub fn init(cx: &mut App) {
    tasks::center::init(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-o", AddEvidence, Some(CONTEXT)),
        KeyBinding::new("ctrl-t", AddEvidence, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(CONTEXT)),
    ]);
}

pub struct Workspace {
    tabs: Vec<Entity<EvidenceTab>>,
    active: Option<usize>,
    welcome: Entity<Welcome>,
    cases: Entity<CaseStore>,
    tasks: Entity<TaskStore>,
    task_hub: Option<(Entity<TaskCenter>, Subscription)>,
    /// A task hub that was put away and is still fading out.
    closing_hub: Option<(Entity<TaskCenter>, Task<()>)>,
    focus: FocusHandle,
    /// Evidence asked for before a case was active (dropped files, the
    /// command line); opened once a case is.
    pending_paths: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

/// What the command line asks the window to open:
/// `LovelyCarve [--case <dir>] [<evidence>…]`.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// A case directory to open; one without a case ledger becomes a new
    /// case named after the directory.
    pub case: Option<PathBuf>,
    pub evidence: Vec<String>,
}

impl Launch {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut launch = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--case" => launch.case = args.next().map(PathBuf::from),
                _ => launch.evidence.push(arg),
            }
        }
        launch
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cases = CaseStore::global(cx);
        let tasks = TaskStore::global(cx);
        let welcome = cx.new(|cx| Welcome::new(window, cx));
        let subscriptions = vec![
            cx.observe(&cases, |_, _, cx| cx.notify()),
            cx.observe(&tasks, |_, _, cx| cx.notify()),
            cx.subscribe_in(&cases, window, |this, _, event: &CaseEvent, window, cx| {
                this.on_case_event(event, window, cx)
            }),
            cx.subscribe_in(&tasks, window, |_, _, notice: &TaskNotice, window, cx| {
                if notice.success {
                    ui::notify_success(window, cx, notice.title.clone(), notice.message.clone());
                } else {
                    ui::notify_error(window, cx, notice.title.clone(), notice.message.clone());
                }
            }),
            cx.subscribe_in(
                &welcome,
                window,
                |this, _, event: &welcome::WelcomeEvent, window, cx| match event {
                    welcome::WelcomeEvent::AddEvidence => this.add_evidence(window, cx),
                    welcome::WelcomeEvent::ShowCaseManager => this.show_case_manager(window, cx),
                },
            ),
        ];
        Self {
            tabs: Vec::new(),
            active: None,
            welcome,
            cases,
            tasks,
            task_hub: None,
            closing_hub: None,
            focus: cx.focus_handle(),
            pending_paths: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Open what the command line names: the case first, then the evidence.
    pub fn launch(&mut self, launch: Launch, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_paths.extend(launch.evidence);
        match launch.case {
            Some(directory) => {
                let ledger =
                    directory.join(lovelycarve_backend::core::casework::case_paths::LEDGER_FILE);
                let dir = directory.display().to_string();
                let task = if ledger.exists() {
                    self.cases.update(cx, |cases, cx| cases.open(dir, cx))
                } else {
                    if let Err(error) = std::fs::create_dir_all(&directory) {
                        ui::notify_error(window, cx, "无法创建案件目录", error.to_string());
                        return;
                    }
                    let name = crate::format::file_name(&dir).to_owned();
                    self.cases.update(cx, |cases, cx| {
                        cases.create(dir, name, None, None, None, cx)
                    })
                };
                cx.spawn_in(window, async move |this, cx| {
                    if let Err(error) = task.await {
                        this.update_in(cx, |_, window, cx| {
                            ui::notify_error(window, cx, "打开案件失败", error.message)
                        })
                        .ok();
                    }
                })
                .detach();
            }
            None => self.open_pending(window, cx),
        }
    }

    /// Open evidence asked for earlier, or ask for a case first.
    fn open_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_paths.is_empty() {
            return;
        }
        if self.cases.read(cx).active().is_none() {
            ui::notify_error(
                window,
                cx,
                "请先创建或打开案件",
                "检材会登记到当前案件；打开案件后将继续打开所选检材。",
            );
            self.show_case_manager(window, cx);
            return;
        }
        for path in std::mem::take(&mut self.pending_paths) {
            self.open_tab(path, window, cx);
        }
    }

    fn on_drop(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_paths
            .extend(paths.paths().iter().map(|path| path.display().to_string()));
        self.open_pending(window, cx);
    }

    fn on_case_event(&mut self, event: &CaseEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Switching or closing a case unloads the previous case's evidence
        // in the backend; its tabs and RAID findings go with it.
        self.tabs.clear();
        self.active = None;
        self.task_hub = None;
        self.closing_hub = None;
        crate::raid::RaidStore::global(cx).update(cx, |store, cx| store.reset(cx));
        crate::evidence::PlatformStore::global(cx).update(cx, |store, cx| store.reset(cx));
        if let CaseEvent::Opened(case) = event {
            let mut seen = std::collections::HashSet::new();
            for evidence in &case.evidence {
                if seen.insert(evidence.path.clone()) {
                    self.open_tab(evidence.path.clone(), window, cx);
                }
            }
            if let Some(first) = (!self.tabs.is_empty()).then_some(0) {
                self.active = Some(first);
            }
            self.open_pending(window, cx);
        }
        cx.notify();
    }

    /// Open `path` as evidence of the active case, or focus its tab.
    pub fn open_tab(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        use lovelycarve_backend::core::casework::case::evidence_path_key;
        let path_key = evidence_path_key(&path);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| evidence_path_key(tab.read(cx).path()) == path_key)
        {
            self.active = Some(index);
            cx.notify();
            return;
        }
        let case_dir = self.cases.read(cx).active().map(|case| case.dir.clone());
        let tab = cx.new(|cx| EvidenceTab::new(path, case_dir, window, cx));
        let subscription = cx.subscribe_in(
            &tab,
            window,
            |this, _, event: &EvidenceTabEvent, window, cx| {
                match event {
                    EvidenceTabEvent::Loaded => {
                        // The backend registered the evidence with the case.
                        this.cases.update(cx, |cases, cx| cases.refresh_active(cx));
                        cx.notify();
                    }
                    EvidenceTabEvent::Failed(message) => {
                        ui::notify_error(window, cx, "打开检材失败", message.clone());
                    }
                    EvidenceTabEvent::OpenImage(path) => this.open_tab(path.clone(), window, cx),
                    EvidenceTabEvent::QuickAnalysis => this.show_task_hub(true, window, cx),
                }
            },
        );
        self._subscriptions.push(subscription);
        self.tabs.push(tab);
        self.active = Some(self.tabs.len() - 1);
        cx.notify();
    }

    fn open_bcp_tab(&mut self, folder: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(folder, window, cx);
        if let Some(tab) = self.active.and_then(|index| self.tabs.get(index)) {
            tab.update(cx, |tab, cx| tab.open_bcp(window, cx));
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = Some(index);
            cx.notify();
        }
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            other => other,
        };
        cx.notify();
    }

    fn cycle(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if count < 2 {
            return;
        }
        let current = self.active.unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(count as isize) as usize;
        self.activate(next, cx);
    }

    fn show_task_hub(&mut self, quick: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((hub, _)) = &self.task_hub {
            if quick {
                hub.update(cx, TaskCenter::show_quick);
                hub.focus_handle(cx).focus(window, cx);
            } else {
                self.close_task_hub(window, cx);
            }
            cx.notify();
            return;
        }
        self.closing_hub = None;
        let hub = cx.new(|cx| TaskCenter::new(quick, window, cx));
        let subscription =
            cx.subscribe_in(&hub, window, |this, _, event: &HubEvent, window, cx| {
                this.close_task_hub(window, cx);
                match event {
                    HubEvent::Close => {}
                    HubEvent::OpenCases => this.show_case_manager(window, cx),
                    HubEvent::OpenPage(page) => {
                        if let Some(tab) = this.active.and_then(|index| this.tabs.get(index)) {
                            tab.update(cx, |tab, cx| tab.show_page(*page, window, cx));
                        }
                    }
                }
                cx.notify();
            });
        hub.focus_handle(cx).focus(window, cx);
        self.task_hub = Some((hub, subscription));
        cx.notify();
    }

    /// Put the task hub away: it fades out, then leaves the window.
    fn close_task_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hub, _)) = self.task_hub.take() else {
            return;
        };
        self.focus.focus(window, cx);
        let exit = cx.theme().motion_tokens().duration_fast;
        let removal = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(exit).await;
            this.update(cx, |this, cx| {
                this.closing_hub = None;
                cx.notify();
            })
            .ok();
        });
        self.closing_hub = Some((hub, removal));
        cx.notify();
    }

    /// The floating card the task hub lives in, under the title bar.
    fn hub_card(hub: &Entity<TaskCenter>, window: &Window, cx: &App) -> Div {
        let height = (window.viewport_size().height - px(84.)).min(px(680.));
        ui::card(cx)
            .absolute()
            .top(px(HUB_TOP))
            .right(px(12.))
            .w(px(440.))
            .h(height)
            .shadow_lg()
            .overflow_hidden()
            .child(hub.clone())
    }

    /// Ask for the evidence source; evidence needs an active, unlocked case.
    pub fn add_evidence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let case = self.cases.read(cx).active().cloned();
        match case {
            None => {
                ui::notify_error(
                    window,
                    cx,
                    "请先创建或打开案件",
                    "检材会登记到当前案件，卷快照、标签和哈希结果也会随案件保存。",
                );
                self.show_case_manager(window, cx);
            }
            Some(case) if case.locked => {
                ui::notify_error(window, cx, "案件已锁定", "解锁案件后才能添加检材。");
            }
            Some(_) => sources::open(cx.entity().downgrade(), window, cx),
        }
    }

    pub fn show_case_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current_image = self
            .active
            .and_then(|index| self.tabs.get(index))
            .and_then(|tab| tab.read(cx).image_id().map(str::to_owned));
        let manager = cx.new(|cx| CaseManager::new(current_image, cx));
        let subscription = cx.subscribe_in(
            &manager,
            window,
            |this, _, event: &CaseManagerEvent, window, cx| match event {
                CaseManagerEvent::AddEvidence => {
                    window.close_dialog(cx);
                    this.add_evidence(window, cx);
                }
                CaseManagerEvent::OpenEvidence { path } => {
                    window.close_dialog(cx);
                    this.open_tab(path.clone(), window, cx);
                }
            },
        );
        self._subscriptions.push(subscription);
        let cases = self.cases.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let (title, subtitle) = cases.read(cx).active().map_or_else(
                || ("案件管理".to_owned(), "新建案件，或继续已有工作".to_owned()),
                |case| (case.name.clone(), format!("案件 · {}", case.case_number)),
            );
            dialog
                .centered()
                .title(ui::dialog_heading(
                    ExtraIcon::Briefcase,
                    title,
                    subtitle,
                    cx,
                ))
                .w(gpui_kit::px(760.))
                .child(manager.clone())
        });
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tasks = self.tasks.read(cx).active_count();
        let case_name = self.cases.read(cx).active().map(|case| case.name.clone());
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let tab = tab.read(cx);
            let loading = tab.is_loading();
            Tab::new()
                .label(tab.name())
                .prefix(div().pl_2().child(if loading {
                    Spinner::new().xsmall().into_any_element()
                } else {
                    Icon::new(IconName::HardDrive).small().into_any_element()
                }))
                .suffix(
                    Button::new(("close-tab", index))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label("关闭标签页")
                        .tooltip("关闭标签页 (Ctrl+W)")
                        .on_click(cx.listener(move |this, _, _, cx| this.close_tab(index, cx))),
                )
        });
        TitleBar::new()
            .h(px(38.))
            .bg(cx.theme().title_bar)
            .child(
                // The tabs take what the actions leave and scroll beyond it,
                // so the case, settings and window buttons stay visible.
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_2()
                            .pr_2()
                            .child(img(crate::assets::BRAND_MARK).size(px(20.)))
                            .child(ui::wordmark(13., cx)),
                    )
                    .when(!self.tabs.is_empty(), |this| {
                        this.child(
                            // Keep the native drag hitbox behind the interactive tabs.
                            div().occlude().min_w_0().child(
                                TabBar::new("evidence-tabs")
                                    .segmented()
                                    .small()
                                    .bg(cx.theme().transparent)
                                    .min_w_0()
                                    .max_width(px(200.))
                                    .children(tabs)
                                    .when_some(self.active, |bar, active| {
                                        bar.selected_index(active)
                                    })
                                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                                        this.activate(*index, cx)
                                    })),
                            ),
                        )
                    })
                    .child(
                        div().occlude().flex_shrink_0().child(
                            Button::new("add-evidence")
                                .ghost()
                                .small()
                                .icon(IconName::Plus)
                                .accessibility_label("添加检材")
                                .tooltip("添加检材 (Ctrl+O)")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.add_evidence(window, cx)
                                    }),
                                ),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .occlude()
                    .flex_shrink_0()
                    .gap_1()
                    .pr_2()
                    .child(
                        Badge::new().count(active_tasks).child(
                            Button::new("task-center")
                                .ghost()
                                .small()
                                .h(px(26.))
                                .selected(self.task_hub.is_some())
                                .icon(Icon::new(ExtraIcon::ListTodo))
                                .label("任务中心")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_task_hub(false, window, cx)
                                })),
                        ),
                    )
                    .child(
                        Button::new("case-manager")
                            .ghost()
                            .small()
                            .h(px(26.))
                            .icon(Icon::new(ExtraIcon::Briefcase))
                            .label("案件管理")
                            .tooltip(match &case_name {
                                Some(name) => format!("案件管理 · {name}"),
                                None => "案件管理（当前无案件）".to_owned(),
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_case_manager(window, cx)
                            })),
                    )
                    .child(
                        Button::new("settings")
                            .ghost()
                            .small()
                            .h(px(26.))
                            .icon(IconName::Settings)
                            .label("设置中心")
                            .on_click(cx.listener(|_, _, window, cx| settings::open(window, cx))),
                    ),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active.and_then(|index| self.tabs.get(index)) {
            Some(tab) => tab.clone().into_any_element(),
            None => ui::workspace_sheet(cx)
                .mx(px(8.))
                .mb(px(8.))
                .child(self.welcome.clone())
                .into_any_element(),
        };
        let task_hub = self.task_hub.as_ref().map(|(hub, _)| {
            let evidence = self
                .active
                .and_then(|index| self.tabs.get(index))
                .and_then(|tab| {
                    let tab = tab.read(cx);
                    Some((tab.image_id()?.to_owned(), tab.name()))
                });
            let has_case = self.cases.read(cx).active().is_some();
            hub.update(cx, |hub, cx| hub.set_context(evidence, has_case, cx));
            // The hub drops in from under the title bar.
            gpui_kit::deferred(Self::hub_card(hub, window, cx).occlude().with_animation(
                "task-hub-enter",
                crate::motion::entrance(Duration::ZERO, cx),
                |card, progress| {
                    card.top(px(HUB_TOP - HUB_DROP * (1. - progress)))
                        .opacity(progress)
                },
            ))
        });
        let closing_hub = self.closing_hub.as_ref().map(|(hub, _)| {
            let tokens = cx.theme().motion_tokens();
            let curve = tokens.easing_exit.clone();
            let exit =
                Animation::new(tokens.duration_fast).with_easing(move |phase| curve.sample(phase));
            gpui_kit::deferred(Self::hub_card(hub, window, cx).with_animation(
                "task-hub-exit",
                exit,
                |card, progress| {
                    card.top(px(HUB_TOP - HUB_DROP * progress))
                        .opacity(1. - progress)
                },
            ))
        });
        let workspace = v_flex()
            .id("workspace")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &AddEvidence, window, cx| this.add_evidence(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                if let Some(active) = this.active {
                    this.close_tab(active, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.cycle(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.cycle(-1, cx)))
            .on_action(cx.listener(|this, _: &ShowCaseManager, window, cx| {
                this.show_case_manager(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTaskCenter, window, cx| {
                this.show_task_hub(false, window, cx)
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.on_drop(paths, window, cx)
            }))
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .size_full()
            .relative()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(v_flex().flex_1().min_h_0().w_full().child(content))
            .children(task_hub)
            .children(closing_hub);
        #[cfg(all(windows, target_arch = "x86_64"))]
        let workspace = workspace.child(crate::files::native::finish_frame(
            self.task_hub.is_some() || self.closing_hub.is_some(),
        ));
        workspace
    }
}
