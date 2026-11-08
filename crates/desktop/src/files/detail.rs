//! The detail pane of the file list: metadata, decoded text and hex bytes of
//! the selected node.
#[cfg(all(windows, target_arch = "x86_64"))]
mod native;
mod source;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        scroll::ScrollableElement as _,
        spinner::Spinner,
        tab::{Tab, TabBar},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list, AppContext as _, ClipboardItem, Context, Entity, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, UniformListScrollHandle,
    WeakEntity, Window,
};
use lovelycarve_backend::{
    commands::{
        app_error::AppError, filesystem::digests as digest_commands,
        preview::filesystem_preview as preview_commands,
    },
    core::{
        catalog::flat_query::flat_ext_of,
        filesystem::tree::fs_tree::{FsNode, FsStat, NodeKind},
    },
    host::State,
};
use serde_json::Value;

use super::{
    hex::{HexView, WINDOW_BYTES},
    icons::node_icon,
    source::{source_language, SourceLanguage},
    table::{created_time, kind_label, tag_chip, FILE_TAGS},
    FileBrowser,
};
use crate::{backend::Backend, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// Bytes decoded per text page.
const TEXT_PAGE_BYTES: usize = 256 * 1024;
/// Lines kept for display; the rest stays one "load more" away.
const TEXT_LINE_LIMIT: usize = 50_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Info,
    Text,
    Hex,
    Native,
}

enum TextState {
    Idle,
    Loading,
    Loaded {
        lines: Vec<SharedString>,
        encoding: SharedString,
        had_errors: bool,
        next_offset: Option<u64>,
    },
    Failed(SharedString),
}

