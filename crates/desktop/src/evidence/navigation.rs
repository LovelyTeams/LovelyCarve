//! Evidence navigation: a category rail and the current category's pages.
//! The rail's highlight springs to the active category, and the page list
//! slides open and shut with the sidebar toggle.
use gpui_kit::{
    base::motion::{self, Transition},
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

/// The height of a category button on the rail.
const RAIL_BUTTON_HEIGHT: f32 = 56.;

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let collapsed = Preferences::global(cx).sidebar_collapsed;
        let active_group = self.page.group();
        let groups: Vec<Group> = Group::ALL
            .into_iter()
            .filter(|group| *group != Group::Bcp || self.bcp.is_some())
            .collect();
        // The rail's top padding (`pt_3`) and gap (`gap_2`) follow the rem
        // size; the highlight lands on the active button's slot.
        let rem = window.rem_size();
        let slot = groups
            .iter()
            .position(|group| *group == active_group)
            .unwrap_or_default();
        let spring = cx.theme().motion_tokens().spring_move;
        let highlight_top = motion::spring(
            "evidence-rail-highlight",
            rem * 0.75 + (px(RAIL_BUTTON_HEIGHT) + rem * 0.5) * slot as f32,
            spring,
            window,
            cx,
        );
        let rail = v_flex()
            .relative()
            .w(px(64.))
            .h_full()
            .flex_shrink_0()
            .px_2()
            .pt_3()
            .pb_2()
            .gap_2()
            .child(
                div()
                    .absolute()
                    .left_2()
                    .right_2()
                    .top(highlight_top)
                    .h(px(RAIL_BUTTON_HEIGHT))
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar_accent),
            )
            .children(groups.into_iter().map(|group| {
                let active = group == active_group;
                Button::new(("evidence-category", group as usize))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .foreground(if active {
                                cx.theme().sidebar_accent_foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .hover(cx.theme().list_hover)
                            // The sliding highlight paints the active
                            // category, so its own fill stays clear.
                            .active(if active {
                                cx.theme().transparent
                            } else {
                                cx.theme().sidebar_accent
                            }),
                    )
                    .selected(active)
                    .w_full()
                    .h(px(RAIL_BUTTON_HEIGHT))
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
                        let page =
                            this.last_pages[group as usize].unwrap_or_else(|| group.first_page());
                        this.show_page(page, window, cx);
                    }))
            }))
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
        let width = if window.viewport_size().width <= px(1000.) {
            px(188.)
        } else {
            px(224.)
        };
        // The page list keeps its full width inside a clip that opens and
        // shuts, so its rows do not reflow while it slides.
        let tokens = cx.theme().motion_tokens();
        let slide = Transition::new(tokens.duration_normal).easing(tokens.easing_move.clone());
        let shown = motion::transition(
            "evidence-pages-width",
            if collapsed { px(0.) } else { width },
            slide,
            window,
            cx,
        );
        let navigation = h_flex().h_full().flex_shrink_0().child(rail);
        if shown <= px(0.) {
            return navigation;
        }
        let pages =
            match &self.pages.analysis_directory {
                Some(directory) if active_group == Group::Analysis => div()
                    .size_full()
                    .child(directory.clone())
                    .into_any_element(),
                _ => Sidebar::new(("evidence-pages", active_group as usize))
                    .w(width)
                    .border_r_0()
                    .child(SidebarMenu::new().children(
                        self.navigation_pages(active_group, info).into_iter().map(
                            |(page, count)| {
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
                            },
                        ),
                    ))
                    .into_any_element(),
            };
        navigation.child(
            div()
                .w(shown)
                .h_full()
                .flex_shrink_0()
                .overflow_hidden()
                .child(div().w(width).h_full().child(pages)),
        )
    }
}
