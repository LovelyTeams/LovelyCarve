//! BCP 数据: GA BCP exchange exports in folder evidence. The sidebar lists
//! the categories that hold records; the page lists a category's datasets,
//! a dataset's records one page at a time and a record's fields.
mod table;

use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        clipboard::Clipboard,
        h_flex,
        input::{Input, InputEvent, InputState},
        scroll::ScrollableElement as _,
        table::{TableEvent, TableState},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use lovelycarve_lib::{
    commands::analysis::bcp as bcp_commands,
    core::analysis::bcp::{BcpCatalog, BcpCursor, BcpDataset},
    host::State,
};

use crate::{backend::Backend, format, ui};
use table::{field_title, BcpTable, Record};

type ExtraIcon = gpui_kit::assets::IconName;

/// The categories, by the first two digits of a dataset code.
const CATEGORIES: [(&str, &str); 9] = [
    ("01", "设备与通信"),
    ("02", "即时通信"),
    ("03", "社交动态"),
    ("05", "浏览器记录"),
    ("07", "交易与支付"),
    ("08", "网络位置"),
    ("09", "应用与媒体"),
    ("10", "文件清单"),
    ("16", "检材信息"),
];

/// Datasets whose code fits no category.
pub const OTHER: &str = "other";

/// The dataset code's six-digit suffix, or the code itself.
fn suffix(code: &str) -> &str {
    let digits = code.chars().rev().take_while(char::is_ascii_digit).count();
    if digits >= 6 {
        &code[code.len() - 6..]
    } else {
        code
    }
}

/// The category a dataset belongs to.
pub fn category_of(code: &str) -> &'static str {
    let suffix = suffix(code);
    let prefix = suffix.get(..2).unwrap_or_default();
    CATEGORIES
        .iter()
        .find(|(category, _)| *category == prefix)
        .map_or(OTHER, |(category, _)| category)
}

/// A category's sidebar name; `None` is every dataset.
pub fn category_label(category: Option<&str>) -> &'static str {
    match category {
        None => "全部数据",
        Some(OTHER) => "其他数据",
        Some(category) => CATEGORIES
            .iter()
            .find(|(prefix, _)| *prefix == category)
            .map_or("BCP 数据", |(_, label)| label),
    }
}

/// A category's sidebar icon.
pub fn category_icon(category: Option<&str>) -> Icon {
    Icon::new(match category {
        Some("01") => ExtraIcon::Phone,
        Some("02") => ExtraIcon::Send,
        Some("03") => ExtraIcon::Share2,
        Some("05") => ExtraIcon::Globe,
        Some("08") => ExtraIcon::Wifi,
        Some("09") => ExtraIcon::Package,
        Some("10") => ExtraIcon::Files,
        Some("16") => ExtraIcon::Info,
        _ => ExtraIcon::Database,
    })
}

/// The categories that hold records, in sidebar order, with their counts.
pub fn categories_with_rows(catalog: &BcpCatalog) -> Vec<(&'static str, u64)> {
    CATEGORIES
        .iter()
        .map(|(prefix, _)| *prefix)
        .chain(std::iter::once(OTHER))
        .filter_map(|category| {
            let rows: u64 = catalog
                .datasets
                .iter()
                .filter(|dataset| category_of(&dataset.code) == category)
                .map(|dataset| dataset.rows)
                .sum();
            (rows > 0).then_some((category, rows))
        })
        .collect()
}

