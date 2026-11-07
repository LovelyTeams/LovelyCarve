//! Small presentation pieces the pages share: page toolbars and footers,
//! empty states and label/value rows. They compose GPUI Kit components and
//! read every color from the active theme.
use gpui_kit::{
    component::{
        empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
        h_flex, ActiveTheme as _, Icon,
    },
    div, App, IntoElement, ParentElement, SharedString, Styled,
};

/// The bar a workspace page opens on: filters on the left, actions on the
/// right.
pub fn page_toolbar(cx: &App) -> gpui_kit::Div {
    h_flex()
        .flex_shrink_0()
        .w_full()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
}

/// The bar a workspace page ends on: context on the left, counts on the
/// right.
pub fn page_footer(left: impl IntoElement, right: impl IntoElement, cx: &App) -> gpui_kit::Div {
    h_flex()
        .flex_shrink_0()
        .w_full()
        .h_8()
        .px_3()
        .gap_3()
        .justify_between()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .child(h_flex().min_w_0().gap_2().child(left))
        .child(h_flex().flex_shrink_0().gap_2().child(right))
}

/// A centered empty state with an icon, a title and an optional description.
pub fn empty_state(
    icon: impl Into<Icon>,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
) -> Empty {
    let mut header = EmptyHeader::new()
        .media(EmptyMedia::new().child(icon.into()))
        .title(EmptyTitle::new().child(title.into()));
    if let Some(description) = description {
        header = header.description(EmptyDescription::new().child(description));
    }
    Empty::new().border_0().header(header)
}

/// A muted label beside its value, for detail panes.
pub fn field_row(
    label: impl Into<SharedString>,
    value: impl IntoElement,
    cx: &App,
) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .py_1()
        .child(
            div()
                .w_24()
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().text_sm().child(value))
}

/// A section heading inside a detail pane.
pub fn section_title(title: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .w_full()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title.into())
}

/// Monospace text for offsets, hashes, sectors and identifiers.
pub fn mono(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .child(text.into())
}

/// An inline error line under a control or at the top of a pane.
pub fn error_text(message: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(message.into())
}

/// Ask the platform for paths; an empty list when the user cancels.
pub fn prompt_paths(
    options: gpui_kit::PathPromptOptions,
    cx: &App,
) -> impl std::future::Future<Output = Vec<std::path::PathBuf>> + use<> {
    let receiver = cx.prompt_for_paths(options);
    async move {
        match receiver.await {
            Ok(Ok(Some(paths))) => paths,
            Ok(Ok(None)) | Err(_) => Vec::new(),
            Ok(Err(error)) => {
                tracing::warn!("path prompt: {error}");
                Vec::new()
            }
        }
    }
}

/// Ask the platform for a single directory.
pub fn prompt_directory(
    prompt: &str,
    cx: &App,
) -> impl std::future::Future<Output = Option<std::path::PathBuf>> + use<> {
    let paths = prompt_paths(
        gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.to_owned().into()),
        },
        cx,
    );
    async move { paths.await.into_iter().next() }
}

/// Ask where to save `name`, starting in Downloads; `None` when cancelled.
pub fn prompt_save(
    name: &str,
    cx: &App,
) -> impl std::future::Future<Output = Option<std::path::PathBuf>> + use<> {
    let directory = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let receiver = cx.prompt_for_new_path(&directory, Some(name));
    async move {
        match receiver.await {
            Ok(Ok(path)) => path,
            Ok(Err(error)) => {
                tracing::warn!("save prompt: {error}");
                None
            }
            Err(_) => None,
        }
    }
}

/// Ask for confirmation before an irreversible action; `on_confirm` runs
/// when the user accepts.
pub fn confirm(
    window: &mut gpui_kit::Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    ok_text: impl Into<SharedString>,
    on_confirm: impl Fn(&mut gpui_kit::Window, &mut App) + 'static,
) {
    use gpui_kit::component::{button::ButtonVariant, WindowExt as _};
    let title = title.into();
    let description = description.into();
    let ok_text = ok_text.into();
    let on_confirm = std::rc::Rc::new(on_confirm);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        alert
            .ok_variant(ButtonVariant::Danger)
            .title(title.clone())
            .description(description.clone())
            .ok_text(ok_text.clone())
            .cancel_text("取消")
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}

/// Show a failed operation as a notification.
pub fn notify_error(
    window: &mut gpui_kit::Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
) {
    use gpui_kit::component::{notification::Notification, WindowExt as _};
    window.push_notification(Notification::error(message.into()).title(title.into()), cx);
}

/// Show an operation that ended without a result, such as a cancellation.
pub fn notify_info(
    window: &mut gpui_kit::Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
) {
    use gpui_kit::component::{notification::Notification, WindowExt as _};
    window.push_notification(Notification::info(message.into()).title(title.into()), cx);
}

/// Show a completed operation whose result needs attention.
pub fn notify_warning(
    window: &mut gpui_kit::Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
) {
    use gpui_kit::component::{notification::Notification, WindowExt as _};
    window.push_notification(
        Notification::warning(message.into()).title(title.into()),
        cx,
    );
}

/// Show a completed operation as a notification.
pub fn notify_success(
    window: &mut gpui_kit::Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
) {
    use gpui_kit::component::{notification::Notification, WindowExt as _};
    window.push_notification(
        Notification::success(message.into()).title(title.into()),
        cx,
    );
}
