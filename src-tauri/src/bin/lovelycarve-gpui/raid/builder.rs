//! 手动重组: assemble a RAID device from a geometry the examiner supplies —
//! member columns in order (with missing ones), level, layout, chunk size and
//! data offset — when neither metadata nor the heuristic recovers it.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        radio::RadioGroup,
        scroll::ScrollableElement as _,
        v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::{app_error::AppError, evidence::image as image_commands},
    host::State,
};
use serde::Deserialize;
use serde_json::json;

use super::{open_synthetic, parity_layout_name, raid10_layout_name, StackImage};
use crate::{backend::Backend, evidence::EvidenceTab, format, ui};

const LEVELS: [(i32, &str); 7] = [
    (-1, "线性"),
    (0, "RAID 0"),
    (1, "RAID 1"),
    (4, "RAID 4"),
    (5, "RAID 5"),
    (6, "RAID 6"),
    (10, "RAID 10"),
];
const RAID5_LAYOUTS: [u32; 6] = [0, 1, 2, 3, 4, 5];
const RAID6_LAYOUTS: [u32; 14] = [0, 1, 2, 3, 4, 5, 8, 9, 10, 16, 17, 18, 19, 20];
/// Near 2, far 2 and offset 2 copies.
const RAID10_LAYOUTS: [u32; 3] = [0x0102, 0x0201, 0x1_0201];

#[derive(Clone, Debug, Deserialize)]
struct Built {
    image_id: String,
    size_bytes: u64,
    partitions: Vec<BuiltPartition>,
}

#[derive(Clone, Debug, Deserialize)]
struct BuiltPartition {
    kind: String,
    start_lba: u64,
    length_lba: u64,
    label: Option<String>,
}