/// A dataset's name: the known name of its code, or `<suffix> 数据`.
fn dataset_label(dataset: &BcpDataset) -> String {
    let name = match suffix(&dataset.code) {
        "010100" => "采集信息",
        "010200" => "设备信息",
        "010201" => "设备标识",
        "010202" => "操作系统",
        "010300" => "SIM 卡",
        "010400" => "联系人",
        "010500" => "联系人字段",
        "010600" => "通话记录",
        "010700" => "短信",
        "010800" => "彩信",
        "010900" => "日程",
        "011000" => "同步账号",
        "020100" => "即时通信账号",
        "020200" => "好友",
        "020300" => "群组",
        "020400" => "群成员",
        "020500" => "私聊消息",
        "020600" => "群聊消息",
        "020700" => "动态消息",
        "020800" | "030500" => "搜索记录",
        "030100" => "社交账号",
        "030200" => "关注与好友",
        "030300" => "发布内容",
        "030400" => "私信",
        "050200" => "书签",
        "050300" => "访问历史",
        "050400" => "Cookie",
        "050500" => "网站账号",
        "050600" => "浏览器搜索",
        "070100" => "支付账号",
        "070400" => "交易记录",
        "070500" => "支付记录",
        "070800" => "红包记录",
        "080100" => "Wi-Fi 记录",
        "090100" => "应用账号",
        "090200" => "媒体关联",
        "090300" => "已装应用",
        "090400" => "媒体文件",
        "090600" => "文件哈希",
        "109999" => "关联文件",
        "160100" => "检材详情",
        other => return format!("{other} 数据"),
    };
    name.to_owned()
}

