//! One open evidence item: its load, its navigation sidebar and the pages
//! that work on it. Pages are created the first time they are shown and keep
//! their state while the tab lives.
mod dashboard;
mod ios;
mod navigation;
mod platform;
mod unlock;
mod volumes;

use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        progress::Progress,
        spinner::Spinner,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window,
};
use lovelycarve_lib::{
    commands::{
        analysis::bcp as bcp_commands,
        app_error::AppError,
        evidence::{encrypted_volume as encrypted_commands, image as image_commands},
    },
    core::{analysis::bcp::BcpCatalog, evidence::image::ImageInfo},
    host::State,
};

use crate::{
    analysis::{AnalysisDirectory, AnalysisPanel},
    backend::Backend,
    bcp::BcpPanel,
    carving::CarvePanel,
    files::FileBrowser,
    format,
    search::SearchPanel,
    tasks::TaskStore,
    timeline::TimelinePanel,
    ui,
};
pub use dashboard::filesystem_label;
use dashboard::Dashboard;
pub use platform::PlatformStore;
pub use volumes::{supports_block_operations, wire_name};

type ExtraIcon = gpui_kit::assets::IconName;

/// The pages an evidence tab offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Files,
    Search,
    Timeline,
    Carve,
    Analysis,
    /// BCP records of one category, or of every category.
    Bcp(Option<&'static str>),
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Dashboard => "镜像信息",
            Page::Files => "文件列表",
            Page::Search => "全局搜索",
            Page::Timeline => "时间线",
            Page::Carve => "文件恢复",
            Page::Analysis => "分析结果",
            Page::Bcp(category) => crate::bcp::category_label(category),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Dashboard => Icon::new(IconName::Info),
            Page::Files => Icon::new(ExtraIcon::Files),
            Page::Search => Icon::new(IconName::Search),
            Page::Timeline => Icon::new(ExtraIcon::ChartColumn),
            Page::Carve => Icon::new(ExtraIcon::FileSearch),
            Page::Analysis => Icon::new(ExtraIcon::ScanSearch),
            Page::Bcp(category) => crate::bcp::category_icon(category),
        }
    }
}

enum LoadState {
    Loading { task_id: String },
    Ready(Rc<ImageInfo>),
    Failed(SharedString),
}

/// What an evidence tab tells the workspace.
#[derive(Clone, Debug)]
pub enum EvidenceTabEvent {
    /// The evidence opened (and joined the active case).
    Loaded,
    Failed(SharedString),
    /// Open a derived image (a shadow copy, a plaintext volume or an
    /// assembled array) in its own tab.
    OpenImage(String),
    /// Show the quick analysis tab of the workspace task hub.
    QuickAnalysis,
}

#[derive(Default)]
struct Pages {
    dashboard: Option<Entity<Dashboard>>,
    files: Option<Entity<FileBrowser>>,
    search: Option<Entity<SearchPanel>>,
    timeline: Option<Entity<TimelinePanel>>,
    carve: Option<Entity<CarvePanel>>,
    analysis: Option<Entity<AnalysisPanel>>,
    analysis_directory: Option<Entity<AnalysisDirectory>>,
    bcp: Option<Entity<BcpPanel>>,
}

pub struct EvidenceTab {
    path: String,
    name: SharedString,
    case_dir: Option<String>,
    state: LoadState,
    page: Page,
    last_pages: [Option<Page>; navigation::Group::COUNT],
    pages: Pages,
    /// The BCP index of folder evidence that holds a BCP export.
    bcp: Option<Rc<BcpCatalog>>,
    open_bcp_on_load: bool,
    tasks: Entity<TaskStore>,
    _tasks: Subscription,
    _load: Option<Task<()>>,
}

impl EventEmitter<EvidenceTabEvent> for EvidenceTab {}

impl EvidenceTab {
    pub fn new(
        path: String,
        case_dir: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tasks = TaskStore::global(cx);
        let subscription = cx.observe(&tasks, |this, _, cx| {
            if matches!(this.state, LoadState::Loading { .. }) {
                cx.notify();
            }
        });
        let mut tab = Self {
            name: format::file_name(&path).to_owned().into(),
            path,
            case_dir,
            state: LoadState::Failed(SharedString::default()),
            page: Page::Dashboard,
            last_pages: Default::default(),
            pages: Pages::default(),
            bcp: None,
            open_bcp_on_load: false,
            tasks,
            _tasks: subscription,
            _load: None,
        };
        tab.load(window, cx);
        tab
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn name(&self) -> SharedString {
        self.name.clone()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, LoadState::Loading { .. })
    }

