//! A virtualized hex view over one file: rows of 16 bytes, read from the
//! evidence in 64 KiB chunks as they scroll into view. A window of at most
//! [`WINDOW_BYTES`] is shown at a time; jumping to an offset moves it.
use std::collections::HashMap;

use gpui_kit::{
    component::{h_flex, v_flex, ActiveTheme as _},
    div,
    prelude::FluentBuilder as _,
    px, uniform_list, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    UniformListScrollHandle, Window,
};
use lovelycarve_lib::{commands::preview::filesystem_preview as preview_commands, host::State};

use crate::{backend::Backend, ui};

const ROW_BYTES: u64 = 16;
const CHUNK_BYTES: u64 = 64 * 1024;
/// The largest span listed at once; keeps the scroll height precise.
pub const WINDOW_BYTES: u64 = 64 * 1024 * 1024;
/// Chunks kept in memory before the farthest ones are dropped.
const CHUNK_CACHE: usize = 64;

enum Chunk {
    Loading,
    Loaded(Vec<u8>),
    Failed(SharedString),
}

pub struct HexView {
    image_id: String,
    node_id: Option<String>,
    size: u64,
    /// First byte of the listed window, a multiple of [`ROW_BYTES`].
    base: u64,
    chunks: HashMap<u64, Chunk>,
    generation: u64,
    scroll: UniformListScrollHandle,
}

impl HexView {
    pub fn new(image_id: String) -> Self {
        Self {
            image_id,
            node_id: None,
            size: 0,
            base: 0,
            chunks: HashMap::new(),
            generation: 0,
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Show `node_id` (of `size` bytes) from its first byte.
    pub fn show(&mut self, node_id: Option<String>, size: u64, cx: &mut Context<Self>) {
        self.node_id = node_id;
        self.size = size;
        self.base = 0;
        self.chunks.clear();
        self.generation = self.generation.wrapping_add(1);
        self.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        cx.notify();
    }

    /// Move the listed window so it starts at `offset` (rounded down to a row).
    pub fn jump(&mut self, offset: u64, cx: &mut Context<Self>) {
        let offset = offset.min(self.size.saturating_sub(1));
        self.base = offset - offset % ROW_BYTES;
        self.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        cx.notify();
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn window_end(&self) -> u64 {
        self.size.min(self.base.saturating_add(WINDOW_BYTES))
    }

    fn rows(&self) -> usize {
        self.window_end()
            .saturating_sub(self.base)
            .div_ceil(ROW_BYTES) as usize
    }

    fn request(&mut self, chunk: u64, cx: &mut Context<Self>) {
        if self.chunks.contains_key(&chunk) {
            return;
        }
        let Some(node_id) = self.node_id.clone() else {
            return;
        };
        if self.chunks.len() >= CHUNK_CACHE {
            let mut keys: Vec<u64> = self.chunks.keys().copied().collect();
            keys.sort_by_key(|key| std::cmp::Reverse(key.abs_diff(chunk)));
            for key in keys.into_iter().take(CHUNK_CACHE / 4) {
                if matches!(
                    self.chunks.get(&key),
                    Some(Chunk::Loaded(_) | Chunk::Failed(_))
                ) {
                    self.chunks.remove(&key);
                }
            }
        }
        self.chunks.insert(chunk, Chunk::Loading);
        let offset = chunk * CHUNK_BYTES;
        let length = CHUNK_BYTES.min(self.size.saturating_sub(offset));
        let image_id = self.image_id.clone();
        let generation = self.generation;
        let read = Backend::global(cx).run(move |_, state| async move {
            preview_commands::fs_read_range(State::new(&state), image_id, node_id, offset, length)
                .await
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.chunks.insert(
                    chunk,
                    match read {
                        Ok(response) => Chunk::Loaded(response.into_bytes()),
                        Err(error) => Chunk::Failed(error.message),
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn byte(&self, offset: u64) -> Option<Option<u8>> {
        match self.chunks.get(&(offset / CHUNK_BYTES))? {
            Chunk::Loaded(bytes) => Some(bytes.get((offset % CHUNK_BYTES) as usize).copied()),
            Chunk::Loading | Chunk::Failed(_) => None,
        }
    }

    fn render_row(&self, offset: u64, cx: &Context<Self>) -> impl IntoElement + use<> {
        let end = (offset + ROW_BYTES).min(self.size);
        let mut hex = String::with_capacity(52);
        let mut ascii = String::with_capacity(16);
        let mut pending = false;
        for position in offset..offset + ROW_BYTES {
            if position == offset + 8 {
                hex.push(' ');
            }
            if position >= end {
                hex.push_str("   ");
                continue;
            }
            match self.byte(position) {
                Some(Some(byte)) => {
                    hex.push_str(&format!("{byte:02X} "));
                    ascii.push(if byte.is_ascii_graphic() || byte == b' ' {
                        byte as char
                    } else {
                        '.'
                    });
                }
                _ => {
                    pending = true;
                    hex.push_str("·· ");
                    ascii.push(' ');
                }
            }
        }
        let theme = cx.theme();
        let (label, width) = if self.size <= u64::from(u32::MAX) {
            (format!("{offset:08X}"), px(64.))
        } else {
            (format!("{offset:012X}"), px(96.))
        };
        h_flex()
            .h(px(20.))
            .gap_3()
            .px_3()
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .child(
                div()
                    .w(width)
                    .flex_shrink_0()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .text_color(if pending {
                        theme.muted_foreground
                    } else {
                        theme.foreground
                    })
                    .child(hex),
            )
            .child(div().text_color(theme.muted_foreground).child(ascii))
    }
}

impl Render for HexView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.node_id.is_none() || self.size == 0 {
            return v_flex()
                .size_full()
                .child(ui::empty_state(
                    gpui_kit::component::IconName::File,
                    "没有可显示的字节",
                    Some("选择一个非空文件以查看十六进制内容。".into()),
                ))
                .into_any_element();
        }
        let failed = self.chunks.values().find_map(|chunk| match chunk {
            Chunk::Failed(error) => Some(error.clone()),
            _ => None,
        });
        let rows = self.rows();
        let base = self.base;
        v_flex()
            .size_full()
            .when_some(failed, |this, error| {
                this.child(div().px_3().py_1().child(ui::error_text(error, cx)))
            })
            .child(
                uniform_list(
                    "hex-rows",
                    rows,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let first = base + range.start as u64 * ROW_BYTES;
                        let last = base + (range.end as u64) * ROW_BYTES;
                        let mut chunk = first / CHUNK_BYTES;
                        while chunk * CHUNK_BYTES < last.min(this.size) {
                            this.request(chunk, cx);
                            chunk += 1;
                        }
                        range
                            .map(|row| {
                                this.render_row(base + row as u64 * ROW_BYTES, cx)
                                    .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}
