//! 文件恢复: signature carving and filesystem record recovery scans of this
//! evidence, their results, and recovery to a host directory.
mod create;
mod filesystem;
mod results;

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        alert::Alert,
        button::{Button, ButtonVariants as _},
        h_flex, h_resizable,
        input::{Input, InputEvent, InputState},
        progress::Progress,
        resizable_panel,
        scroll::ScrollableElement as _,
        tab::{Tab, TabBar},
        table::{DataTable, TableEvent, TableState},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
};
use lovelycarve_lib::{
    commands::carving::{
        carve::{self as carve_commands},
        carve_filesystem as filesystem_commands, carve_history as history_commands,
        carve_recovery::{self as recovery_commands, CarveRecoverOptions},
    },
    core::{
        carving::{
            carve_session::{CarveScanSnapshot, CarveSessionStatus},
            carver::CarveArtifact,
        },
        evidence::image::ImageInfo,
        filesystem::recovery::RecoveryCandidate,
    },
    host::State,
};

use crate::{
    backend::{Backend, BackendEvent},
    events::EventHub,
    format, ui,
};
use create::{ScanForm, StartScan};
use filesystem::{readable_ids, FilesystemTable};
use results::ResultTable;

/// Which results the page lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResultView {
    Signatures,
    Filesystem,
}

/// The filesystem phase finished and its result can be read.
fn filesystem_ready(scan: &CarveScanSnapshot) -> bool {
    scan.filesystem
        .as_ref()
        .is_some_and(|filesystem| filesystem.finished && filesystem.error.is_none())
}

type ExtraIcon = gpui_kit::assets::IconName;

fn status_tag(snapshot: &CarveScanSnapshot) -> Tag {
    match snapshot.status {
        CarveSessionStatus::Queued => Tag::secondary().xsmall().child("排队中"),
        CarveSessionStatus::Running => Tag::info().xsmall().child("扫描中"),
        CarveSessionStatus::Paused => Tag::warning().xsmall().child("已暂停"),
        CarveSessionStatus::Completed => Tag::success().xsmall().child("已完成"),
        CarveSessionStatus::Cancelled => Tag::secondary().xsmall().child("已取消"),
        CarveSessionStatus::Failed => Tag::danger().xsmall().child("失败"),
    }
}

