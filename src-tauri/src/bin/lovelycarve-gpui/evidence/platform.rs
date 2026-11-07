//! The evidence header's actions: the platform conclusion (platform, maker
//! and kind of evidence), the examiner's manual choice, re-detection and the
//! full RAW export. The backend detects the platform as a first task when
//! evidence joins a case and broadcasts the report; the store keeps each
//! image's report for the session.
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{DropdownMenu as _, PopupMenuItem},
        spinner::Spinner,
        tag::Tag,
        ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window,
};
use lovelycarve_lib::{
    commands::{
        analysis::autodetect as autodetect_commands, casework::case as case_commands,
        evidence::image as image_commands,
    },
    core::{
        analysis::identify::autodetect::{AutoDetectReport, EvidencePlatform},
        evidence::image::ImageInfo,
        filesystem::evidence_session::ImageExportMode,
    },
    host::State,
};

use crate::{
    backend::{Backend, BackendEvent},
    cases::CaseStore,
    events::EventHub,
    format,
    tasks::{TaskRow, TaskStore},
    ui,
};

type ExtraIcon = gpui_kit::assets::IconName;

/// A platform as the interface names it.
pub fn platform_label(platform: EvidencePlatform) -> &'static str {
    match platform {
        EvidencePlatform::Android => "Android",
        EvidencePlatform::Ios => "iOS",
        EvidencePlatform::Windows => "Windows",
        EvidencePlatform::MacOs => "macOS",
        EvidencePlatform::Linux => "Linux",
        EvidencePlatform::Harmony => "鸿蒙 HarmonyOS",
        EvidencePlatform::HuaweiHmos => "鸿蒙 HMOS",
        EvidencePlatform::OtherMobile => "其他移动设备",
        EvidencePlatform::Drone => "无人机",
    }
}

/// `evidence.type` as the interface names it; backup words are identifiers
/// already.
fn evidence_type_label(evidence_type: &str) -> &str {
    match evidence_type {
        "disk" => "磁盘镜像",
        "compressfile" => "归档 / 逻辑检材",
        "backup" => "备份文件",
        other => other,
    }
}