pub struct BcpPanel {
    image_id: String,
    catalog: Rc<BcpCatalog>,
    category: Option<&'static str>,
    /// The selected dataset's code.
    selected: Option<String>,
    /// The cursors of the pages after the first, in the order they were
    /// opened; the current page is the last one.
    history: Vec<BcpCursor>,
    next: Option<BcpCursor>,
    table: Entity<TableState<BcpTable>>,
    filter: Entity<InputState>,
    record: Option<Record>,
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl BcpPanel {
    pub fn new(
        image_id: String,
        catalog: Rc<BcpCatalog>,
        category: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            TableState::new(BcpTable::new(), window, cx)
                .col_movable(false)
                .sortable(false)
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("筛选当前页…"));
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(row) = event {
                    this.record = table.read(cx).delegate().record(*row).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &filter,
                window,
                |this, filter, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        let text = filter.read(cx).value().to_string();
                        this.record = None;
                        this.table.update(cx, |table, cx| {
                            table.clear_selection(cx);
                            table.delegate_mut().set_filter(&text);
                            table.refresh(cx);
                        });
                        cx.notify();
                    }
                },
            ),
        ];
        let mut panel = Self {
            image_id,
            catalog,
            category,
            selected: None,
            history: Vec::new(),
            next: None,
            table,
            filter,
            record: None,
            generation: 0,
            _subscriptions: subscriptions,
        };
        panel.set_category(category, window, cx);
        panel
    }

    fn datasets(&self) -> Vec<&BcpDataset> {
        self.catalog
            .datasets
            .iter()
            .filter(|dataset| {
                self.category
                    .is_none_or(|category| category_of(&dataset.code) == category)
            })
            .collect()
    }

    fn dataset(&self) -> Option<&BcpDataset> {
        let selected = self.selected.as_deref()?;
        self.catalog
            .datasets
            .iter()
            .find(|dataset| dataset.code == selected)
    }

    /// Show a category, starting at its first dataset.
    pub fn set_category(
        &mut self,
        category: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.category == category && self.selected.is_some() {
            return;
        }
        self.category = category;
        let first = self.datasets().first().map(|dataset| dataset.code.clone());
        match first {
            Some(code) => self.select(code, window, cx),
            None => {
                self.generation += 1;
                self.selected = None;
                self.record = None;
                cx.notify();
            }
        }
    }

    fn select(&mut self, code: String, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self
            .catalog
            .datasets
            .iter()
            .find(|dataset| dataset.code == code)
            .map(|dataset| dataset.fields.clone())
            .unwrap_or_default();
        self.selected = Some(code);
        self.history.clear();
        self.next = None;
        self.record = None;
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            let delegate = table.delegate_mut();
            delegate.set_fields(fields);
            delegate.set_filter("");
            table.refresh(cx);
        });
        self.load(None, cx);
    }

    fn load(&mut self, cursor: Option<BcpCursor>, cx: &mut Context<Self>) {
        let Some(code) = self.selected.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.record = None;
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().set_loading();
            table.refresh(cx);
        });
        let image_id = self.image_id.clone();
        let loading = Backend::global(cx).run(move |_, state| async move {
            bcp_commands::bcp_rows(State::new(&state), image_id, code, cursor).await
        });
        cx.spawn(async move |this, cx| {
            let page = loading.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let records = match page {
                    Ok(page) => {
                        this.next = page.next_cursor;
                        Ok(page.rows.into_iter().map(Record::from).collect())
                    }
                    Err(error) => {
                        this.next = None;
                        Err(error.message)
                    }
                };
                this.table.update(cx, |table, cx| {
                    table.delegate_mut().set_page(records);
                    table.refresh(cx);
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if let Some(cursor) = self.next {
            self.history.push(cursor);
            self.load(Some(cursor), cx);
        }
    }

    fn previous_page(&mut self, cx: &mut Context<Self>) {
        if self.history.pop().is_some() {
            let cursor = self.history.last().copied();
            self.load(cursor, cx);
        }
    }

    fn render_datasets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let datasets = self.datasets();
        let count = datasets.len();
        let selected = self.selected.clone();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .w(px(206.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .justify_between()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("数据集")
                    .child(count.to_string()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .pb_2()
                    .gap_0p5()
                    .children(datasets.into_iter().enumerate().map(|(index, dataset)| {
                        let active = selected.as_deref() == Some(dataset.code.as_str());
                        let code = dataset.code.clone();
                        v_flex()
                            .id(("bcp-dataset", index))
                            .w_full()
                            .px_2()
                            .py_1p5()
                            .rounded(cx.theme().radius)
                            .cursor_pointer()
                            .when(active, |this| {
                                this.bg(cx.theme().sidebar_accent)
                                    .text_color(cx.theme().sidebar_primary)
                            })
                            .when(!active, |this| {
                                this.hover(|style| style.bg(cx.theme().sidebar_accent))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.selected.as_deref() != Some(code.as_str()) {
                                    this.select(code.clone(), window, cx);
                                }
                            }))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(dataset_label(dataset)),
                                    )
                                    .child(
                                        ui::mono(suffix(&dataset.code).to_owned(), cx)
                                            .text_xs()
                                            .text_color(muted),
                                    ),
                            )
                            .child(
                                ui::mono(format::count(dataset.rows), cx)
                                    .text_xs()
                                    .text_color(muted),
                            )
                    }))
                    .overflow_y_scrollbar(),
            )
    }

    fn render_dataset_head(
        &self,
        dataset: &BcpDataset,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loading = self.table.read(cx).delegate().is_loading();
        h_flex()
            .w_full()
            .flex_shrink_0()
            .justify_between()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(dataset_label(dataset)),
                    )
                    .child(ui::mono(dataset.code.clone(), cx))
                    .child(format!(
                        "{} 个文件 · {} 条记录 · {} 列",
                        format::count(dataset.files.len() as u64),
                        format::count(dataset.rows),
                        format::count(dataset.fields.len() as u64)
                    )),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Button::new("bcp-previous")
                            .small()
                            .icon(IconName::ChevronLeft)
                            .label("上一页")
                            .disabled(self.history.is_empty() || loading)
                            .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                    )
                    .child(format!("第 {} 页", self.history.len() + 1))
                    .child(
                        Button::new("bcp-next")
                            .small()
                            .label("下一页")
                            .icon(IconName::ChevronRight)
                            .disabled(self.next.is_none() || loading)
                            .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                    ),
            )
    }

    fn render_record(&self, dataset: &BcpDataset, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(record) = self.record.clone() else {
            return div().into_any_element();
        };
        let copy_row =
            |id: String, label: String, value: String, mono: bool, cx: &mut Context<Self>| {
                let text = if mono {
                    ui::mono(value.clone(), cx).text_xs()
                } else {
                    div().child(value.clone())
                };
                ui::field_row(
                    label,
                    h_flex()
                        .gap_1()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(text))
                        .when(!value.is_empty(), |this| {
                            this.child(Clipboard::new(SharedString::from(id)).value(value))
                        }),
                    cx,
                )
            };
        let mut content = Vec::new();
        for (index, field) in dataset.fields.iter().enumerate() {
            let value = record.values.get(index).cloned().unwrap_or_default();
            content.push(copy_row(
                format!("bcp-field-{index}"),
                field_title(field),
                value,
                false,
                cx,
            ));
        }
        for (index, value) in record.values.iter().enumerate().skip(dataset.fields.len()) {
            let extra = index - dataset.fields.len() + 1;
            content.push(copy_row(
                format!("bcp-extra-{index}"),
                format!("附加字段 {extra}"),
                value.clone(),
                false,
                cx,
            ));
        }
        let source = copy_row(
            "bcp-source".to_owned(),
            "来源文件".to_owned(),
            record.file_name.clone(),
            true,
            cx,
        );
        let code = copy_row(
            "bcp-code".to_owned(),
            "数据集".to_owned(),
            dataset.code.clone(),
            true,
            cx,
        );
        v_flex()
            .size_full()
            .child(ui::detail_header(
                format!("记录 #{}", format::count(record.number)),
                Button::new("close-bcp-record")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .accessibility_label("关闭详情")
                    .tooltip("关闭详情")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.record = None;
                        this.table.update(cx, |table, cx| table.clear_selection(cx));
                        cx.notify();
                    })),
                cx,
            ))
            .child(
                h_flex()
                    .gap_1()
                    .px_4()
                    .pt_1()
                    .child(Tag::secondary().small().child("BCP 记录"))
                    .child(Tag::secondary().small().child(dataset_label(dataset))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_3()
                    .child(ui::section_title("基本信息", cx))
                    .child(source)
                    .child(code)
                    .child(ui::section_title("记录内容", cx))
                    .children(content)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }
}