pub struct CarvePanel {
    info: Rc<ImageInfo>,
    scans: Vec<CarveScanSnapshot>,
    selected: Option<String>,
    view: ResultView,
    table: Entity<TableState<ResultTable>>,
    artifact: Option<CarveArtifact>,
    files: Entity<TableState<FilesystemTable>>,
    file: Option<RecoveryCandidate>,
    /// The scan whose filesystem result is loaded or loading.
    files_for: Option<String>,
    filter: Entity<InputState>,
    refresh: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CarvePanel {
    pub fn new(info: Rc<ImageInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| TableState::new(ResultTable::new(), window, cx).col_movable(false));
        let files = cx.new(|cx| {
            TableState::new(FilesystemTable::new(), window, cx)
                .col_movable(false)
                .sortable(false)
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("筛选文件名或路径"));
        let hub = EventHub::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    this.artifact = table.read(cx).delegate().row(*index).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe_in(&files, window, |this, files, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    this.file = files.read(cx).delegate().candidate(*index).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&files, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &filter,
                window,
                |this, filter, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        let text = filter.read(cx).value().to_string();
                        this.file = None;
                        this.files.update(cx, |files, cx| {
                            files.clear_selection(cx);
                            files.delegate_mut().set_filter(&text);
                            files.refresh(cx);
                        });
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
                if event.name == "carve-scan-progress" {
                    this.schedule_refresh(cx);
                }
            }),
        ];
        let mut panel = Self {
            info,
            scans: Vec::new(),
            selected: None,
            view: ResultView::Signatures,
            table,
            artifact: None,
            files,
            file: None,
            files_for: None,
            filter,
            refresh: None,
            _subscriptions: subscriptions,
        };
        panel.load_scans(true, cx);
        panel
    }

    /// Read the selected scan's filesystem result once its phase finished.
    fn ensure_filesystem(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = self.selected_scan() else {
            return;
        };
        if !filesystem_ready(scan) || self.files_for.as_deref() == Some(scan.scan_id.as_str()) {
            return;
        }
        let scan_id = scan.scan_id.clone();
        self.files_for = Some(scan_id.clone());
        self.files.update(cx, |files, cx| {
            files.delegate_mut().set_loading();
            files.refresh(cx);
        });
        let reading = Backend::global(cx).run(move |_, state| async move {
            filesystem_commands::carve_filesystem_results(State::new(&state), scan_id).await
        });
        let loading_for = self.files_for.clone();
        cx.spawn(async move |this, cx| {
            let result = reading.await;
            this.update(cx, |this, cx| {
                if this.files_for != loading_for {
                    return;
                }
                if result.is_err() {
                    // A later refresh may try again.
                    this.files_for = None;
                }
                let result = result
                    .map(|scan| scan.map(Rc::new))
                    .map_err(|error| error.message);
                this.files.update(cx, |files, cx| {
                    files.delegate_mut().set_result(result);
                    files.refresh(cx);
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn selected_scan(&self) -> Option<&CarveScanSnapshot> {
        let selected = self.selected.as_ref()?;
        self.scans.iter().find(|scan| &scan.scan_id == selected)
    }

    fn load_scans(&mut self, select_first: bool, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let listed = Backend::global(cx).run(move |_, state| async move {
            history_commands::carve_scan_list(State::new(&state), image_id).await
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                match listed {
                    Ok(mut scans) => {
                        scans.sort_by(|left, right| {
                            right.created_unix_ms.cmp(&left.created_unix_ms)
                        });
                        this.scans = scans;
                        if select_first && this.selected.is_none() {
                            if let Some(first) = this.scans.first().map(|scan| scan.scan_id.clone())
                            {
                                this.select(first, cx);
                            }
                        }
                        this.ensure_filesystem(cx);
                    }
                    Err(error) => tracing::warn!("carve_scan_list: {error}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, scan_id: String, cx: &mut Context<Self>) {
        self.selected = Some(scan_id.clone());
        self.artifact = None;
        self.file = None;
        self.files_for = None;
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().show(Some(scan_id), None, cx);
        });
        self.files.update(cx, |files, cx| {
            files.clear_selection(cx);
            files.delegate_mut().clear();
            files.refresh(cx);
        });
        // A filesystem-only scan opens on its records.
        self.view = match self.selected_scan() {
            Some(scan) if !scan.signatures && scan.filesystem.is_some() => ResultView::Filesystem,
            _ => ResultView::Signatures,
        };
        self.ensure_filesystem(cx);
        cx.notify();
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_some() {
            return;
        }
        self.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            this.update(cx, |this, cx| {
                this.refresh = None;
                this.load_scans(false, cx);
                this.table
                    .update(cx, |table, cx| table.delegate_mut().follow(cx));
            })
            .ok();
        }));
    }

    fn show_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let info = Rc::clone(&self.info);
        let form = cx.new(|_| ScanForm::new(&info, carve_commands::carve_formats()));
        let subscription =
            cx.subscribe_in(&form, window, |this, _, StartScan(options), window, cx| {
                window.close_dialog(cx);
                this.start_scan(options.clone(), window, cx);
            });
        self._subscriptions.push(subscription);
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("新建恢复扫描").w(px(620.)).child(form.clone())
        });
    }

    fn start_scan(
        &mut self,
        options: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = match serde_json::from_value(options) {
            Ok(options) => options,
            Err(error) => {
                ui::notify_error(window, cx, "扫描参数无效", error.to_string());
                return;
            }
        };
        let image_id = self.info.id.clone();
        let started = Backend::global(cx).run(move |host, state| async move {
            carve_commands::carve_start(host, State::new(&state), image_id, options).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let started = started.await;
            this.update_in(cx, |this, window, cx| match started {
                Ok(started) => {
                    this.select(started.scan_id, cx);
                    this.load_scans(false, cx);
                }
                Err(error) => ui::notify_error(window, cx, "无法开始扫描", error.message),
            })
            .ok();
        })
        .detach();
    }

    fn pause_or_resume(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = self.selected_scan() else {
            return;
        };
        let scan_id = scan.scan_id.clone();
        let paused = scan.status == CarveSessionStatus::Paused;
        let changed = Backend::global(cx).run(move |host, state| async move {
            if paused {
                carve_commands::carve_scan_resume(host, State::new(&state), scan_id).await
            } else {
                carve_commands::carve_scan_pause(host, State::new(&state), scan_id).await
            }
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = changed.await {
                tracing::warn!("carve pause/resume: {error}");
            }
            this.update(cx, |this, cx| this.load_scans(false, cx)).ok();
        })
        .detach();
    }

    fn remove_scan(&mut self, scan_id: String, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let removing = scan_id.clone();
        let removed = Backend::global(cx).run(move |_, state| async move {
            history_commands::carve_scan_remove(State::new(&state), image_id, removing).await
        });
        cx.spawn(async move |this, cx| {
            let removed = removed.await;
            this.update(cx, |this, cx| {
                if removed.is_ok() && this.selected.as_deref() == Some(scan_id.as_str()) {
                    this.selected = None;
                    this.artifact = None;
                    this.file = None;
                    this.files_for = None;
                    this.table
                        .update(cx, |table, cx| table.delegate_mut().show(None, None, cx));
                    this.files.update(cx, |files, cx| {
                        files.delegate_mut().clear();
                        files.refresh(cx);
                    });
                }
                this.load_scans(false, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Recover carved results (all of them for `None`) and filesystem
    /// records into a directory.
    fn recover(
        &mut self,
        artifact_ids: Option<Vec<String>>,
        file_ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scan_id) = self.selected.clone() else {
            return;
        };
        let directory = ui::prompt_directory("选择恢复文件的保存目录", cx);
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |_, cx| {
            let Some(directory) = directory.await else {
                return;
            };
            let options = CarveRecoverOptions {
                artifact_ids,
                file_ids,
                destination: directory.display().to_string(),
                task_id: uuid::Uuid::new_v4().to_string(),
            };
            cx.update(|window, cx| {
                ui::notify_success(
                    window,
                    cx,
                    "开始恢复",
                    format!("{} · 进度见任务中心", directory.display()),
                )
            })
            .ok();
            let recovered = backend
                .run(move |_, state| async move {
                    recovery_commands::carve_recover(State::new(&state), scan_id, options).await
                })
                .await;
            cx.update(|window, cx| match recovered {
                Ok(result) => {
                    // Top-level counts are carved results; `files` the records.
                    let files = result.files.as_ref();
                    let recovered = result.recovered + files.map_or(0, |files| files.recovered);
                    let failed = result.failed + files.map_or(0, |files| files.failed);
                    let bytes = result.bytes_written + files.map_or(0, |files| files.bytes_written);
                    let synthetic = files
                        .is_some_and(|files| files.read_faults.read_faults_detected)
                        || result.synthetic_artifacts > 0;
                    let mut detail = format!(
                        "{} 个文件 · {}",
                        format::count(recovered as u64),
                        format::bytes(bytes)
                    );
                    if failed > 0 {
                        detail.push_str(&format!(" · 失败 {failed}"));
                    }
                    if synthetic {
                        detail.push_str(" · 含补零内容");
                    }
                    for report in [
                        result.report_path.clone(),
                        files.map(|files| files.report_path.clone()),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        detail.push_str(&format!(" · 报告 {report}"));
                    }
                    if failed > 0 || synthetic {
                        ui::notify_warning(window, cx, "恢复完成", detail);
                    } else {
                        ui::notify_success(window, cx, "恢复完成", detail);
                    }
                }
                Err(error) => ui::notify_error(window, cx, "恢复失败", error.message),
            })
            .ok();
        })
        .detach();
    }

    fn render_scans(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .child(div().flex_1().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("恢复记录"))
                    .child(Tag::secondary().small().child(self.scans.len().to_string())),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .px_2()
                    .when(self.scans.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("还没有扫描；点击「新建扫描」开始。扫描结果只保留在本次会话中。"),
                        )
                    })
                    .children(self.scans.iter().enumerate().map(|(index, scan)| {
                        let active = selected.as_deref() == Some(scan.scan_id.as_str());
                        let scan_id = scan.scan_id.clone();
                        let remove_id = scan.scan_id.clone();
                        let finished = scan.status.is_finished();
                        let percent = format::percent(scan.bytes_scanned, scan.bytes_total);
                        // Carved results plus files from filesystem records.
                        let found = scan.artifacts_found
                            + scan.filesystem.as_ref().map_or(0, |filesystem| filesystem.files as u64);
                        v_flex()
                            .id(("carve-scan", index))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .rounded(cx.theme().radius)
                            .cursor_pointer()
                            .when(active, |this| this.bg(cx.theme().list_active))
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(scan_id.clone(), cx)))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().truncate().text_sm().child(scan.scope_label.clone()))
                                    .child(Tag::secondary().xsmall().child(format::count(found)))
                                    .when(finished, |this| {
                                        this.child(
                                            Button::new(("remove-scan", index))
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(ExtraIcon::Trash))
                                                .accessibility_label("移除扫描记录")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.remove_scan(remove_id.clone(), cx)
                                                })),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(status_tag(scan))
                                    .child(format::short_local_time(scan.created_unix_ms))
                                    .when(scan.truncated, |this| this.child("已达上限")),
                            )
                            .when(!finished, |this| this.child(Progress::new(("scan-progress", index)).value(percent)))
                    }))
                    .overflow_y_scrollbar(),
            )
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(artifact) = self.artifact.clone() else {
            return v_flex()
                .size_full()
                .child(ui::empty_state(
                    Icon::new(IconName::Info),
                    "未选择文件",
                    Some("在结果列表中选择一项查看详情。".into()),
                ))
                .into_any_element();
        };
        let id = artifact.id.clone();
        v_flex()
            .size_full()
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(ExtraIcon::FileSearch).text_color(cx.theme().primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(artifact.name.clone()),
                    )
                    .child(
                        Tag::secondary()
                            .xsmall()
                            .child(artifact.extension.to_uppercase()),
                    ),
            )
            .child(
                div().py_2().child(
                    Button::new("recover-one")
                        .small()
                        .icon(Icon::new(ExtraIcon::Download))
                        .label("恢复此文件")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.recover(Some(vec![id.clone()]), Vec::new(), window, cx)
                        })),
                ),
            )
            .child(ui::section_title("基本信息", cx))
            .child(ui::field_row("格式", artifact.format_id.to_uppercase(), cx))
            .child(ui::field_row("类别", artifact.category.clone(), cx))
            .child(ui::field_row(
                "大小",
                format!(
                    "{}（{} 字节）",
                    format::bytes(artifact.length),
                    format::count(artifact.length)
                ),
                cx,
            ))
            .child(ui::field_row(
                "起始偏移",
                ui::mono(format::hex_offset(artifact.offset), cx),
                cx,
            ))
            .child(ui::field_row("所在区域", artifact.region.clone(), cx))
            .child(ui::field_row(
                "置信度",
                format!("{}%", artifact.confidence),
                cx,
            ))
            .when(artifact.read_fault, |this| {
                this.child(ui::error_text(
                    "该区域读取时遇到坏扇区，恢复内容含补零字节。",
                    cx,
                ))
            })
            .child(ui::section_title("复核提示", cx))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("雕刻结果没有原始路径与时间戳，请通过偏移和导出哈希复核。"),
            )
            .into_any_element()
    }

    fn render_file_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(file) = self.file.clone() else {
            return v_flex()
                .size_full()
                .child(ui::empty_state(
                    Icon::new(IconName::Info),
                    "未选择文件",
                    Some("在文件系统记录中选择一项查看详情。".into()),
                ))
                .into_any_element();
        };
        let volume = self
            .files
            .read(cx)
            .delegate()
            .volume_name(file.volume_index);
        let id = file.id.clone();
        let node = &file.node;
        let times = [
            ("修改时间", &node.timestamps.modified),
            ("访问时间", &node.timestamps.accessed),
            ("创建时间", &node.timestamps.created),
            ("元数据变更", &node.timestamps.mft_changed),
            ("出生时间", &node.timestamps.birth),
        ];
        v_flex()
            .size_full()
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(ExtraIcon::FolderSearch).text_color(cx.theme().primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(node.name.clone()),
                    )
                    .child(if node.deleted {
                        Tag::danger().xsmall().child("已删除")
                    } else {
                        Tag::secondary().xsmall().child("现存")
                    }),
            )
            .child(
                div().py_2().child(
                    Button::new("recover-file")
                        .small()
                        .icon(Icon::new(ExtraIcon::Download))
                        .label("恢复此文件")
                        .disabled(!file.readable)
                        .tooltip(if file.readable {
                            "导出该记录映射的数据"
                        } else {
                            "该记录没有可读取的数据映射"
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.recover(Some(Vec::new()), vec![id.clone()], window, cx)
                        })),
                ),
            )
            .child(ui::section_title("基本信息", cx))
            .child(ui::field_row(
                "路径",
                ui::mono(file.path.clone(), cx).text_xs(),
                cx,
            ))
            .child(ui::field_row("卷", volume, cx))
            .child(ui::field_row(
                "大小",
                format!(
                    "{}（{} 字节）",
                    format::bytes(node.size),
                    format::count(node.size)
                ),
                cx,
            ))
            .when_some(file.initialized_size, |this, initialized| {
                this.child(ui::field_row(
                    "已初始化",
                    format!("{} 字节，其后按格式为零", format::count(initialized)),
                    cx,
                ))
            })
            .child(ui::field_row(
                "数据映射",
                if file.readable {
                    "可读取"
                } else {
                    "不可读取"
                },
                cx,
            ))
            .when_some(file.source_path.clone(), |this, source| {
                this.child(ui::field_row(
                    "回收站记录",
                    ui::mono(source, cx).text_xs(),
                    cx,
                ))
            })
            .when(times.iter().any(|(_, time)| time.is_some()), |this| {
                this.child(ui::section_title("时间", cx))
                    .children(times.into_iter().filter_map(|(label, time)| {
                        let time = time.clone()?;
                        Some(ui::field_row(label, ui::mono(time, cx), cx))
                    }))
            })
            .child(ui::section_title("复核提示", cx))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("可读映射只说明数据能导出，不能证明内容未被覆盖；请结合导出哈希复核。"),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }

    /// The filesystem phase's volumes, counts and warnings.
    fn render_filesystem_summary(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let files = self.files.read(cx).delegate();
        let scan = files.scan()?.clone();
        let lost = scan.volumes.iter().filter(|volume| volume.lost).count();
        let readable = scan
            .candidates
            .iter()
            .filter(|candidate| candidate.readable)
            .count();
        let mut summary = format!(
            "{} 个卷{} · {} 个目录 · {} 个文件（可恢复 {}）",
            format::count(scan.volumes.len() as u64),
            if lost > 0 {
                format!("（{lost} 个丢失卷）")
            } else {
                String::new()
            },
            format::count(scan.directories.len() as u64),
            format::count(scan.candidates.len() as u64),
            format::count(readable as u64)
        );
        let volume_errors: Vec<String> = scan
            .volumes
            .iter()
            .zip(filesystem::volume_names(&scan.volumes))
            .filter_map(|(volume, name)| {
                volume
                    .error
                    .as_ref()
                    .map(|error| format!("{name}：{error}"))
            })
            .collect();
        if scan.read_faults.read_faults_detected {
            summary.push_str(" · 读取时遇到坏扇区");
        }
        Some(
            v_flex()
                .gap_1()
                .px_3()
                .pt_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(summary),
                )
                .when(scan.truncated, |this| {
                    this.child(Alert::warning(
                        "filesystem-truncated",
                        "文件系统记录达到安全上限，结果不完整。请缩小扫描范围。",
                    ))
                })
                .children(
                    scan.warnings
                        .iter()
                        .chain(volume_errors.iter())
                        .take(4)
                        .enumerate()
                        .map(|(index, warning)| {
                            Alert::warning(("filesystem-warning", index), warning.clone())
                        }),
                ),
        )
    }
}