/// A maker keyword with its first letter capitalized.
fn manufactor_label(manufactor: &str) -> String {
    if manufactor.eq_ignore_ascii_case("dji") {
        return "DJI".to_owned();
    }
    let mut chars = manufactor.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What the chip says after the platform: 手动指定, or the maker and the
/// kind of evidence.
fn conclusion_type(report: Option<&AutoDetectReport>, manual: bool) -> String {
    if manual {
        return "手动指定".to_owned();
    }
    let Some(report) = report else {
        return String::new();
    };
    let kind = if report.is_backup {
        "备份"
    } else {
        evidence_type_label(&report.evidence_type)
    };
    report
        .manufactor
        .as_deref()
        .map(manufactor_label)
        .into_iter()
        .chain([kind.to_owned()])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The suggested RAW file name: the evidence name with characters Windows
/// refuses and whitespace replaced.
fn export_file_name(path: &str) -> String {
    let name: String = format::file_name(path)
        .chars()
        .map(|character| match character {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_whitespace() => '_',
            character => character,
        })
        .collect();
    format!("{name}.dd")
}

/// The task the backend registers for an image's detection (one per image,
/// shared by the first task and later runs).
fn detect_task_id(image_id: &str) -> String {
    format!("autodetect:{image_id}")
}

/// Each loaded image's latest report, runs in progress and failures.
pub struct PlatformStore {
    reports: HashMap<String, AutoDetectReport>,
    running: HashSet<String>,
    errors: HashMap<String, SharedString>,
    generation: u64,
    _events: Subscription,
}

struct GlobalPlatformStore(Entity<PlatformStore>);

impl Global for GlobalPlatformStore {}

impl PlatformStore {
    /// Installed with the task store at start, so a first task that finishes
    /// before the image-info page opens still leaves its report.
    pub fn install(cx: &mut App) {
        let store = cx.new(|cx| {
            let hub = EventHub::global(cx);
            let events = cx.subscribe(&hub, |this: &mut Self, _, event: &BackendEvent, cx| {
                this.on_event(event, cx)
            });
            Self {
                reports: HashMap::new(),
                running: HashSet::new(),
                errors: HashMap::new(),
                generation: 0,
                _events: events,
            }
        });
        cx.set_global(GlobalPlatformStore(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPlatformStore>().0.clone()
    }

    fn on_event(&mut self, event: &BackendEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            "autodetect-finished" => {
                match serde_json::from_value::<AutoDetectReport>(event.payload.clone()) {
                    Ok(report) => self.publish(report, cx),
                    Err(error) => tracing::warn!("autodetect-finished payload: {error}"),
                }
            }
            "autodetect-failed" => {
                let payload = &event.payload;
                let image_id = payload.get("image_id").and_then(serde_json::Value::as_str);
                let error = payload.get("error");
                let code = error
                    .and_then(|error| error.get("code"))
                    .and_then(serde_json::Value::as_str);
                let message = error
                    .and_then(|error| error.get("message"))
                    .and_then(serde_json::Value::as_str);
                // Cancelling from the task center or unloading is not a failure.
                if let (Some(image_id), Some(message)) = (image_id, message) {
                    if code != Some("cancelled") {
                        self.errors
                            .insert(image_id.to_owned(), message.to_owned().into());
                        cx.notify();
                    }
                }
            }
            _ => {}
        }
    }

    /// Keep a report. The backend wrote its platform and type onto the
    /// case's evidence row, so a changed report re-reads the active case.
    fn publish(&mut self, report: AutoDetectReport, cx: &mut Context<Self>) {
        self.errors.remove(&report.image_id);
        let changed = self.reports.get(&report.image_id) != Some(&report);
        let cases = CaseStore::global(cx);
        let in_case = cases.read(cx).active().is_some_and(|case| {
            case.evidence
                .iter()
                .any(|evidence| evidence.image_id == report.image_id)
        });
        self.reports.insert(report.image_id.clone(), report);
        if changed && in_case {
            cases.update(cx, |cases, cx| cases.refresh_active(cx));
        }
        cx.notify();
    }

    pub fn report(&self, image_id: &str) -> Option<&AutoDetectReport> {
        self.reports.get(image_id)
    }

    pub fn error(&self, image_id: &str) -> Option<&SharedString> {
        self.errors.get(image_id)
    }

    /// Whether a run this interface started is in progress.
    pub fn is_running(&self, image_id: &str) -> bool {
        self.running.contains(image_id)
    }

    /// Detect the platform again and write it back to the case; resolves to
    /// the failure message, `None` on success or cancellation.
    pub fn run(&mut self, image_id: String, cx: &mut Context<Self>) -> Task<Option<SharedString>> {
        if !self.running.insert(image_id.clone()) {
            return Task::ready(None);
        }
        let generation = self.generation;
        let source = image_id.clone();
        let detecting = Backend::global(cx).run(move |host, state| async move {
            autodetect_commands::autodetect_evidence(host, State::new(&state), source).await
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let detected = detecting.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return None;
                }
                this.running.remove(&image_id);
                cx.notify();
                match detected {
                    Ok(report) => {
                        this.publish(report, cx);
                        None
                    }
                    Err(error) if error.cancelled => None,
                    Err(error) => {
                        this.errors.insert(image_id, error.message.clone());
                        Some(error.message)
                    }
                }
            })
            .ok()
            .flatten()
        })
    }

    /// A different case is open: its evidence was unloaded.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.reports.clear();
        self.running.clear();
        self.errors.clear();
        cx.notify();
    }
}