pub struct ManualBuilder {
    images: Vec<StackImage>,
    tab: WeakEntity<EvidenceTab>,
    /// Image per column, `None` for a column whose disk is absent.
    columns: Vec<Option<String>>,
    level: i32,
    parity_layout: u32,
    raid10_layout: u32,
    chunk_kib: Entity<InputState>,
    data_offset: Entity<InputState>,
    running: bool,
    built: Option<Built>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl ManualBuilder {
    pub fn new(
        images: Vec<StackImage>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chunk_kib = cx.new(|cx| InputState::new(window, cx).default_value("32"));
        let data_offset = cx.new(|cx| InputState::new(window, cx).default_value("2048"));
        let subscriptions = [&chunk_kib, &data_offset]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.built = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            images,
            tab,
            columns: Vec::new(),
            level: 5,
            parity_layout: 2,
            raid10_layout: RAID10_LAYOUTS[0],
            chunk_kib,
            data_offset,
            running: false,
            built: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn image_name(&self, image_id: &str) -> String {
        self.images
            .iter()
            .find(|image| image.image_id == image_id)
            .map(|image| format::file_name(&image.path).to_owned())
            .unwrap_or_else(|| image_id.chars().take(8).collect())
    }

    fn layout(&self) -> u32 {
        match self.level {
            5 | 6 => self.parity_layout,
            10 => self.raid10_layout,
            _ => 0,
        }
    }

    /// Why the geometry cannot be assembled, if it cannot.
    fn problem(&self, cx: &gpui_kit::App) -> Option<String> {
        let present = self
            .columns
            .iter()
            .filter(|column| column.is_some())
            .count();
        let missing = self.columns.len() - present;
        if present == 0 {
            return Some("至少加入一块成员盘".to_owned());
        }
        let tolerated = match self.level {
            -1 | 0 => 0,
            1 => self.columns.len() - 1,
            4 | 5 => 1,
            6 => 2,
            _ => self.columns.len() / 2,
        };
        if missing > tolerated {
            return Some(format!(
                "{} 最多缺 {tolerated} 列",
                if self.level == -1 {
                    "线性".to_owned()
                } else {
                    format!("RAID {}", self.level)
                }
            ));
        }
        let chunk = self
            .chunk_kib
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        if !matches!(self.level, -1 | 1) && chunk == 0 {
            return Some("块大小必须大于 0".to_owned());
        }
        if self
            .data_offset
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .is_err()
        {
            return Some("数据偏移须为扇区数".to_owned());
        }
        None
    }

    fn assemble(&mut self, cx: &mut Context<Self>) {
        if self.running || self.problem(cx).is_some() {
            return;
        }
        let chunk = self
            .chunk_kib
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        let data_offset = self
            .data_offset
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        let params = json!({
            "member_image_ids": self.columns.iter().map(|column| column.clone().unwrap_or_default()).collect::<Vec<_>>(),
            "level": self.level,
            "layout": self.layout(),
            "chunk_sectors": if self.level == 1 { 0 } else { chunk * 2 },
            "data_offset_sectors": data_offset,
            "missing_slots": self
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column.is_none())
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>(),
        });
        self.running = true;
        self.error = None;
        self.built = None;
        let assembling = Backend::global(cx).run(move |_, state| async move {
            let params = serde_json::from_value(params)
                .map_err(|error| AppError::invalid_input(error.to_string()))?;
            let built = image_commands::raid_manual_assemble(State::new(&state), params).await?;
            serde_json::to_value(built).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let assembled = assembling.await;
            this.update(cx, |this, cx| {
                this.running = false;
                match assembled.map(serde_json::from_value::<Built>) {
                    Ok(Ok(built)) => this.built = Some(built),
                    Ok(Err(error)) => this.error = Some(error.to_string().into()),
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_columns(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let last = self.columns.len().saturating_sub(1);
        v_flex()
            .gap_1()
            .when(self.columns.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("从下方已加载镜像中按列顺序加入成员盘。"),
                )
            })
            .children(self.columns.iter().enumerate().map(|(slot, column)| {
                h_flex()
                    .gap_2()
                    .child(ui::mono(format!("列 {slot}"), cx).text_xs().w(px(44.)))
                    .child(match column {
                        Some(image) => div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(self.image_name(image)),
                        None => div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child("缺失（由校验或镜像重建）"),
                    })
                    .child(
                        Button::new(("column-up", slot))
                            .xsmall()
                            .ghost()
                            .icon(IconName::ArrowUp)
                            .disabled(slot == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.columns.swap(slot, slot - 1);
                                this.built = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("column-down", slot))
                            .xsmall()
                            .ghost()
                            .icon(IconName::ArrowDown)
                            .disabled(slot == last)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.columns.swap(slot, slot + 1);
                                this.built = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("column-remove", slot))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.columns.remove(slot);
                                this.built = None;
                                cx.notify();
                            })),
                    )
            }))
    }

    fn render_result(&self, built: &Built, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let image = built.image_id.clone();
        v_flex()
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(cx.theme().muted)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().child(format!(
                        "已重组为虚拟镜像，容量 {}",
                        format::binary_bytes(built.size_bytes)
                    )))
                    .child(
                        Button::new("open-built")
                            .small()
                            .primary()
                            .label("打开重组镜像")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.close_dialog(cx);
                                open_synthetic(image.clone(), this.tab.clone(), window, cx);
                            })),
                    ),
            )
            .when(built.partitions.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("该几何未识别出分区表或文件系统，可调整参数后重试。"),
                )
            })
            .children(built.partitions.iter().map(|partition| {
                div().text_xs().text_color(muted).child(format!(
                    "{} · {} · 起始 {} · {} 扇区",
                    partition
                        .label
                        .clone()
                        .filter(|label| !label.is_empty())
                        .unwrap_or_else(|| "分区".to_owned()),
                    partition.kind,
                    format::count(partition.start_lba),
                    format::count(partition.length_lba)
                ))
            }))
    }
}

