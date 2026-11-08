//! Small presentation pieces the pages share: page toolbars and footers,
//! empty states and label/value rows. They compose GPUI Kit components and
//! read every color from the active theme.
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
        h_flex, h_resizable, resizable_panel,
        table::{DataTable, TableDelegate, TableState},
        tag::Tag,
        v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, Size,
    },
    div, px, AnimationExt as _, AnyElement, App, Div, ElementId, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, SharedString, Styled, Window,
};

pub const TOOLBAR_HEIGHT: f32 = 48.;
pub const TABLE_HEADER_HEIGHT: f32 = 34.;
pub const TABLE_ROW_HEIGHT: f32 = 36.;

pub fn data_table<T: TableDelegate + 'static>(table: &Entity<TableState<T>>) -> DataTable<T> {
    DataTable::new(table)
        .bordered(false)
        .header_row_height(px(TABLE_HEADER_HEIGHT))
        .with_size(Size::Size(px(TABLE_ROW_HEIGHT)))
}

pub fn wordmark(size: f32, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .font_family("Quicksand")
        .font_weight(FontWeight::BOLD)
        .text_size(px(size))
        .child("LovelyCarve")
        .child(div().text_color(cx.theme().primary).child("."))
}

pub fn workspace_sheet(cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .rounded(px(crate::theme::PANEL_RADIUS))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
}

pub fn card(cx: &App) -> Div {
    v_flex()
        .min_w_0()
        .rounded(px(crate::theme::PANEL_RADIUS))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
}

pub fn card_heading(icon: Icon, label: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon.small().text_color(cx.theme().accent_foreground))
        .child(label)
}

pub fn dialog_heading(
    icon: impl Into<Icon>,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    cx: &App,
) -> Div {
    h_flex()
        .min_w_0()
        .gap_3()
        .child(
            h_flex()
                .size(px(36.))
                .flex_shrink_0()
                .justify_center()
                .rounded_full()
                .bg(cx.theme().accent)
                .child(
                    icon.into()
                        .size(px(18.))
                        .text_color(cx.theme().accent_foreground),
                ),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle.into()),
                ),
        )
}

pub fn pane_header(cx: &App) -> Div {
    h_flex()
        .h(px(TABLE_HEADER_HEIGHT))
        .flex_shrink_0()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().table_head)
        .text_size(px(12.))
}

/// The bar a workspace page opens on: filters on the left, actions on the
/// right.
pub fn page_toolbar(cx: &App) -> gpui_kit::Div {
    h_flex()
        .flex_shrink_0()
        .w_full()
        .min_h(px(TOOLBAR_HEIGHT))
        .flex_wrap()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
}

/// Results pages share a full-height history rail and a separate working pane.
pub fn result_workspace(
    history: Option<AnyElement>,
    content: impl IntoElement,
    window: &Window,
    cx: &App,
) -> Div {
    h_flex()
        .size_full()
        .items_stretch()
        .bg(cx.theme().popover)
        .when_some(history, |layout, history| {
            let rail = div()
                .w(px(if window.viewport_size().width < px(1120.) {
                    210.
                } else {
                    256.
                }))
                .flex_shrink_0()
                .h_full()
                .border_r_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().table_foot)
                .child(history);
            layout.child(crate::motion::slide(rail, "history-rail", 16., cx))
        })
        .child(v_flex().flex_1().min_w_0().min_h_0().child(content))
}

pub fn history_header(label: &'static str, count: usize, close: Button, cx: &App) -> Div {
    pane_header(cx)
        .h(px(TOOLBAR_HEIGHT))
        .bg(cx.theme().table_foot)
        .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
        .child(
            Tag::secondary()
                .small()
                .child(crate::format::count(count as u64)),
        )
        .child(div().flex_1())
        .child(close)
}

pub fn history_toggle(
    id: &'static str,
    label: &'static str,
    visible: bool,
    show_label: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .h(px(30.))
        .selected(visible)
        .icon(IconName::PanelLeft)
        .tooltip(label)
        .accessibility_label(label)
        .when(show_label, |button| button.label(label))
}

/// Keep an open inspector above the table when it would crowd a narrow pane.
pub fn detail_frame(
    id: impl Into<ElementId>,
    content: impl IntoElement,
    detail: Option<AnyElement>,
    overlay: bool,
    width: Pixels,
    cx: &App,
) -> Div {
    let frame = div().flex_1().min_h_0().min_w_0().relative();
    let Some(detail) = detail else {
        return frame.child(content);
    };
    let id = id.into();
    let dock = |detail: AnyElement| {
        v_flex()
            .size_full()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .child(detail)
    };
    if overlay {
        // The floating inspector slides in over the table like a drawer from
        // the right edge, opaque all the way so the rows do not show through.
        frame.child(content).child(gpui_kit::deferred(
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(width)
                .max_w_full()
                .occlude()
                .shadow_lg()
                .child(dock(detail))
                .with_animation(
                    (id, "enter"),
                    crate::motion::entrance(Duration::ZERO, cx),
                    move |panel, progress| panel.right(-width * (1. - progress)),
                ),
        ))
    } else {
        // A docked inspector is opaque at once, since the table may run on
        // beneath it; only its contents slide in.
        let contents = crate::motion::slide(
            v_flex().size_full().child(detail),
            (id.clone(), "enter"),
            -16.,
            cx,
        );
        frame.child(
            h_resizable(id)
                .child(resizable_panel().child(content))
                .child(
                    resizable_panel()
                        .size(width)
                        .size_range(px(300.)..px(720.))
                        .child(dock(contents.into_any_element())),
                ),
        )
    }
}

pub fn detail_header(title: impl Into<SharedString>, close: Button, cx: &App) -> Div {
    pane_header(cx)
        .h(px(TOOLBAR_HEIGHT))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(close)
}

pub fn disclosure_button(
    id: &'static str,
    title: &'static str,
    summary: impl Into<SharedString>,
    open: bool,
    cx: &App,
) -> Button {
    Button::new(id)
        .ghost()
        .w_full()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().flex_1())
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(summary.into()),
        )
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .small(),
        )
}

pub fn workspace_start(toolbar: impl IntoElement, state: Empty, cx: &App) -> Div {
    v_flex()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(cx.theme().popover)
        .child(h_flex().px_3().pt_3().min_h(px(42.)).child(toolbar))
        .child(state.pb(px(78.)))
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
        .bg(cx.theme().table_foot)
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
        .media(
            EmptyMedia::new()
                .with_variant(EmptyMediaVariant::Icon)
                .size(px(48.))
                .rounded(px(crate::theme::PANEL_RADIUS))
                .child(icon.into().size(px(22.))),
        )
        .title(
            EmptyTitle::new()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        );
    if let Some(description) = description {
        header = header.description(
            EmptyDescription::new()
                .text_size(px(12.))
                .child(description),
        );
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
            .centered()
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
