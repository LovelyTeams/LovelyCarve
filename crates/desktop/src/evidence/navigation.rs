//! Evidence navigation: a category rail and the current category's pages.
use gpui_kit::{
    component::{
        button::{Button, ButtonCustomVariant, ButtonVariants as _},
        h_flex,
        sidebar::{Sidebar, SidebarMenu, SidebarMenuItem, SidebarToggleButton},
        v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    },
    div, px, Context, IntoElement, ParentElement as _, Styled as _, Window,
};
use lovelycarve_backend::core::evidence::image::ImageInfo;

use super::{wire_name, EvidenceTab, ExtraIcon, Page};
use crate::{format, prefs::Preferences};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Image,
    Tools,
    Analysis,
    Bcp,
}

impl Group {
    const ALL: [Self; 4] = [Self::Image, Self::Tools, Self::Analysis, Self::Bcp];
    pub(super) const COUNT: usize = Self::ALL.len();

    fn label(self) -> &'static str {
        match self {
            Self::Image => "镜像",
            Self::Tools => "工具",
            Self::Analysis => "分析",
            Self::Bcp => "BCP 数据",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Image => Icon::new(IconName::HardDrive),
            Self::Tools => Icon::new(ExtraIcon::Wrench),
            Self::Analysis => Icon::new(ExtraIcon::ScanSearch),
            Self::Bcp => Icon::new(ExtraIcon::Database),
        }
    }

    fn first_page(self) -> Page {
        match self {
            Self::Image => Page::Dashboard,
            Self::Tools => Page::Search,
            Self::Analysis => Page::Analysis,
            Self::Bcp => Page::Bcp(None),
        }
    }
}

impl Page {
    pub(super) fn group(self) -> Group {
        match self {
            Self::Dashboard | Self::Files => Group::Image,
            Self::Search | Self::Timeline | Self::Carve => Group::Tools,
            Self::Analysis => Group::Analysis,
            Self::Bcp(_) => Group::Bcp,
        }
    }
}

impl EvidenceTab {
    fn navigation_pages(&self, group: Group, info: &ImageInfo) -> Vec<(Page, Option<u64>)> {
        match group {
            Group::Image => vec![(Page::Dashboard, None), (Page::Files, None)],
            Group::Tools => {
                let mut pages = vec![(Page::Search, None), (Page::Timeline, None)];
                if !info.index_only && wire_name(&info.source_format) != "folder" {
                    pages.push((Page::Carve, None));
                }
                pages
            }
            Group::Analysis => vec![(Page::Analysis, None)],
            Group::Bcp => std::iter::once((Page::Bcp(None), None))
                .chain(
                    self.bcp
                        .as_deref()
                        .into_iter()
                        .flat_map(crate::bcp::categories_with_rows)
                        .map(|(category, rows)| (Page::Bcp(Some(category)), Some(rows))),
                )
                .collect(),
        }
    }

    pub(super) fn render_navigation(
        &self,
        info: &ImageInfo,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = Preferences::global(cx).sidebar_collapsed;
        let active_group = self.page.group();
        let rail = v_flex()
            .w(px(64.))
            .h_full()
            .flex_shrink_0()
            .px_2()
            .pt_3()
            .pb_2()
            .gap_2()
            .children(
                Group::ALL
                    .into_iter()
                    .filter(|group| *group != Group::Bcp || self.bcp.is_some())
                    .map(|group| {
                        let active = group == active_group;
                        Button::new(("evidence-category", group as usize))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .foreground(if active {
                                        cx.theme().sidebar_accent_foreground
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .hover(if active {
                                        cx.theme().sidebar_accent
                                    } else {
                                        cx.theme().list_hover
                                    })
                                    .active(cx.theme().sidebar_accent),
                            )
                            .selected(active)
                            .w_full()
                            .h(px(56.))
                            .p_0()
                            .accessibility_label(group.label())
                            .child(
                                v_flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.))
                                    .child(group.icon().size(px(19.)))
                                    .child(group.label()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if Preferences::global(cx).sidebar_collapsed {
                                    Preferences::update(window, cx, |preferences| {
                                        preferences.sidebar_collapsed = false;
                                    });
                                }
                                let page = this.last_pages[group as usize]
                                    .unwrap_or_else(|| group.first_page());
                                this.show_page(page, window, cx);
                            }))
                    }),
            )
            .child(div().flex_1())
            .child(
                h_flex().justify_center().child(
                    SidebarToggleButton::new()
                        .collapsed(collapsed)
                        .accessibility_label(if collapsed {
                            "展开侧栏"
                        } else {
                            "收起侧栏"
                        })
                        .on_click(|_, window, cx| {
                            Preferences::update(window, cx, |preferences| {
                                preferences.sidebar_collapsed = !preferences.sidebar_collapsed;
                            });
                        }),
                ),
            );
        let mut navigation = h_flex().h_full().flex_shrink_0().child(rail);
        if !collapsed {
            let pages = self.navigation_pages(active_group, info);
            let width = if window.viewport_size().width <= px(1000.) {
                px(188.)
            } else {
                px(224.)
            };
            if active_group == Group::Analysis {
                if let Some(directory) = &self.pages.analysis_directory {
                    return navigation.child(
                        div()
                            .w(width)
                            .h_full()
                            .flex_shrink_0()
                            .child(directory.clone()),
                    );
                }
            }
            navigation = navigation.child(
                Sidebar::new(("evidence-pages", active_group as usize))
                    .w(width)
                    .border_r_0()
                    .child(
                        SidebarMenu::new().children(pages.into_iter().map(|(page, count)| {
                            let mut item = SidebarMenuItem::new(page.label())
                                .icon(page.icon().small())
                                .text_color(cx.theme().foreground)
                                .active(page == self.page)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.show_page(page, window, cx);
                                }));
                            if let Some(count) = count {
                                item = item.suffix(move |_, cx| {
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format::count(count))
                                });
                            }
                            item
                        })),
                    ),
            );
        }
        navigation
    }
}