impl Render for ManualBuilder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let level = self.level;
        let problem = self.problem(cx);
        let layouts: Vec<(u32, String)> = match level {
            5 => RAID5_LAYOUTS
                .iter()
                .map(|layout| (*layout, parity_layout_name(*layout)))
                .collect(),
            6 => RAID6_LAYOUTS
                .iter()
                .map(|layout| (*layout, parity_layout_name(*layout)))
                .collect(),
            10 => RAID10_LAYOUTS
                .iter()
                .map(|layout| (*layout, raid10_layout_name(*layout)))
                .collect(),
            _ => Vec::new(),
        };
        let selected_layout = layouts
            .iter()
            .position(|(layout, _)| *layout == self.layout());
        v_flex()
            .w_full()
            .gap_2()
            .child(
                v_flex()
                    .max_h(px(520.))
                    .gap_2()
                    .child(ui::section_title("成员列（按条带顺序）", cx))
                    .child(self.render_columns(cx))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .children(self.images.iter().enumerate().map(|(index, image)| {
                                let id = image.image_id.clone();
                                Button::new(("add-member", index))
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .label(format!(
                                        "{} · {}",
                                        format::file_name(&image.path),
                                        format::binary_bytes(image.size_bytes)
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.columns.push(Some(id.clone()));
                                        this.built = None;
                                        cx.notify();
                                    }))
                            }))
                            .child(
                                Button::new("add-missing")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .label("缺失列")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.columns.push(None);
                                        this.built = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(self.images.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("尚无已加载镜像；先把成员盘作为检材加载，再刷新检测。"),
                        )
                    })
                    .child(ui::section_title("级别", cx))
                    .child(
                        RadioGroup::horizontal("raid-level")
                            .children(LEVELS.iter().map(|(_, label)| *label))
                            .selected_index(
                                LEVELS.iter().position(|(candidate, _)| *candidate == level),
                            )
                            .on_change(cx.listener(|this, index: &usize, _, cx| {
                                if let Some((level, _)) = LEVELS.get(*index) {
                                    this.level = *level;
                                    this.built = None;
                                    cx.notify();
                                }
                            })),
                    )
                    .when(!layouts.is_empty(), |this| {
                        let labels: Vec<SharedString> = layouts
                            .iter()
                            .map(|(_, label)| SharedString::from(label.clone()))
                            .collect();
                        let values: Vec<u32> = layouts.iter().map(|(layout, _)| *layout).collect();
                        this.child(ui::section_title(
                            if level == 10 {
                                "副本布局"
                            } else {
                                "校验布局"
                            },
                            cx,
                        ))
                        .child(
                            RadioGroup::horizontal("raid-layout")
                                .children(labels)
                                .selected_index(selected_layout)
                                .on_change(cx.listener(move |this, index: &usize, _, cx| {
                                    if let Some(layout) = values.get(*index).copied() {
                                        if this.level == 10 {
                                            this.raid10_layout = layout;
                                        } else {
                                            this.parity_layout = layout;
                                        }
                                        this.built = None;
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_4()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("块大小（KiB）"))
                                    .child(
                                        div().w(px(120.)).child(
                                            Input::new(&self.chunk_kib)
                                                .small()
                                                .disabled(matches!(level, -1 | 1)),
                                        ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div().text_xs().text_color(muted).child("数据偏移（扇区）"),
                                    )
                                    .child(
                                        div()
                                            .w(px(140.))
                                            .child(Input::new(&self.data_offset).small()),
                                    ),
                            ),
                    )
                    .overflow_y_scrollbar(),
            )
            .when_some(problem.clone(), |this, problem| {
                this.child(ui::error_text(problem, cx).text_xs())
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .when_some(self.built.clone(), |this, built| {
                this.child(self.render_result(&built, cx))
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("assemble-raid")
                        .primary()
                        .label(if self.running {
                            "正在重组…"
                        } else {
                            "重组"
                        })
                        .loading(self.running)
                        .disabled(self.running || problem.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.assemble(cx))),
                ),
            )
    }
}
