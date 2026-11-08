//! The original-format preview tab and its viewer process lifecycle.
use futures::channel::oneshot;
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        v_flex, ActiveTheme as _, Sizable as _,
    },
    div, AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::{DetailTab, FileDetail};
use crate::{backend::Backend, files::native, prefs::Preferences, ui};

pub(super) enum NativePreview {
    Idle,
    Running {
        viewer: Option<native::ViewerWindow>,
        _cancel: DropGuard,
    },
    Failed(SharedString),
}

impl FileDetail {
    pub(in crate::files) fn close_native(&mut self, cx: &mut Context<Self>) {
        self.native = NativePreview::Idle;
        cx.notify();
    }

    fn open_native(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(node) = self.readable() else {
            return;
        };
        let directory = Preferences::global(cx).outside_in_directory.clone();
        let setup = native::runtime_directory(directory).and_then(|runtime| {
            native::ViewerWindow::from_gpui(window).map(|parent| (runtime, parent))
        });
        let (runtime, parent) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                self.native = NativePreview::Failed(error.into());
                cx.notify();
                return;
            }
        };
        let cancel = CancellationToken::new();
        let (ready, opened) = oneshot::channel();
        let finished = native::open(
            Backend::global(cx),
            parent,
            runtime,
            self.image_id.clone(),
            node.clone(),
            cancel.clone(),
            ready,
        );
        self.native = NativePreview::Running {
            viewer: None,
            _cancel: cancel.clone().drop_guard(),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            if let Some(handle) = opened.await.ok().filter(|_| !cancel.is_cancelled()) {
                this.update(cx, |this, cx| {
                    if let NativePreview::Running { viewer, .. } = &mut this.native {
                        *viewer = Some(handle);
                        cx.notify();
                    }
                })
                .ok();
            }
            let result = finished.await;
            if !cancel.is_cancelled() {
                this.update(cx, |this, cx| {
                    this.native = NativePreview::Failed(match result {
                        Ok(()) => "原类型视图已关闭".into(),
                        Err(error) => error.message,
                    });
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn choose_native_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = ui::prompt_directory("选择 Outside In x64 Viewer 目录", cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(directory) = selected.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                match native::validate_runtime(&directory) {
                    Ok(()) => {
                        Preferences::update(window, cx, |preferences| {
                            preferences.outside_in_directory = Some(directory);
                        });
                        if matches!(this.native, NativePreview::Failed(_)) {
                            this.close_native(cx);
                        }
                    }
                    Err(error) => ui::notify_error(window, cx, "无法设置 Viewer 目录", error),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_native(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.readable().is_none() {
            return div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("选择文件以查看原类型。")
                .into_any_element();
        }
        if matches!(self.native, NativePreview::Idle) {
            let generation = self.generation;
            cx.defer_in(window, move |this, window, cx| {
                if this.generation == generation
                    && this.tab == DetailTab::Native
                    && matches!(this.native, NativePreview::Idle)
                {
                    this.open_native(window, cx);
                }
            });
        }
        match &self.native {
            NativePreview::Idle | NativePreview::Running { viewer: None, .. } => h_flex()
                .p_4()
                .text_sm()
                .gap_2()
                .child(Spinner::new().small())
                .child("正在准备原类型视图…")
                .into_any_element(),
            NativePreview::Running {
                viewer: Some(viewer),
                ..
            } => native::viewport(*viewer).into_any_element(),
            NativePreview::Failed(error) => v_flex()
                .p_4()
                .gap_3()
                .text_sm()
                .child(ui::error_text(error.clone(), cx))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("choose-native-viewer")
                                .small()
                                .primary()
                                .label("选择 Viewer 目录")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.choose_native_directory(window, cx);
                                })),
                        )
                        .child(
                            Button::new("retry-native-viewer")
                                .small()
                                .ghost()
                                .label("重试")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_native(window, cx);
                                })),
                        ),
                )
                .into_any_element(),
        }
    }
}