/// The platform chip, the manual choice, re-detection and the RAW export at
/// the right of the evidence header.
pub struct HeaderActions {
    info: Rc<ImageInfo>,
    store: Entity<PlatformStore>,
    cases: Entity<CaseStore>,
    tasks: Entity<TaskStore>,
    /// The detection the backend started for this image is still running.
    detect_task_active: bool,
    /// The choice being saved, shown until the case has it.
    saving: Option<Option<EvidencePlatform>>,
    /// The RAW export's task while it runs.
    export_task: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl HeaderActions {
    pub fn new(info: Rc<ImageInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = PlatformStore::global(cx);
        let cases = CaseStore::global(cx);
        let tasks = TaskStore::global(cx);
        let detect_task = detect_task_id(&info.id);
        let detect_task_active = tasks
            .read(cx)
            .row(&detect_task)
            .is_some_and(TaskRow::is_active);
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&cases, |_, _, cx| cx.notify()),
            cx.observe(&tasks, move |this: &mut Self, tasks, cx| {
                let active = tasks
                    .read(cx)
                    .row(&detect_task)
                    .is_some_and(TaskRow::is_active);
                if active != this.detect_task_active || this.export_task.is_some() {
                    this.detect_task_active = active;
                    cx.notify();
                }
            }),
        ];
        let mut actions = Self {
            info,
            store,
            cases,
            tasks,
            detect_task_active,
            saving: None,
            export_task: None,
            _subscriptions: subscriptions,
        };
        // The first time the page opens without a report or a run in
        // progress, detect once: detection only reads, and its write-back is
        // idempotent. Offline indexes have no source bytes to detect from.
        let reported = actions.store.read(cx).report(&actions.info.id).is_some();
        if !reported && !actions.detecting(cx) && !actions.info.index_only {
            actions.detect(window, cx);
        }
        actions
    }

    fn detecting(&self, cx: &App) -> bool {
        self.detect_task_active || self.store.read(cx).is_running(&self.info.id)
    }

    fn detect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let running = self.store.update(cx, |store, cx| store.run(image_id, cx));
        cx.spawn_in(window, async move |_, cx| {
            if let Some(message) = running.await {
                cx.update(|window, cx| ui::notify_error(window, cx, "自动识别失败", message))
                    .ok();
            }
        })
        .detach();
    }

    /// The examiner's platform outranks detection and survives re-detection;
    /// `None` returns to the detected one.
    fn choose(
        &mut self,
        choice: Option<EvidencePlatform>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving.is_some() {
            return;
        }
        self.saving = Some(choice);
        let image_id = self.info.id.clone();
        let word = choice.map_or("auto", EvidencePlatform::as_str).to_owned();
        let setting = Backend::global(cx).run(move |_, state| async move {
            case_commands::case_evidence_platform_set(State::new(&state), image_id, word).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let set = setting.await;
            this.update_in(cx, |this, window, cx| {
                match set {
                    Ok(evidence) => this
                        .cases
                        .update(cx, |cases, cx| cases.update_evidence(evidence, cx)),
                    Err(error) => ui::notify_error(window, cx, "手动指定平台失败", error.message),
                }
                this.saving = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_task.is_some() {
            return;
        }
        let prompt = ui::prompt_save(&export_file_name(&self.info.path), cx);
        let image_id = self.info.id.clone();
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(destination) = prompt.await else {
                return;
            };
            let task_id = uuid::Uuid::new_v4().to_string();
            let started = this
                .update(cx, |this, cx| {
                    if this.export_task.is_some() {
                        return false;
                    }
                    this.export_task = Some(task_id.clone());
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !started {
                return;
            }
            let shown = destination.display().to_string();
            // Unlocked partitions are written as plaintext; everything else
            // keeps the source bytes.
            let exported = backend
                .run(move |host, state| async move {
                    image_commands::image_export_raw(
                        host,
                        State::new(&state),
                        image_id,
                        destination.display().to_string(),
                        task_id,
                        None,
                        Some(ImageExportMode::UnlockedPartitions),
                    )
                    .await
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.export_task = None;
                match exported {
                    Ok(result) if result.read_faults.read_faults_detected => ui::notify_warning(
                        window,
                        cx,
                        "导出完成，但包含补零字节",
                        format!(
                            "{shown} · {} · 含 {} 补零字节（{} 个坏区）",
                            format::bytes(result.bytes_written),
                            format::bytes(result.read_faults.synthetic_bytes),
                            result.read_faults.bad_regions.len()
                        ),
                    ),
                    Ok(result) => ui::notify_success(
                        window,
                        cx,
                        "源镜像导出完成",
                        format!("{shown} · {}", format::bytes(result.bytes_written)),
                    ),
                    Err(error) if error.cancelled => {
                        ui::notify_info(window, cx, "导出已取消", format!("{shown} · 保留部分文件"))
                    }
                    Err(error) => ui::notify_error(window, cx, "导出失败", error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if let Some(task_id) = self.export_task.clone() {
            self.tasks.update(cx, |tasks, cx| tasks.cancel(task_id, cx));
        }
    }

    fn render_conclusion(
        &self,
        report: Option<&AutoDetectReport>,
        manual: Option<EvidencePlatform>,
        cx: &App,
    ) -> gpui_kit::AnyElement {
        let muted = cx.theme().muted_foreground;
        let platform = manual.or(report.and_then(|report| report.platform));
        if report.is_none() && manual.is_none() {
            let store = self.store.read(cx);
            return if self.detecting(cx) {
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(Spinner::new().xsmall())
                    .child("正在识别平台…")
                    .into_any_element()
            } else if let Some(error) = store.error(&self.info.id) {
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(Icon::new(ExtraIcon::TriangleAlert).xsmall())
                    .child(format!("平台识别失败：{error}"))
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("尚未识别平台")
                    .into_any_element()
            };
        }
        let backup = manual.is_none() && report.is_some_and(|report| report.is_backup);
        let icon = if backup {
            ExtraIcon::Archive
        } else {
            match platform {
                Some(EvidencePlatform::Drone) => ExtraIcon::Drone,
                Some(platform) if platform.is_phone() => ExtraIcon::Smartphone,
                _ => ExtraIcon::Monitor,
            }
        };
        let name = platform.map_or("平台未识别", platform_label);
        let kind = conclusion_type(report, manual.is_some());
        let chip = if platform.is_some() {
            Tag::info()
        } else {
            Tag::secondary()
        };
        // With a manual choice the detected conclusion stays in the second
        // line only.
        let detected = report
            .filter(|_| manual.is_some())
            .map(|report| match report.platform {
                _ if !report.display_platform_name.is_empty() => {
                    report.display_platform_name.clone()
                }
                Some(platform) => platform_label(platform).to_owned(),
                None => "未识别".to_owned(),
            });
        h_flex()
            .gap_1()
            .child(
                chip.small().rounded_full().child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(icon).xsmall())
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(name),
                        )
                        .when(!kind.is_empty(), |this| this.child(kind)),
                ),
            )
            .when_some(detected, |this, detected| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("自动识别：{detected}")),
                )
            })
            .into_any_element()
    }

    fn render_choice(
        &self,
        manual: Option<EvidencePlatform>,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.saving.unwrap_or(manual);
        let label = current.map_or("自动识别", platform_label);
        let entity = cx.entity().downgrade();
        Button::new("platform-choice")
            .small()
            .outline()
            .dropdown_caret(true)
            .label(label)
            .loading(self.saving.is_some())
            .disabled(self.saving.is_some() || locked)
            .tooltip(if locked {
                "案件已锁定，解锁后才能修改平台"
            } else {
                "自动识别有误时手动指定平台；重新识别不会覆盖"
            })
            .dropdown_menu(move |menu, _, _| {
                let choices =
                    std::iter::once(None).chain(EvidencePlatform::ALL.into_iter().map(Some));
                choices.fold(menu, |menu, choice| {
                    let entity = entity.clone();
                    let label = choice.map_or("自动识别", platform_label);
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(choice == current)
                            .on_click(move |_, window, cx| {
                                entity
                                    .update(cx, |this, cx| {
                                        if choice != current {
                                            this.choose(choice, window, cx);
                                        }
                                    })
                                    .ok();
                            }),
                    )
                })
            })
    }
}

impl Render for HeaderActions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let image_id = self.info.id.clone();
        let case = self.cases.read(cx).active();
        let evidence = case.and_then(|case| {
            case.evidence
                .iter()
                .find(|evidence| evidence.image_id == image_id)
                .cloned()
        });
        let locked = case.is_some_and(|case| case.locked);
        let manual = evidence
            .as_ref()
            .and_then(|evidence| evidence.manual_platform.as_deref())
            .and_then(EvidencePlatform::from_word);
        let report = self.store.read(cx).report(&image_id).cloned();
        // Other platforms the partitions matched; a manual choice keeps only
        // its own.
        let extra: Vec<EvidencePlatform> = match (&report, manual) {
            (Some(report), None) => report
                .platforms
                .iter()
                .copied()
                .filter(|platform| Some(*platform) != report.platform)
                .collect(),
            _ => Vec::new(),
        };
        let detecting = self.detecting(cx);
        let export_progress = self
            .export_task
            .as_ref()
            .map(|task_id| self.tasks.read(cx).row(task_id).and_then(TaskRow::percent));
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .child(self.render_conclusion(report.as_ref(), manual, cx))
            .children(
                extra
                    .into_iter()
                    .map(|platform| Tag::secondary().small().child(platform_label(platform))),
            )
            .when(evidence.is_some(), |this| {
                this.child(self.render_choice(manual, locked, cx))
            })
            .map(|this| match export_progress {
                Some(percent) => this
                    .child(Button::new("export-source").small().loading(true).label(
                        match percent {
                            Some(percent) => format!("导出中 {percent:.1}%"),
                            None => "导出中…".to_owned(),
                        },
                    ))
                    .child(
                        Button::new("cancel-export-source")
                            .small()
                            .ghost()
                            .label("取消")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_export(cx))),
                    ),
                None if super::supports_block_operations(&self.info) => this.child(
                    Button::new("export-source")
                        .small()
                        .icon(Icon::new(ExtraIcon::HardDriveDownload))
                        .label("导出源镜像")
                        .tooltip("导出完整 RAW 镜像；已解锁分区使用解密内容")
                        .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                ),
                None => this,
            })
            .child(
                Button::new("redetect-platform")
                    .small()
                    .ghost()
                    .icon(Icon::new(ExtraIcon::RefreshCw))
                    .loading(detecting)
                    .disabled(detecting || self.info.index_only)
                    .tooltip(if detecting {
                        "正在识别平台"
                    } else if report.is_some() {
                        "重新识别平台并回写检材"
                    } else {
                        "开始识别平台"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.detect(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use lovelycarve_lib::core::analysis::identify::autodetect::AutoDetectReport;

    use super::{conclusion_type, export_file_name, manufactor_label};

    fn report(evidence_type: &str, manufactor: Option<&str>, is_backup: bool) -> AutoDetectReport {
        serde_json::from_value(serde_json::json!({
            "image_id": "img",
            "route": "disk_image",
            "platform": "android",
            "platforms": ["android"],
            "evidence_type": evidence_type,
            "is_backup": is_backup,
            "manufactor": manufactor,
            "display_platform_name": "",
            "friend_platform_name": "",
            "partitions": [],
            "backup": null,
            "warnings": [],
        }))
        .expect("report")
    }

    #[test]
    fn makers_are_capitalized() {
        assert_eq!(manufactor_label("dji"), "DJI");
        assert_eq!(manufactor_label("huawei"), "Huawei");
        assert_eq!(manufactor_label(""), "");
    }

    #[test]
    fn the_chip_names_the_maker_and_the_kind_of_evidence() {
        assert_eq!(conclusion_type(None, true), "手动指定");
        assert_eq!(conclusion_type(None, false), "");
        assert_eq!(
            conclusion_type(Some(&report("disk", None, false)), false),
            "磁盘镜像"
        );
        assert_eq!(
            conclusion_type(Some(&report("huawei_backup", Some("huawei"), true)), false),
            "Huawei 备份"
        );
        assert_eq!(
            conclusion_type(Some(&report("compressfile", None, false)), false),
            "归档 / 逻辑检材"
        );
    }

    #[test]
    fn export_names_drop_characters_windows_refuses() {
        assert_eq!(
            export_file_name(r"D:\cases\disk 1:a?.E01"),
            "disk_1_a_.E01.dd"
        );
    }
}