pub struct FileDetail {
    browser: WeakEntity<FileBrowser>,
    image_id: String,
    node: Option<FsNode>,
    path: String,
    tab: DetailTab,
    stat: Option<Result<FsStat, SharedString>>,
    text: TextState,
    text_scroll: UniformListScrollHandle,
    hex: Entity<HexView>,
    source_language: Option<SourceLanguage>,
    source: source::SourcePreview,
    #[cfg(all(windows, target_arch = "x86_64"))]
    native: native::NativePreview,
    offset_input: Entity<InputState>,
    /// The node's digests kept with the case (`StoredDigest` JSON).
    digest: Option<Value>,
    /// Bumped when the node changes, so late answers for the old one drop.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl FileDetail {
    pub fn new(
        browser: WeakEntity<FileBrowser>,
        image_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hex = cx.new(|_| HexView::new(image_id.clone()));
        let offset_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("跳转偏移，如 0x1000"));
        let subscriptions = vec![cx.subscribe_in(
            &offset_input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let value = input.read(cx).value().to_string();
                    this.jump_to(&value, cx);
                }
            },
        )];
        Self {
            browser,
            image_id,
            node: None,
            path: String::new(),
            tab: DetailTab::Info,
            stat: None,
            text: TextState::Idle,
            text_scroll: UniformListScrollHandle::new(),
            hex,
            source_language: None,
            source: source::SourcePreview::Idle,
            #[cfg(all(windows, target_arch = "x86_64"))]
            native: native::NativePreview::Idle,
            offset_input,
            digest: None,
            generation: 0,
            _subscriptions: subscriptions,
        }
    }

    /// New digests were kept for these nodes (`node id → StoredDigest`).
    pub fn digests_changed(&mut self, stored: &Value, cx: &mut Context<Self>) {
        if let Some(digest) = self.node.as_ref().and_then(|node| stored.get(&node.id)) {
            self.digest = Some(digest.clone());
            cx.notify();
        }
    }

    fn load_digest(&mut self, cx: &mut Context<Self>) {
        let Some(node_id) = self.readable().map(|node| node.id.clone()) else {
            return;
        };
        let image_id = self.image_id.clone();
        let generation = self.generation;
        let lookup = node_id.clone();
        let loading = Backend::global(cx).run(move |_, state| async move {
            let stored =
                digest_commands::fs_digest_load_nodes(State::new(&state), image_id, vec![lookup])
                    .await?;
            serde_json::to_value(stored).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let Ok(stored) = loading.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.digest = stored.get(&node_id).cloned();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn node(&self) -> Option<&FsNode> {
        self.node.as_ref()
    }

    /// Show `node` (at `path`), or nothing.
    pub fn show(&mut self, node: Option<FsNode>, path: String, cx: &mut Context<Self>) {
        if self.node.as_ref().map(|node| &node.id) == node.as_ref().map(|node| &node.id) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.node = node;
        self.path = path;
        self.stat = None;
        self.text = TextState::Idle;
        self.source = source::SourcePreview::Idle;
        self.source_language = self
            .node
            .as_ref()
            .filter(|node| node.kind == NodeKind::File)
            .and_then(|node| source_language(&node.name, &flat_ext_of(node)));
        if self.tab == DetailTab::Native && !self.has_native_tab() {
            self.tab = DetailTab::Info;
        }
        #[cfg(all(windows, target_arch = "x86_64"))]
        self.close_native(cx);
        self.digest = None;
        let (node_id, size) = match &self.node {
            Some(node) if node.kind != NodeKind::Dir => (Some(node.id.clone()), node.size),
            _ => (None, 0),
        };
        self.hex.update(cx, |hex, cx| hex.show(node_id, size, cx));
        self.load_stat(cx);
        self.load_digest(cx);
        if self.tab == DetailTab::Text {
            self.load_text(None, cx);
        }
        cx.notify();
    }

    fn select_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        #[cfg(all(windows, target_arch = "x86_64"))]
        if tab != DetailTab::Native {
            self.close_native(cx);
        }
        self.tab = tab;
        if tab == DetailTab::Text && matches!(self.text, TextState::Idle) {
            self.load_text(None, cx);
        }
        cx.notify();
    }

    fn readable(&self) -> Option<&FsNode> {
        self.node.as_ref().filter(|node| node.kind != NodeKind::Dir)
    }

    fn has_native_tab(&self) -> bool {
        cfg!(all(windows, target_arch = "x86_64")) || self.source_language.is_some()
    }

    fn load_stat(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.readable() else {
            return;
        };
        let image_id = self.image_id.clone();
        let node_id = node.id.clone();
        let generation = self.generation;
        let stat = Backend::global(cx).run(move |_, state| async move {
            preview_commands::fs_stat(State::new(&state), image_id, node_id).await
        });
        cx.spawn(async move |this, cx| {
            let stat = stat.await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.stat = Some(stat.map_err(|error| error.message));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_text(&mut self, offset: Option<u64>, cx: &mut Context<Self>) {
        let Some(node_id) = self.readable().map(|node| node.id.clone()) else {
            return;
        };
        if offset.is_none() {
            self.text = TextState::Loading;
        }
        let image_id = self.image_id.clone();
        let generation = self.generation;
        let page = Backend::global(cx).run(move |_, state| async move {
            preview_commands::fs_preview_text(
                State::new(&state),
                image_id,
                node_id,
                TEXT_PAGE_BYTES,
                offset,
                None,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let page = page.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match page {
                    Ok(page) => {
                        let new_lines = page
                            .text
                            .lines()
                            .map(|line| SharedString::from(line.to_owned()));
                        match &mut this.text {
                            TextState::Loaded {
                                lines,
                                next_offset,
                                had_errors,
                                ..
                            } if offset.is_some() => {
                                lines.extend(new_lines);
                                lines.truncate(TEXT_LINE_LIMIT);
                                *next_offset = page.next_offset;
                                *had_errors |= page.had_errors;
                            }
                            _ => {
                                let mut lines: Vec<SharedString> = new_lines.collect();
                                lines.truncate(TEXT_LINE_LIMIT);
                                this.text = TextState::Loaded {
                                    lines,
                                    encoding: page.encoding.into(),
                                    had_errors: page.had_errors,
                                    next_offset: page.next_offset,
                                };
                            }
                        }
                    }
                    Err(error) => this.text = TextState::Failed(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn jump_to(&mut self, value: &str, cx: &mut Context<Self>) {
        let value = value.trim();
        let parsed = match value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => value.parse::<u64>().ok(),
        };
        if let Some(offset) = parsed {
            self.hex.update(cx, |hex, cx| hex.jump(offset, cx));
        }
    }

    fn render_tag(&self, node: &FsNode, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self
            .browser
            .upgrade()
            .and_then(|browser| browser.read(cx).tag_of(&node.id, cx));
        let browser = self.browser.clone();
        let clear_browser = self.browser.clone();
        let (node_id, clear_id) = (node.id.clone(), node.id.clone());
        ui::field_row(
            "标签",
            h_flex()
                .gap_1()
                .flex_wrap()
                .map(|this| match &current {
                    Some(tag) => this.child(tag_chip(tag)),
                    None => this.child(div().text_color(cx.theme().muted_foreground).child("无")),
                })
                .children(
                    FILE_TAGS
                        .iter()
                        .enumerate()
                        .filter(|(_, tag)| current.as_deref() != Some(**tag))
                        .map(|(index, tag)| {
                            let (browser, node_id) = (browser.clone(), node_id.clone());
                            Button::new(("detail-tag", index))
                                .xsmall()
                                .ghost()
                                .label(*tag)
                                .on_click(move |_, window, cx| {
                                    browser
                                        .update(cx, |browser, cx| {
                                            browser.apply_tag(
                                                vec![node_id.clone()],
                                                Some(tag),
                                                window,
                                                cx,
                                            )
                                        })
                                        .ok();
                                })
                        }),
                )
                .when(current.is_some(), |this| {
                    this.child(
                        Button::new("detail-untag")
                            .xsmall()
                            .ghost()
                            .label("清除")
                            .on_click(move |_, window, cx| {
                                clear_browser
                                    .update(cx, |browser, cx| {
                                        browser.apply_tag(vec![clear_id.clone()], None, window, cx)
                                    })
                                    .ok();
                            }),
                    )
                }),
            cx,
        )
    }

    fn render_digest(&self, node: &FsNode, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<(&str, String)> = ["md5", "sha1", "sha256", "sm3"]
            .into_iter()
            .filter_map(|key| {
                let value = self.digest.as_ref()?.get(key)?.as_str()?.to_owned();
                Some((key, value))
            })
            .collect();
        let browser = self.browser.clone();
        let target = node.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(ui::section_title("文件哈希", cx)))
                    .child(
                        Button::new("detail-hash")
                            .xsmall()
                            .ghost()
                            .label(if rows.is_empty() {
                                "计算哈希"
                            } else {
                                "重新计算"
                            })
                            .on_click(move |_, window, cx| {
                                browser
                                    .update(cx, |browser, cx| {
                                        browser.hash_nodes(vec![target.clone()], window, cx)
                                    })
                                    .ok();
                            }),
                    ),
            )
            .when(rows.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("尚未计算；结果随案件保存。"),
                )
            })
            .children(rows.into_iter().map(|(key, value)| {
                let copied = value.clone();
                ui::field_row(
                    key.to_uppercase(),
                    h_flex()
                        .gap_1()
                        .child(ui::mono(value, cx).text_xs().flex_1().min_w_0())
                        .child(
                            Button::new(SharedString::from(format!("copy-digest-{key}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .tooltip(format!("复制 {}", key.to_uppercase()))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                                }),
                        ),
                    cx,
                )
            }))
    }

    fn render_info(&self, node: &FsNode, cx: &mut Context<Self>) -> impl IntoElement {
        let timestamps = [
            ("修改时间", node.timestamps.modified.as_deref()),
            ("访问时间", node.timestamps.accessed.as_deref()),
            ("创建时间", created_time(node)),
            ("元数据变更", node.timestamps.mft_changed.as_deref()),
        ];
        let path = self.path.clone();
        v_flex()
            .gap_1()
            .child(ui::section_title("基本信息", cx))
            .child(ui::field_row(
                "路径",
                h_flex()
                    .gap_1()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(path.clone()))
                    .child(
                        Button::new("copy-node-path")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("复制路径")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(path.clone()))
                            }),
                    ),
                cx,
            ))
            .child(ui::field_row("类型", kind_label(node), cx))
            .when(node.kind != NodeKind::Dir, |this| {
                this.child(ui::field_row(
                    "大小",
                    format!(
                        "{}（{} 字节）",
                        format::bytes(node.size),
                        format::count(node.size)
                    ),
                    cx,
                ))
            })
            .when_some(node.inode, |this, inode| {
                this.child(ui::field_row(
                    "Inode / MFT",
                    ui::mono(inode.to_string(), cx),
                    cx,
                ))
            })
            .when_some(node.link_target.clone(), |this, target| {
                this.child(ui::field_row("链接目标", target, cx))
            })
            .child(ui::field_row("节点 ID", ui::mono(node.id.clone(), cx), cx))
            .child(self.render_tag(node, cx))
            .when(node.kind != NodeKind::Dir, |this| {
                this.child(self.render_digest(node, cx))
            })
            .child(ui::section_title("时间", cx))
            .children(timestamps.into_iter().filter_map(|(label, value)| {
                value.map(|value| {
                    ui::field_row(label, ui::mono(format::timestamp(Some(value)), cx), cx)
                })
            }))
            .when_some(self.stat.as_ref(), |this, stat| {
                this.child(ui::section_title("内容", cx)).child(match stat {
                    Ok(stat) => v_flex()
                        .gap_1()
                        .child(ui::field_row("MIME", stat.mime.clone(), cx))
                        .when_some(stat.magic_hex.clone(), |this, magic| {
                            this.child(ui::field_row("文件头", ui::mono(magic, cx), cx))
                        })
                        .when_some(stat.entropy, |this, entropy| {
                            this.child(ui::field_row("熵", format!("{entropy:.3} bit/字节"), cx))
                        })
                        .when(!stat.data_runs.is_empty(), |this| {
                            this.child(ui::field_row(
                                "数据段",
                                format!("{} 段", stat.data_runs.len()),
                                cx,
                            ))
                        })
                        .into_any_element(),
                    Err(error) => ui::error_text(error.clone(), cx).into_any_element(),
                })
            })
    }

    fn render_text(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.text {
            TextState::Idle | TextState::Loading => h_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child("正在解码文本…")
                .into_any_element(),
            TextState::Failed(error) => div()
                .p_4()
                .child(ui::error_text(error.clone(), cx))
                .into_any_element(),
            TextState::Loaded {
                lines,
                encoding,
                had_errors,
                next_offset,
            } => {
                let lines = lines.clone();
                let next_offset = *next_offset;
                let count = lines.len();
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("编码 {encoding}"))
                            .when(*had_errors, |this| {
                                this.child(Tag::warning().xsmall().child("含无法解码的字节"))
                            })
                            .child(div().flex_1())
                            .when_some(next_offset, |this, offset| {
                                this.child(
                                    Button::new("more-text")
                                        .xsmall()
                                        .label("继续加载")
                                        .disabled(count >= TEXT_LINE_LIMIT)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.load_text(Some(offset), cx)
                                        })),
                                )
                            }),
                    )
                    .child(
                        uniform_list(
                            "text-lines",
                            count,
                            move |range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| {
                                        div()
                                            .h(px(20.))
                                            .px_3()
                                            .whitespace_nowrap()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_xs()
                                            .child(lines[index].clone())
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(&self.text_scroll)
                        .flex_1()
                        .min_h_0(),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_hex(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hex = self.hex.read(cx);
        let range = format!(
            "{} – {}",
            format::hex_offset(hex.base()),
            format::hex_offset(hex.window_end())
        );
        let windowed = self
            .node
            .as_ref()
            .is_some_and(|node| node.size > WINDOW_BYTES);
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .w(px(180.))
                            .child(Input::new(&self.offset_input).small()),
                    )
                    .child(ui::mono(range, cx))
                    .when(windowed, |this| {
                        this.child("每次显示 64 MB，输入偏移后回车跳转")
                    }),
            )
            .child(div().flex_1().min_h_0().child(self.hex.clone()))
    }
}

impl Render for FileDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close_browser = self.browser.clone();
        let close = Button::new("close-file-preview")
            .ghost()
            .small()
            .icon(IconName::Close)
            .tooltip("关闭预览")
            .on_click(move |_, _, cx| {
                close_browser.update(cx, FileBrowser::close_preview).ok();
            });
        let Some(node) = self.node.clone() else {
            return v_flex()
                .size_full()
                .child(ui::detail_header("文件预览", close, cx))
                .child(ui::empty_state(
                    Icon::new(IconName::PanelRightOpen),
                    "选择文件，即刻预览",
                    Some("单击文件查看内容，双击目录进入。".into()),
                ))
                .into_any_element();
        };
        let tab = self.tab;
        let readable = node.kind != NodeKind::Dir;
        let export_node = node.clone();
        let browser = self.browser.clone();
        let body = match tab {
            DetailTab::Info => div()
                .size_full()
                .px_4()
                .pb_4()
                .child(self.render_info(&node, cx))
                .overflow_y_scrollbar()
                .into_any_element(),
            DetailTab::Text => self.render_text(cx).into_any_element(),
            DetailTab::Hex => self.render_hex(cx).into_any_element(),
            DetailTab::Native => match self.source_language {
                Some(language) => self.render_source(language, window, cx),
                None => {
                    #[cfg(all(windows, target_arch = "x86_64"))]
                    {
                        self.render_native(window, cx)
                    }
                    #[cfg(not(all(windows, target_arch = "x86_64")))]
                    {
                        ui::error_text("此文件没有可用的原类型预览", cx).into_any_element()
                    }
                }
            },
        };
        let tabs = TabBar::new("detail-tabs")
            .px_2()
            .underline()
            .child(Tab::new().label("详细信息"))
            .child(Tab::new().label("文本").disabled(!readable))
            .child(Tab::new().label("十六进制").disabled(!readable))
            .when(self.has_native_tab(), |tabs| {
                tabs.child(Tab::new().label("原类型视图").disabled(!readable))
            });
        v_flex()
            .size_full()
            .child(
                ui::pane_header(cx)
                    .child(div().flex_shrink_0().child(node_icon(&node, cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(if node.deleted {
                                cx.theme().danger
                            } else {
                                cx.theme().foreground
                            })
                            .child(node.name.clone()),
                    )
                    .when(node.deleted, |this| {
                        this.child(Tag::danger().xsmall().child("已删除"))
                    })
                    .child(
                        Button::new("export-node")
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .label("导出")
                            .on_click(move |_, window, cx| {
                                browser
                                    .update(cx, |browser, cx| {
                                        browser.export_node(export_node.clone(), window, cx)
                                    })
                                    .ok();
                            }),
                    )
                    .child(close),
            )
            .child(
                tabs.selected_index(match tab {
                    DetailTab::Info => 0,
                    DetailTab::Text => 1,
                    DetailTab::Hex => 2,
                    DetailTab::Native => 3,
                })
                .on_click(cx.listener(|this, index: &usize, _, cx| {
                    let tab = match index {
                        1 => DetailTab::Text,
                        2 => DetailTab::Hex,
                        3 => DetailTab::Native,
                        _ => DetailTab::Info,
                    };
                    this.select_tab(tab, cx);
                })),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
