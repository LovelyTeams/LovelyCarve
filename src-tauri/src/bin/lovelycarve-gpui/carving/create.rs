//! The new-scan form: carving formats by category, the scanned region and
//! filesystem record recovery.
use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputState},
        radio::RadioGroup,
        scroll::ScrollableElement as _,
        v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window,
};
use lovelycarve_lib::core::{
    carving::carver::{CarveFormatInfo, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_RESULTS},
    evidence::image::ImageInfo,
};
use serde_json::json;

use crate::ui;

/// The form asks to start a scan with these options (the JSON shape of
/// `CarveStartOptions`).
#[derive(Clone, Debug)]
pub struct StartScan(pub serde_json::Value);

pub struct ScanForm {
    formats: Vec<CarveFormatInfo>,
    selected: BTreeSet<String>,
    /// `whole`, `unallocated`, `range` or a partition id.
    scopes: Vec<(String, SharedString)>,
    scope: usize,
    scope_open: bool,
    advanced_open: bool,
    total_sectors: u64,
    start_sector: Entity<InputState>,
    sector_count: Entity<InputState>,
    max_file_mib: Entity<InputState>,
    max_results: Entity<InputState>,
    filesystem: bool,
    signatures: bool,
    fat: bool,
    ntfs: bool,
    exfat: bool,
    include_existing: bool,
    error: Option<SharedString>,
}

impl EventEmitter<StartScan> for ScanForm {}

impl ScanForm {
    pub fn new(
        info: &ImageInfo,
        formats: Vec<CarveFormatInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut scopes = vec![
            ("whole".to_owned(), SharedString::from("整个镜像")),
            ("range".to_owned(), SharedString::from("自定义扇区")),
        ];
        if !info.partitions.is_empty() {
            scopes.insert(1, ("unallocated".to_owned(), "未分配空间".into()));
        }
        for partition in &info.partitions {
            scopes.push((
                partition.id.to_string(),
                format!(
                    "分区 {}（{}）",
                    partition.id,
                    crate::evidence::filesystem_label(&partition.kind)
                )
                .into(),
            ));
        }
        Self {
            selected: formats.iter().map(|format| format.id.clone()).collect(),
            formats,
            scopes,
            scope: 0,
            scope_open: false,
            advanced_open: false,
            total_sectors: info
                .size_bytes
                .checked_div(info.sector_size as u64)
                .unwrap_or_default(),
            start_sector: cx.new(|cx| InputState::new(window, cx).default_value("0")),
            sector_count: cx.new(|cx| InputState::new(window, cx).placeholder("留空扫描至末尾")),
            max_file_mib: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value((DEFAULT_MAX_FILE_SIZE / 1024 / 1024).to_string())
            }),
            max_results: cx.new(|cx| {
                InputState::new(window, cx).default_value(DEFAULT_MAX_RESULTS.to_string())
            }),
            filesystem: true,
            signatures: true,
            fat: true,
            ntfs: true,
            exfat: true,
            include_existing: false,
            error: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let filesystems = self.filesystem && (self.fat || self.ntfs || self.exfat);
        if !self.signatures && !filesystems {
            self.error = Some("请至少选择一种恢复方式".into());
            cx.notify();
            return;
        }
        if self.signatures && self.selected.is_empty() {
            self.error = Some("至少选择一种文件格式".into());
            cx.notify();
            return;
        }
        let (scope, partition) = match self.scopes.get(self.scope).map(|(key, _)| key.as_str()) {
            Some("unallocated") => ("unallocated", None),
            Some("range") => ("range", None),
            Some("whole") | None => ("whole", None),
            Some(partition) => ("partition", partition.parse::<u8>().ok()),
        };
        let (start_sector, sector_count) = if scope == "range" {
            let start = self
                .start_sector
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .ok();
            let count = self.sector_count.read(cx).value();
            let range = start.and_then(|start| {
                let remaining = self.total_sectors.checked_sub(start)?;
                let count = if count.trim().is_empty() {
                    remaining
                } else {
                    count.trim().parse().ok()?
                };
                (count > 0 && count <= remaining).then_some((start, count))
            });
            let Some((start, count)) = range else {
                self.error = Some("请输入镜像范围内的整数扇区范围。".into());
                self.scope_open = true;
                cx.notify();
                return;
            };
            (Some(start), Some(count))
        } else {
            (None, None)
        };
        let (max_file_size, max_results) = if self.signatures {
            let size = self
                .max_file_mib
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|size| size.checked_mul(1024 * 1024))
                .filter(|&size| size > 0);
            let results = self
                .max_results
                .read(cx)
                .value()
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|&count| count > 0);
            let (Some(size), Some(results)) = (size, results) else {
                self.error = Some("请输入有效的文件大小和结果数量上限。".into());
                self.advanced_open = true;
                cx.notify();
                return;
            };
            (size, results)
        } else {
            (DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_RESULTS)
        };
        self.error = None;
        cx.emit(StartScan(json!({
            "format_ids": self.selected.iter().collect::<Vec<_>>(),
            "scope": scope,
            "partition_id": partition,
            "start_sector": start_sector,
            "sector_count": sector_count,
            "max_file_size": max_file_size,
            "max_results": max_results,
            "signatures": self.signatures,
            "filesystems": {
                "fat": filesystems && self.fat,
                "ntfs": filesystems && self.ntfs,
                "exfat": filesystems && self.exfat,
                "include_existing": filesystems && self.include_existing,
            },
        })));
    }
}

