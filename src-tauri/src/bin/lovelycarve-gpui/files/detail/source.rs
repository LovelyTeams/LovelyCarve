//! Read-only source previews backed by the GPUI Kit editor.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Editor, EditorState},
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Sizable as _,
    },
    div, relative, AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window,
};
use lovelycarve_lib::{commands::preview::filesystem_preview, host::State};

use super::{DetailTab, FileDetail, SourceLanguage};
use crate::{backend::Backend, format, ui};

const SOURCE_PREVIEW_BYTES: usize = 1024 * 1024;

pub(super) enum SourcePreview {
    Idle,
    Loading,
    Ready {
        editor: Entity<EditorState>,
        encoding: String,
        had_errors: bool,
        next_offset: Option<u64>,
    },
    Failed(SharedString),
}

impl FileDetail {
    fn load_source(
        &mut self,
        language: SourceLanguage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.readable() else {
            return;
        };
        let node_id = node.id.clone();
        let image_id = self.image_id.clone();
        let generation = self.generation;
        self.source = SourcePreview::Loading;
        cx.notify();

        let page = Backend::global(cx).run(move |_, state| async move {
            filesystem_preview::fs_preview_text(
                State::new(&state),
                image_id,
                node_id,
                SOURCE_PREVIEW_BYTES,
                None,
                None,
            )
            .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = page.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.source = match result {
                    Ok(page) => {
                        let editor = cx.new(|cx| {
                            EditorState::new(window, cx)
                                .language(language.grammar)
                                .line_number(true)
                                .soft_wrap(false)
                                .indent_guides(true)
                                .default_value(page.text)
                        });
                        SourcePreview::Ready {
                            editor,
                            encoding: page.encoding,
                            had_errors: page.had_errors,
                            next_offset: page.next_offset,
                        }
                    }
                    Err(error) => SourcePreview::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_source(
        &self,
        language: SourceLanguage,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if matches!(self.source, SourcePreview::Idle) {
            let generation = self.generation;
            cx.defer_in(window, move |this, window, cx| {
                if this.generation == generation
                    && this.tab == DetailTab::Native
                    && matches!(this.source, SourcePreview::Idle)
                {
                    this.load_source(language, window, cx);
                }
            });
        }
        match &self.source {
            SourcePreview::Idle | SourcePreview::Loading => h_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child("正在加载源码…")
                .into_any_element(),
            SourcePreview::Failed(error) => v_flex()
                .p_4()
                .gap_3()
                .items_start()
                .child(ui::error_text(error.clone(), cx))
                .child(
                    Button::new("retry-source-preview")
                        .small()
                        .ghost()
                        .label("重试")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.load_source(language, window, cx);
                        })),
                )
                .into_any_element(),
            SourcePreview::Ready {
                editor,
                encoding,
                had_errors,
                next_offset,
            } => v_flex()
                .size_full()
                .bg(cx.theme().background)
                .child(
                    div().flex_1().min_h_0().overflow_hidden().child(
                        Editor::new(editor)
                            .h(relative(1.))
                            .appearance(false)
                            .bordered(false)
                            .readonly(true)
                            .aria_label("只读源码预览"),
                    ),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(language.label)
                        .child(encoding.clone())
                        .child("只读")
                        .children(
                            next_offset.map(|offset| format!("仅显示前 {}", format::bytes(offset))),
                        )
                        .children(
                            had_errors.then(|| Tag::warning().xsmall().child("含无法解码的字节")),
                        ),
                )
                .into_any_element(),
        }
    }
}
