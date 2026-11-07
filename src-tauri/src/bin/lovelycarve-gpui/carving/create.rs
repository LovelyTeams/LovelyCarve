//! The new-scan form: carving formats by category, the scanned region and
//! filesystem record recovery.
use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        radio::RadioGroup,
        scroll::ScrollableElement as _,
        v_flex, ActiveTheme as _, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    Context, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window,
};
use lovelycarve_lib::core::{carving::carver::CarveFormatInfo, evidence::image::ImageInfo};
use serde_json::json;

use crate::ui;

/// The form asks to start a scan with these options (the JSON shape of
/// `CarveStartOptions`).
#[derive(Clone, Debug)]
pub struct StartScan(pub serde_json::Value);

pub struct ScanForm {
    formats: Vec<CarveFormatInfo>,
    selected: BTreeSet<String>,
    /// `whole`, `unallocated` or a partition id.
    scopes: Vec<(String, SharedString)>,
    scope: usize,
    signatures: bool,
    fat: bool,
    ntfs: bool,
    exfat: bool,
    include_existing: bool,
    error: Option<SharedString>,
}

impl EventEmitter<StartScan> for ScanForm {}

impl ScanForm {
    pub fn new(info: &ImageInfo, formats: Vec<CarveFormatInfo>) -> Self {
        let mut scopes = vec![
            ("whole".to_owned(), SharedString::from("整个镜像")),
            ("unallocated".to_owned(), SharedString::from("未分配空间")),
        ];
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
            signatures: true,
            fat: false,
            ntfs: false,
            exfat: false,
            include_existing: false,
            error: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let filesystems = self.fat || self.ntfs || self.exfat;
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
            Some("whole") | None => ("whole", None),
            Some(partition) => ("partition", partition.parse::<u8>().ok()),
        };
        self.error = None;
        cx.emit(StartScan(json!({
            "format_ids": self.selected.iter().collect::<Vec<_>>(),
            "scope": scope,
            "partition_id": partition,
            "signatures": self.signatures,
            "filesystems": {
                "fat": self.fat,
                "ntfs": self.ntfs,
                "exfat": self.exfat,
                "include_existing": self.include_existing,
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
        v_flex()
            .w_full()
            .gap_3()
            .child(ui::section_title("扫描范围", cx))
            .child(
                RadioGroup::horizontal("carve-scope")
                    .children(self.scopes.iter().map(|(_, label)| label.clone()))
                    .selected_index(Some(self.scope))
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.scope = *index;
                        cx.notify();
                    })),
            )
            .child(ui::section_title("文件系统记录恢复", cx))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .children(
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
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("signatures")
                            .label("按文件签名雕刻")
                            .checked(self.signatures)
                            .on_click(cx.listener(|this, value: &bool, _, cx| {
                                this.signatures = *value;
                                cx.notify();
                            })),
                    )
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
            .when(self.signatures, |this| {
                this.child(
                    v_flex()
                        .max_h(gpui_kit::px(260.))
                        .gap_2()
                        .children(categories.into_iter().map(|(category, formats)| {
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(category),
                                )
                                .child(h_flex().gap_3().flex_wrap().children(
                                    formats.into_iter().map(|format| {
                                        let id = format.id.clone();
                                        Checkbox::new(SharedString::from(format!("format-{id}")))
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
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("start-scan")
                        .primary()
                        .label("开始扫描")
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
            )
    }
}