/// Reaches one of the form's on/off options.
type FlagAccess = fn(&mut ScanForm) -> &mut bool;

impl Render for ScanForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut categories: BTreeMap<String, Vec<CarveFormatInfo>> = BTreeMap::new();
        for format in &self.formats {
            categories
                .entry(format.category.clone())
                .or_default()
                .push(format.clone());
        }
        let filesystem_options: [(&str, &str, bool, FlagAccess); 4] = [
            ("recover-ntfs", "NTFS 记录", self.ntfs, |this| {
                &mut this.ntfs
            }),
            ("recover-fat", "FAT 目录项", self.fat, |this| {
                &mut this.fat
            }),
            ("recover-exfat", "exFAT 目录项", self.exfat, |this| {
                &mut this.exfat
            }),
            (
                "recover-existing",
                "同时列出现存文件",
                self.include_existing,
                |this| &mut this.include_existing,
            ),
        ];
        let scope = self.scopes.get(self.scope);
        let range = scope.is_some_and(|(key, _)| key == "range");
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_3()
                    .max_h(px(540.))
                    .overflow_y_scrollbar()
                    .child(ui::section_title("恢复方式", cx))
                    .child(
                        h_flex().gap_2().children(
                            [
                                (
                                    "recover-filesystem",
                                    "文件系统恢复",
                                    "读取已删除记录并重建丢失分区",
                                    gpui_kit::assets::IconName::FolderTree,
                                    self.filesystem,
                                    (|this: &mut Self| &mut this.filesystem) as FlagAccess,
                                ),
                                (
                                    "signatures",
                                    "签名雕刻",
                                    "按文件签名查找没有目录记录的文件",
                                    gpui_kit::assets::IconName::FileSearch,
                                    self.signatures,
                                    (|this: &mut Self| &mut this.signatures) as FlagAccess,
                                ),
                            ]
                            .map(
                                |(id, label, hint, icon, checked, flag)| {
                                    ui::card(cx)
                                        .flex_1()
                                        .p_3()
                                        .gap_2()
                                        .child(
                                            h_flex().gap_2().child(Icon::new(icon).small()).child(
                                                Checkbox::new(id)
                                                    .label(label)
                                                    .checked(checked)
                                                    .on_click(cx.listener(
                                                        move |this, value: &bool, _, cx| {
                                                            *flag(this) = *value;
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(hint),
                                        )
                                },
                            ),
                        ),
                    )
                    .when(self.filesystem, |form| {
                        form.child(
                            h_flex().gap_3().flex_wrap().children(
                                filesystem_options
                                    .into_iter()
                                    .map(|(id, label, checked, flag)| {
                                        Checkbox::new(id).label(label).checked(checked).on_click(
                                            cx.listener(move |this, value: &bool, _, cx| {
                                                *flag(this) = *value;
                                                cx.notify();
                                            }),
                                        )
                                    }),
                            ),
                        )
                    })
                    .when(self.signatures, |form| {
                        form.child(
                            h_flex()
                                .gap_2()
                                .child(ui::section_title(
                                    format!(
                                        "文件类型 · {} / {}",
                                        self.selected.len(),
                                        self.formats.len()
                                    ),
                                    cx,
                                ))
                                .child(div().flex_1())
                                .child(
                                    Button::new("select-all-formats")
                                        .xsmall()
                                        .ghost()
                                        .label("全选")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected = this
                                                .formats
                                                .iter()
                                                .map(|format| format.id.clone())
                                                .collect();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("clear-formats")
                                        .xsmall()
                                        .ghost()
                                        .label("清空")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected.clear();
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .when(self.signatures, |this| {
                        this.child(
                            v_flex()
                                .max_h(gpui_kit::px(260.))
                                .gap_2()
                                .children(categories.into_iter().map(|(category, formats)| {
                                    let label = match category.as_str() {
                                        "image" => "图像",
                                        "document" => "文档",
                                        "archive" => "归档",
                                        "audio" => "音频",
                                        "video" => "视频",
                                        "database" => "数据库",
                                        _ => &category,
                                    };
                                    let ids: Vec<_> =
                                        formats.iter().map(|format| format.id.clone()).collect();
                                    let all_selected =
                                        ids.iter().all(|id| self.selected.contains(id));
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Checkbox::new(SharedString::from(format!(
                                                "category-{category}"
                                            )))
                                            .label(label.to_owned())
                                            .checked(all_selected)
                                            .on_click(cx.listener(
                                                move |this, checked: &bool, _, cx| {
                                                    if *checked {
                                                        this.selected.extend(ids.iter().cloned());
                                                    } else {
                                                        this.selected
                                                            .retain(|id| !ids.contains(id));
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                        )
                                        .child(h_flex().gap_3().flex_wrap().children(
                                            formats.into_iter().map(|format| {
                                                let id = format.id.clone();
                                                Checkbox::new(SharedString::from(format!(
                                                    "format-{id}"
                                                )))
                                                .label(format.label.clone())
                                                .tooltip(format.description.clone())
                                                .checked(self.selected.contains(&format.id))
                                                .on_click(cx.listener(
                                                    move |this, checked: &bool, _, cx| {
                                                        if *checked {
                                                            this.selected.insert(id.clone());
                                                        } else {
                                                            this.selected.remove(&id);
                                                        }
                                                        cx.notify();
                                                    },
                                                ))
                                            }),
                                        ))
                                }))
                                .overflow_y_scrollbar(),
                        )
                    })
                    .child(
                        ui::card(cx)
                            .child(
                                ui::disclosure_button(
                                    "carve-scope-settings",
                                    "扫描范围",
                                    scope.map(|(_, label)| label.clone()).unwrap_or_default(),
                                    self.scope_open,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.scope_open = !this.scope_open;
                                        cx.notify();
                                    },
                                )),
                            )
                            .when(self.scope_open, |card| {
                                card.child(
                                    v_flex()
                                        .p_3()
                                        .gap_3()
                                        .child(
                                            RadioGroup::vertical("carve-scope")
                                                .children(
                                                    self.scopes
                                                        .iter()
                                                        .map(|(_, label)| label.clone()),
                                                )
                                                .selected_index(Some(self.scope))
                                                .on_change(cx.listener(
                                                    |this, index: &usize, _, cx| {
                                                        this.scope = *index;
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                        .when(range, |body| {
                                            body.child(ui::field_row(
                                                "起始扇区",
                                                Input::new(&self.start_sector),
                                                cx,
                                            ))
                                            .child(
                                                ui::field_row(
                                                    "扇区数量",
                                                    Input::new(&self.sector_count),
                                                    cx,
                                                ),
                                            )
                                        }),
                                )
                            }),
                    )
                    .when(self.signatures, |form| {
                        form.child(
                            ui::card(cx)
                                .child(
                                    ui::disclosure_button(
                                        "carve-advanced",
                                        "高级选项",
                                        "文件大小与结果上限",
                                        self.advanced_open,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.advanced_open = !this.advanced_open;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .when(self.advanced_open, |card| {
                                    card.child(
                                        v_flex()
                                            .p_3()
                                            .gap_3()
                                            .child(ui::field_row(
                                                "单文件上限（MiB）",
                                                Input::new(&self.max_file_mib),
                                                cx,
                                            ))
                                            .child(ui::field_row(
                                                "结果数量上限",
                                                Input::new(&self.max_results),
                                                cx,
                                            )),
                                    )
                                }),
                        )
                    })
                    .when_some(self.error.clone(), |this, error| {
                        this.child(ui::error_text(error, cx))
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("cancel-scan")
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("start-scan")
                            .primary()
                            .label("开始扫描")
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}