impl Render for CarvePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table.read(cx).delegate();
        let artifacts = table.total();
        let files = self.files.read(cx).delegate();
        let readable = files
            .scan()
            .map(|scan| readable_ids(scan))
            .unwrap_or_default();
        let view = self.view;
        let footer_right = match view {
            ResultView::Signatures => format!(
                "已加载 {} / {} 条结果{}",
                format::count(table.loaded() as u64),
                format::count(artifacts as u64),
                if table.is_truncated() {
                    " · 已达结果上限"
                } else {
                    ""
                }
            ),
            ResultView::Filesystem => match files.scan() {
                Some(scan) => format!(
                    "显示 {} / {} 个文件",
                    format::count(files.shown_count() as u64),
                    format::count(scan.candidates.len() as u64)
                ),
                None if files.is_loading() => "正在读取文件系统记录…".to_owned(),
                None => String::new(),
            },
        };
        let scan = self.selected_scan().cloned();
        // Both kinds of results: switch between them.
        let both = scan
            .as_ref()
            .is_some_and(|scan| scan.signatures && scan.filesystem.is_some());
        let filesystem_pending = scan.as_ref().is_some_and(|scan| {
            scan.filesystem.is_some() && !filesystem_ready(scan) && files.error().is_none()
        });
        let footer_left: SharedString = match &scan {
            Some(scan) => format!(
                "{} · 已扫描 {} / {}",
                scan.scope_label,
                format::bytes(scan.bytes_scanned),
                format::bytes(scan.bytes_total)
            )
            .into(),
            None => "未选择扫描".into(),
        };
        let can_pause = scan.as_ref().is_some_and(|scan| {
            matches!(
                scan.status,
                CarveSessionStatus::Running | CarveSessionStatus::Paused
            )
        });
        let paused = scan
            .as_ref()
            .is_some_and(|scan| scan.status == CarveSessionStatus::Paused);
        let recover_all_files = readable.clone();
        v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .when(both, |this| {
                        this.child(
                            TabBar::new("carve-result-view")
                                .segmented()
                                .small()
                                .child(Tab::new().label("签名雕刻"))
                                .child(Tab::new().label("文件系统记录"))
                                .selected_index(match view {
                                    ResultView::Signatures => 0,
                                    ResultView::Filesystem => 1,
                                })
                                .on_click(cx.listener(|this, index: &usize, _, cx| {
                                    this.view = if *index == 1 {
                                        ResultView::Filesystem
                                    } else {
                                        ResultView::Signatures
                                    };
                                    cx.notify();
                                })),
                        )
                    })
                    .when(view == ResultView::Filesystem, |this| {
                        this.child(
                            div().w(px(280.)).child(
                                Input::new(&self.filter)
                                    .small()
                                    .cleanable(true)
                                    .prefix(Icon::new(IconName::Search).small()),
                            ),
                        )
                    })
                    .child(div().flex_1())
                    .when(can_pause, |this| {
                        this.child(
                            Button::new("pause-scan")
                                .small()
                                .icon(Icon::new(if paused {
                                    ExtraIcon::Play
                                } else {
                                    ExtraIcon::CircleStop
                                }))
                                .label(if paused {
                                    "继续扫描"
                                } else {
                                    "暂停扫描"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.pause_or_resume(cx))),
                        )
                    })
                    .child(
                        Button::new("recover-all")
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .label("恢复全部")
                            .tooltip("恢复全部雕刻结果与可读取的文件系统记录")
                            .disabled(scan.is_none() || (artifacts == 0 && readable.is_empty()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // An empty carved list with records selects the records only.
                                let artifact_ids = (artifacts == 0).then(Vec::new);
                                this.recover(artifact_ids, recover_all_files.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("new-scan")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label("新建扫描")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_form(window, cx)),
                            ),
                    ),
            )
            .when_some(
                scan.as_ref().and_then(|scan| scan.error.clone()),
                |this, error| this.child(div().px_3().pt_2().child(ui::error_text(error, cx))),
            )
            .when_some(
                scan.as_ref()
                    .and_then(|scan| scan.filesystem.as_ref())
                    .and_then(|filesystem| filesystem.error.clone()),
                |this, error| {
                    this.child(div().px_3().pt_2().child(Alert::error(
                        "filesystem-error",
                        format!("文件系统扫描失败：{error}"),
                    )))
                },
            )
            .when(view == ResultView::Filesystem, |this| {
                this.children(self.render_filesystem_summary(cx))
            })
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("carve-panels")
                        .child(
                            resizable_panel()
                                .size(px(260.))
                                .size_range(px(180.)..px(480.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .child(self.render_scans(cx)),
                                ),
                        )
                        .child(
                            resizable_panel().child(match view {
                                ResultView::Signatures => DataTable::new(&self.table)
                                    .bordered(false)
                                    .small()
                                    .into_any_element(),
                                ResultView::Filesystem if filesystem_pending => v_flex()
                                    .size_full()
                                    .child(ui::empty_state(
                                        Icon::new(ExtraIcon::FolderSearch),
                                        "正在扫描文件系统记录",
                                        Some("文件系统阶段完成后列出恢复到的文件。".into()),
                                    ))
                                    .into_any_element(),
                                ResultView::Filesystem => DataTable::new(&self.files)
                                    .bordered(false)
                                    .small()
                                    .into_any_element(),
                            }),
                        )
                        .child(
                            resizable_panel()
                                .size(px(360.))
                                .size_range(px(260.)..px(720.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_l_1()
                                        .border_color(cx.theme().border)
                                        .child(match view {
                                            ResultView::Signatures => {
                                                self.render_detail(cx).into_any_element()
                                            }
                                            ResultView::Filesystem => {
                                                self.render_file_detail(cx).into_any_element()
                                            }
                                        }),
                                ),
                        ),
                ),
            )
            .child(ui::page_footer(
                div().truncate().child(footer_left),
                footer_right,
                cx,
            ))
    }
}