impl Render for BcpPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let category_rows: u64 = self.datasets().iter().map(|dataset| dataset.rows).sum();
        let table = self.table.read(cx).delegate();
        let page_summary =
            if !table.is_loading() && table.error().is_none() && self.selected.is_some() {
                format!(
                    "当前页 {} / {} 条",
                    format::count(table.shown_count() as u64),
                    format::count(table.page_count() as u64)
                )
            } else {
                String::new()
            };
        let dataset = self.dataset().cloned();
        let main = match &dataset {
            Some(dataset) => v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.render_dataset_head(dataset, cx))
                .child(div().flex_1().min_h_0().child(ui::data_table(&self.table)))
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(ui::empty_state(
                    Icon::new(ExtraIcon::Database),
                    "这个分类没有数据集",
                    None,
                ))
                .into_any_element(),
        };
        let detail = if self.record.is_some() {
            dataset
                .as_ref()
                .map(|dataset| self.render_record(dataset, cx).into_any_element())
        } else {
            None
        };
        v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .child(Icon::new(ExtraIcon::Database).text_color(cx.theme().primary))
                    .child(div().text_sm().child(category_label(self.category)))
                    .child(
                        Tag::info()
                            .small()
                            .child(format!("{} 条", format::count(category_rows))),
                    )
                    .child(div().flex_1())
                    .child(
                        div().w(px(240.)).child(
                            Input::new(&self.filter)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(IconName::Search).small())
                                .disabled(dataset.is_none()),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_datasets(cx))
                    .child(ui::detail_frame(
                        "bcp-details",
                        main,
                        detail,
                        window.viewport_size().width < px(1352.),
                        px(380.),
                        cx,
                    )),
            )
            .child(ui::page_footer(
                ui::mono(self.catalog.path.clone(), cx).truncate(),
                page_summary,
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::{category_label, category_of, suffix};

    #[test]
    fn datasets_sort_into_categories_by_their_code() {
        assert_eq!(suffix("WA_MFORENSICS_020500"), "020500");
        assert_eq!(suffix("CODE"), "CODE");
        assert_eq!(category_of("WA_MFORENSICS_020500"), "02");
        assert_eq!(category_of("WA_MFORENSICS_990100"), "other");
        assert_eq!(category_of("X"), "other");
        assert_eq!(category_label(Some("02")), "即时通信");
        assert_eq!(category_label(Some("other")), "其他数据");
        assert_eq!(category_label(None), "全部数据");
    }
}