    pub fn image_id(&self) -> Option<&str> {
        match &self.state {
            LoadState::Ready(info) => Some(info.id.as_str()),
            _ => None,
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task_id = uuid::Uuid::new_v4().to_string();
        self.state = LoadState::Loading {
            task_id: task_id.clone(),
        };
        let path = self.path.clone();
        let case_dir = self.case_dir.clone();
        let loading = Backend::global(cx).run(move |host, state| async move {
            image_commands::image_load(
                host,
                State::new(&state),
                path,
                None,
                None,
                Some(task_id),
                case_dir,
                None,
            )
            .await
        });
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = loading.await;
            this.update_in(cx, |this, window, cx| {
                match loaded {
                    Ok(info) => {
                        this.page = if info.index_only {
                            Page::Files
                        } else {
                            Page::Dashboard
                        };
                        let folder = !info.index_only && wire_name(&info.source_format) == "folder";
                        this.state = LoadState::Ready(Rc::new(info));
                        this.ensure_page(window, cx);
                        if folder {
                            this.load_bcp(window, cx);
                        }
                        cx.emit(EvidenceTabEvent::Loaded);
                    }
                    Err(error) => {
                        let message = if error.cancelled {
                            SharedString::from("加载已取消")
                        } else {
                            error.message.clone()
                        };
                        this.state = LoadState::Failed(message.clone());
                        if !error.cancelled {
                            cx.emit(EvidenceTabEvent::Failed(message));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Read the BCP index of folder evidence; folders without one offer no
    /// BCP pages.
    pub fn open_bcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bcp.is_some() {
            self.show_page(Page::Bcp(None), window, cx);
        } else {
            self.open_bcp_on_load = true;
            if self.image_id().is_some() {
                self.load_bcp(window, cx);
            }
        }
    }

    fn load_bcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(image_id) = self.image_id().map(str::to_owned) else {
            return;
        };
        let reading = Backend::global(cx).run(move |_, state| async move {
            bcp_commands::bcp_catalog(State::new(&state), image_id).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = reading.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(catalog) => {
                    this.bcp = Some(Rc::new(catalog));
                    if std::mem::take(&mut this.open_bcp_on_load) {
                        this.show_page(Page::Bcp(None), window, cx);
                    }
                    cx.notify();
                }
                Err(error) => {
                    if std::mem::take(&mut this.open_bcp_on_load) {
                        ui::notify_error(window, cx, "读取 BCP 索引失败", error.message);
                    } else {
                        tracing::debug!("bcp_catalog: {error}");
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn cancel_load(&mut self, cx: &mut Context<Self>) {
        if let LoadState::Loading { task_id } = &self.state {
            let task_id = task_id.clone();
            self.tasks.update(cx, |tasks, cx| tasks.cancel(task_id, cx));
        }
    }

    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.last_pages[page.group() as usize] = Some(page);
        self.ensure_page(window, cx);
        cx.notify();
    }

    /// Create the current page's view the first time it is shown.
    fn ensure_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let LoadState::Ready(info) = &self.state else {
            return;
        };
        let info = Rc::clone(info);
        let tab = cx.entity().downgrade();
        match self.page {
            Page::Dashboard if self.pages.dashboard.is_none() => {
                self.pages.dashboard = Some(cx.new(|cx| Dashboard::new(info, tab, window, cx)));
            }
            Page::Files if self.pages.files.is_none() => {
                self.pages.files = Some(cx.new(|cx| FileBrowser::new(info, tab, window, cx)));
            }
            Page::Search if self.pages.search.is_none() => {
                self.pages.search = Some(cx.new(|cx| SearchPanel::new(info, tab, window, cx)));
            }
            Page::Timeline if self.pages.timeline.is_none() => {
                self.pages.timeline = Some(cx.new(|cx| TimelinePanel::new(info, tab, window, cx)));
            }
            Page::Carve if self.pages.carve.is_none() => {
                self.pages.carve = Some(cx.new(|cx| CarvePanel::new(info, window, cx)));
            }
            Page::Analysis if self.pages.analysis.is_none() => {
                let panel = cx.new(|cx| AnalysisPanel::new(info, tab, window, cx));
                self.pages.analysis_directory =
                    Some(cx.new(|cx| AnalysisDirectory::new(panel.clone(), cx)));
                self.pages.analysis = Some(panel);
            }
            Page::Bcp(category) => match (&self.pages.bcp, &self.bcp) {
                (Some(panel), _) => {
                    panel.update(cx, |panel, cx| panel.set_category(category, window, cx));
                }
                (None, Some(catalog)) => {
                    let catalog = Rc::clone(catalog);
                    self.pages.bcp =
                        Some(cx.new(|cx| {
                            BcpPanel::new(info.id.clone(), catalog, category, window, cx)
                        }));
                }
                (None, None) => {}
            },
            _ => {}
        }
    }

    /// Show a file of this evidence in the file list.
    pub fn locate_node(&mut self, node_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(Page::Files, window, cx);
        if let Some(files) = self.pages.files.clone() {
            files.update(cx, |files, cx| files.locate(node_id, window, cx));
        }
    }

    /// Ask the workspace to open a derived image in its own tab.
    pub fn open_image(&mut self, path: String, cx: &mut Context<Self>) {
        cx.emit(EvidenceTabEvent::OpenImage(path));
    }

    /// Volumes became readable (a BitLocker unlock): rebuild the file list.
    pub fn reload_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.files = None;
        if self.page == Page::Files {
            self.ensure_page(window, cx);
        }
        cx.notify();
    }

    /// The session changed under this tab (an unlocked container): show
    /// its new information and rebuild the pages from it.
    fn replace_info(&mut self, info: ImageInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.state = LoadState::Ready(Rc::new(info));
        self.pages = Pages::default();
        self.ensure_page(window, cx);
        cx.notify();
    }

    /// Unlock a LUKS / VeraCrypt / FileVault container in the background with
    /// `options` (the `UnlockOptions` JSON). The source session gains the
    /// plaintext view, or a separate plaintext image opens in its own tab.
    pub fn start_container_unlock(
        &mut self,
        options: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let LoadState::Ready(info) = &self.state else {
            return;
        };
        let image_id = info.id.clone();
        let unlocking = Backend::global(cx).run(move |host, state| async move {
            let options = serde_json::from_value(options)
                .map_err(|error| AppError::invalid_input(error.to_string()))?;
            let result = encrypted_commands::encrypted_volume_unlock(
                host,
                State::new(&state),
                image_id,
                options,
                Some(uuid::Uuid::new_v4().to_string()),
            )
            .await?;
            serde_json::to_value(result).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let unlocked = unlocking.await;
            this.update_in(cx, |this, window, cx| match unlocked {
                Ok(result) => {
                    let cipher = result["cipher"].as_str().unwrap_or_default().to_owned();
                    match serde_json::from_value::<ImageInfo>(result["image"].clone()) {
                        Ok(image) => {
                            if this.image_id() == Some(image.id.as_str()) {
                                this.replace_info(image, window, cx);
                            } else {
                                cx.emit(EvidenceTabEvent::OpenImage(image.path));
                            }
                            ui::notify_success(window, cx, "加密卷解锁成功", cipher);
                        }
                        Err(error) => {
                            ui::notify_error(window, cx, "解锁加密容器失败", error.to_string())
                        }
                    }
                }
                Err(error) if error.cancelled => {}
                Err(error) => ui::notify_error(window, cx, "解锁加密容器失败", error.message),
            })
            .ok();
        })
        .detach();
    }

    fn render_page(&self) -> AnyElement {
        let page = match self.page {
            Page::Dashboard => self
                .pages
                .dashboard
                .clone()
                .map(|view| view.into_any_element()),
            Page::Files => self.pages.files.clone().map(|view| view.into_any_element()),
            Page::Search => self
                .pages
                .search
                .clone()
                .map(|view| view.into_any_element()),
            Page::Timeline => self
                .pages
                .timeline
                .clone()
                .map(|view| view.into_any_element()),
            Page::Carve => self.pages.carve.clone().map(|view| view.into_any_element()),
            Page::Analysis => self
                .pages
                .analysis
                .clone()
                .map(|view| view.into_any_element()),
            Page::Bcp(_) => self.pages.bcp.clone().map(|view| view.into_any_element()),
        };
        page.unwrap_or_else(|| div().into_any_element())
    }

    fn render_loading(&self, task_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let row = self.tasks.read(cx).row(task_id).cloned();
        let percent = row.as_ref().and_then(|row| row.percent());
        let message = row
            .as_ref()
            .map(|row| row.message())
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| "正在打开检材…".to_owned());
        let cancelling = row.as_ref().is_some_and(|row| row.cancelling);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(Spinner::new().large())
            .child(div().text_lg().child(self.name.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message),
            )
            .child(
                div().w(gpui_kit::rems(24.)).child(
                    Progress::new("evidence-load")
                        .when_some(percent, |progress, percent| progress.value(percent))
                        .when(percent.is_none(), |progress| progress.loading(true)),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("解压与分区检测完成后自动打开，可切换标签页继续操作。"),
            )
            .child(
                Button::new("cancel-load")
                    .small()
                    .label(if cancelling {
                        "正在取消…"
                    } else {
                        "取消"
                    })
                    .disabled(cancelling)
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_load(cx))),
            )
    }

    fn render_failed(&self, message: &SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        ui::empty_state(
            Icon::new(IconName::TriangleAlert),
            format!("无法打开 {}", self.name),
            Some(message.clone()),
        )
        .size_full()
        .child(
            Button::new("retry-load")
                .primary()
                .label("重试")
                .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
        )
    }
}

impl Render for EvidenceTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state {
            LoadState::Loading { task_id } => {
                let task_id = task_id.clone();
                v_flex()
                    .size_full()
                    .px_2()
                    .pb_2()
                    .child(ui::workspace_sheet(cx).child(self.render_loading(&task_id, cx)))
                    .into_any_element()
            }
            LoadState::Failed(message) => {
                let message = message.clone();
                v_flex()
                    .size_full()
                    .px_2()
                    .pb_2()
                    .child(ui::workspace_sheet(cx).child(self.render_failed(&message, cx)))
                    .into_any_element()
            }
            LoadState::Ready(info) => {
                let info = Rc::clone(info);
                h_flex()
                    .size_full()
                    .items_stretch()
                    .child(self.render_navigation(&info, window, cx))
                    .child(
                        ui::workspace_sheet(cx)
                            .mr_2()
                            .mb_2()
                            .child(self.render_page()),
                    )
                    .into_any_element()
            }
        }
    }
}
