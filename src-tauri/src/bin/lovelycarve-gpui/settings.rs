//! 设置: appearance (theme and interface size), the installed plugin
//! packages, and version information.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        spinner::Spinner,
        switch::Switch,
        tab::{Tab, TabBar},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
        WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, App, AppContext as _, Context, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Window,
};
use lovelycarve_lib::{commands::extensions::plugin as plugin_commands, host::State};
use serde_json::Value;

use crate::{
    backend::Backend,
    prefs::{Preferences, ThemeChoice},
    ui,
};

const FONT_SIZES: [f32; 4] = [13.0, 14.0, 15.0, 16.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Appearance,
    Plugins,
    About,
}

enum Plugins {
    Loading,
    Loaded(Value),
    Failed(SharedString),
}

pub struct SettingsView {
    section: Section,
    plugins: Plugins,
    busy: bool,
}

/// Open the settings dialog.
pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let mut view = SettingsView {
            section: Section::Appearance,
            plugins: Plugins::Loading,
            busy: false,
        };
        view.load_plugins(cx);
        view
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .centered()
            .title(ui::dialog_heading(
                IconName::Settings,
                "设置中心",
                "更改自动保存",
                cx,
            ))
            .w(px(760.))
            .child(view.clone())
    });
}

impl SettingsView {
    fn load_plugins(&mut self, cx: &mut Context<Self>) {
        let listed = Backend::global(cx)
            .run(|_, state| async move { plugin_commands::plugin_list(State::new(&state)).await });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                this.plugins = match listed.map(serde_json::to_value) {
                    Ok(Ok(value)) => Plugins::Loaded(value),
                    Ok(Err(error)) => Plugins::Failed(error.to_string().into()),
                    Err(error) => Plugins::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = ui::prompt_paths(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择插件包（.lcpkg）".into()),
            },
            cx,
        );
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(archive) = prompt.await.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.busy = true;
                cx.notify();
            })
            .ok();
            let archive = archive.display().to_string();
            let installed = backend
                .run(move |_, state| async move {
                    plugin_commands::plugin_install(State::new(&state), archive).await
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match installed {
                    Ok(_) => ui::notify_success(
                        window,
                        cx,
                        "插件已安装",
                        "新插件的分析器已可在「快速分析」中使用。",
                    ),
                    Err(error) => ui::notify_error(window, cx, "安装插件失败", error.message),
                }
                this.load_plugins(cx);
            })
            .ok();
        })
        .detach();
    }

    fn uninstall(&mut self, plugin_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        ui::confirm(
            window,
            cx,
            "卸载插件",
            format!("卸载插件 {plugin_id}？已产生的分析结果会保留。"),
            "卸载",
            move |window, cx| {
                let plugin_id = plugin_id.clone();
                let removed = Backend::global(cx).run(move |_, state| async move {
                    plugin_commands::plugin_uninstall(State::new(&state), plugin_id).await
                });
                let view = view.clone();
                window
                    .spawn(cx, async move |cx| {
                        let removed = removed.await;
                        cx.update(|window, cx| {
                            if let Err(error) = removed {
                                ui::notify_error(window, cx, "卸载插件失败", error.message);
                            }
                            view.update(cx, |this, cx| this.load_plugins(cx)).ok();
                        })
                        .ok();
                    })
                    .detach();
            },
        );
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preferences = Preferences::global(cx).clone();
        let theme_index = ThemeChoice::ALL
            .iter()
            .position(|choice| *choice == preferences.theme);
        let size_index = FONT_SIZES
            .iter()
            .position(|size| (*size - preferences.font_size).abs() < 0.1);
        v_flex()
            .gap_5()
            .child(ui::field_row(
                "主题",
                TabBar::new("theme-choice")
                    .segmented()
                    .small()
                    .children(ThemeChoice::ALL.into_iter().map(|choice| {
                        let icon = match choice {
                            ThemeChoice::System => Icon::new(gpui_kit::assets::IconName::Monitor),
                            ThemeChoice::Light => Icon::new(IconName::Sun),
                            ThemeChoice::Dark => Icon::new(IconName::Moon),
                        };
                        Tab::new()
                            .label(choice.label())
                            .prefix(div().pl_2().child(icon.small()))
                    }))
                    .when_some(theme_index, |bar, index| bar.selected_index(index))
                    .on_click(|index: &usize, window, cx| {
                        if let Some(choice) = ThemeChoice::ALL.get(*index).copied() {
                            Preferences::update(window, cx, |preferences| {
                                preferences.theme = choice
                            });
                        }
                    }),
                cx,
            ))
            .child(ui::field_row(
                "界面大小",
                TabBar::new("font-size")
                    .segmented()
                    .small()
                    .children(["紧凑", "标准", "宽松", "较大"].map(|label| Tab::new().label(label)))
                    .when_some(size_index, |bar, index| bar.selected_index(index))
                    .on_click(|index: &usize, window, cx| {
                        if let Some(size) = FONT_SIZES.get(*index).copied() {
                            Preferences::update(window, cx, |preferences| {
                                preferences.font_size = size
                            });
                        }
                    }),
                cx,
            ))
            .child(ui::field_row(
                "首页动画",
                Switch::new("welcome-motion")
                    .label("播放欢迎页动画")
                    .checked(preferences.welcome_motion)
                    .on_click(|enabled, window, cx| {
                        Preferences::update(window, cx, |preferences| {
                            preferences.welcome_motion = *enabled;
                        });
                    }),
                cx,
            ))
    }

    fn render_plugins(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.plugins {
            Plugins::Loading => h_flex()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child("正在读取插件…")
                .into_any_element(),
            Plugins::Failed(error) => ui::error_text(error.clone(), cx).into_any_element(),
            Plugins::Loaded(value) => {
                let packages = value["packages"].as_array().cloned().unwrap_or_default();
                let rejected = value["rejected"].as_array().cloned().unwrap_or_default();
                let directory = value["installDirectory"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                v_flex()
                    .gap_2()
                    .when(packages.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("没有发现插件包。"),
                        )
                    })
                    .children(packages.iter().enumerate().map(|(index, package)| {
                        let id = package["id"].as_str().unwrap_or_default().to_owned();
                        let name = package["name"].as_str().unwrap_or(&id).to_owned();
                        let version = package["version"].as_str().unwrap_or_default().to_owned();
                        let removable = package["removable"].as_bool().unwrap_or(false);
                        let analyzers = package["analyzers"].as_array().map(Vec::len).unwrap_or(0);
                        let error = package["error"].as_str().map(str::to_owned);
                        let present = package["libraryPresent"].as_bool().unwrap_or(true);
                        let remove_id = id.clone();
                        h_flex()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                                    .child(name),
                                            )
                                            .child(Tag::secondary().xsmall().child(version))
                                            .when(!present, |this| {
                                                this.child(
                                                    Tag::danger().xsmall().child("缺少库文件"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{id} · {analyzers} 个分析器")),
                                    )
                                    .when_some(error, |this, error| {
                                        this.child(ui::error_text(error, cx))
                                    }),
                            )
                            .when(removable, |this| {
                                this.child(
                                    Button::new(("uninstall-plugin", index))
                                        .small()
                                        .ghost()
                                        .label("卸载")
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.uninstall(remove_id.clone(), window, cx)
                                        })),
                                )
                            })
                    }))
                    .when(!rejected.is_empty(), |this| {
                        this.child(ui::section_title("未能加载", cx))
                            .children(rejected.iter().map(|package| {
                                let label = package["directory"]
                                    .as_str()
                                    .or_else(|| package["path"].as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                                let reason = package["reason"]
                                    .as_str()
                                    .or_else(|| package["error"].as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().child(label))
                                    .child(ui::error_text(reason, cx))
                            }))
                    })
                    .when(!directory.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("安装目录：{directory}")),
                        )
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_title("插件包", cx))
                    .child(div().flex_1())
                    .child(
                        Button::new("install-plugin")
                            .small()
                            .label("安装插件…")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.install(window, cx))),
                    ),
            )
            .child(body)
    }

    fn render_about(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let data_dir = lovelycarve_lib::host::default_app_data_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".to_owned());
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("LovelyCarve"),
            )
            .child(ui::field_row("版本", env!("CARGO_PKG_VERSION"), cx))
            .child(ui::field_row("界面", "GPUI Kit 原生桌面", cx))
            .child(ui::field_row(
                "数据目录",
                ui::mono(data_dir, cx).text_xs(),
                cx,
            ))
            .child(ui::field_row("许可", "AGPL-3.0-only", cx))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.section;
        let body = match section {
            Section::Appearance => self.render_appearance(cx).into_any_element(),
            Section::Plugins => self.render_plugins(cx).into_any_element(),
            Section::About => self.render_about(cx).into_any_element(),
        };
        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .pb_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(
                        [
                            (
                                "appearance",
                                "界面",
                                gpui_kit::assets::IconName::Palette,
                                Section::Appearance,
                            ),
                            (
                                "plugins",
                                "插件",
                                gpui_kit::assets::IconName::Puzzle,
                                Section::Plugins,
                            ),
                            (
                                "about",
                                "关于",
                                gpui_kit::assets::IconName::Info,
                                Section::About,
                            ),
                        ]
                        .map(|(id, label, icon, target)| {
                            Button::new(id)
                                .ghost()
                                .small()
                                .selected(section == target)
                                .icon(Icon::new(icon))
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section = target;
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .child(div().max_h(px(440.)).child(body).overflow_y_scrollbar())
            .child(
                h_flex()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .when(section == Section::Appearance, |footer| {
                        footer.child(
                            Button::new("reset-appearance")
                                .ghost()
                                .label("恢复本页默认设置")
                                .on_click(|_, window, cx| {
                                    let defaults = Preferences::default();
                                    Preferences::update(window, cx, |preferences| {
                                        preferences.theme = defaults.theme;
                                        preferences.font_size = defaults.font_size;
                                        preferences.welcome_motion = defaults.welcome_motion;
                                    });
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("finish-settings")
                            .primary()
                            .label("完成")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}
