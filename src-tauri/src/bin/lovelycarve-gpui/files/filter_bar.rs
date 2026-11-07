//! Scope, column visibility and batch actions for the file listing.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{DropdownMenu as _, PopupMenuItem},
        ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    Context, IntoElement, ParentElement as _, Styled as _,
};

use super::{FileBrowser, ListingScope, FILE_TAGS};
use crate::ui;

impl FileBrowser {
    /// Batch actions on the checked rows.
    fn render_checked_actions(&self, checked: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let checked_ids = |this: &Self, cx: &gpui_kit::App| -> Vec<String> {
            this.table
                .read(cx)
                .delegate()
                .checked_rows()
                .into_iter()
                .map(|node| node.id)
                .collect()
        };
        h_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("已勾选 {checked} 项")),
            )
            .child(
                Button::new("export-checked")
                    .small()
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Download))
                    .label("导出")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let nodes = this.table.read(cx).delegate().checked_rows();
                        this.export_nodes(nodes, window, cx);
                    })),
            )
            .child(
                Button::new("hash-checked")
                    .small()
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::FingerprintPattern))
                    .label("哈希")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let nodes = this.table.read(cx).delegate().checked_rows();
                        this.hash_nodes(nodes, window, cx);
                    })),
            )
            .children(FILE_TAGS.iter().enumerate().map(|(index, tag)| {
                Button::new(("tag-checked", index))
                    .xsmall()
                    .ghost()
                    .label(*tag)
                    .tooltip(format!("标记为「{tag}」"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ids = checked_ids(this, cx);
                        this.apply_tag(ids, Some(tag), window, cx);
                    }))
            }))
            .child(
                Button::new("untag-checked")
                    .xsmall()
                    .ghost()
                    .label("清除标签")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ids = checked_ids(this, cx);
                        this.apply_tag(ids, None, window, cx);
                    })),
            )
            .child(
                Button::new("uncheck-all")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("取消勾选")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.table.update(cx, |table, cx| {
                            table.delegate_mut().clear_checked();
                            cx.notify();
                        });
                        cx.notify();
                    })),
            )
    }

    pub(super) fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table.read(cx).delegate();
        let scope = table.scope();
        let checked = table.checked_count();
        let options = table.column_options();
        let table = self.table.clone();
        let query = self.filter.read(cx).value().trim().to_owned();
        ui::page_toolbar(cx)
            .child(
                h_flex()
                    .p_0p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .children(
                        [
                            ("directory", "当前目录", ListingScope::Directory),
                            ("descendants", "所有子目录", ListingScope::Descendants),
                        ]
                        .map(|(id, label, target)| {
                            Button::new(id)
                                .xsmall()
                                .ghost()
                                .selected(scope == target)
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if scope != target {
                                        this.set_scope(target, cx);
                                    }
                                }))
                        }),
                    ),
            )
            .when(!query.is_empty(), |this| {
                this.child(
                    Button::new("clear-name-filter")
                        .xsmall()
                        .outline()
                        .icon(IconName::Close)
                        .label(format!("名称包含「{query}」"))
                        .tooltip("清除名称筛选")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.filter
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            this.schedule_filter(String::new(), cx);
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("file-columns")
                    .small()
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Columns3))
                    .label("列设置")
                    .dropdown_menu(move |menu, _, _| {
                        options.iter().fold(menu, |menu, &(key, label, visible)| {
                            let table = table.clone();
                            menu.item(PopupMenuItem::new(label).checked(visible).on_click(
                                move |_, _, cx| {
                                    table.update(cx, |table, cx| {
                                        table.delegate_mut().toggle_column(key);
                                        table.refresh(cx);
                                        cx.notify();
                                    });
                                },
                            ))
                        })
                    }),
            )
            .when(checked > 0, |this| {
                this.child(self.render_checked_actions(checked, cx))
            })
    }
}
