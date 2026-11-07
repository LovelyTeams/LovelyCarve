//! The add-evidence chooser: disk images, folders or physical disks. Each
//! choice ends in one or more evidence tabs of the active case.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::evidence::image as image_commands, core::evidence::physical_disk::PhysicalDisk,
};

use super::Workspace;
use crate::{backend::Backend, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// Open the chooser over the workspace.
pub fn open(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
    let chooser = cx.new(|_| SourceChooser {
        workspace,
        disks: DiskList::Hidden,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("添加检材")
            .w(gpui_kit::px(560.))
            .child(chooser.clone())
    });
}

enum DiskList {
    Hidden,
    Loading,
    Loaded(Vec<PhysicalDisk>),
    Failed(SharedString),
}

struct SourceChooser {
    workspace: WeakEntity<Workspace>,
    disks: DiskList,
}

impl SourceChooser {
    fn open_paths(&self, paths: Vec<String>, window: &mut Window, cx: &mut App) {
        if paths.is_empty() {
            return;
        }
        window.close_dialog(cx);
        self.workspace
            .update(cx, |workspace, cx| {
                for path in paths {
                    workspace.open_tab(path, window, cx);
                }
            })
            .ok();
    }

    fn pick(&mut self, directories: bool, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = ui::prompt_paths(
            PathPromptOptions {
                files: !directories,
                directories,
                multiple: !directories,
                prompt: Some(
                    if directories {
                        "选择文件夹作为逻辑检材"
                    } else {
                        "选择磁盘镜像（可多选）"
                    }
                    .into(),
                ),
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = prompt.await;
            let paths = paths
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            this.update_in(cx, |this, window, cx| this.open_paths(paths, window, cx))
                .ok();
        })
        .detach();
    }

    fn list_disks(&mut self, cx: &mut Context<Self>) {
        self.disks = DiskList::Loading;
        cx.notify();
        let listed = Backend::global(cx)
            .run(|_, _| async move { image_commands::physical_disks_list().await });
        cx.spawn(async move |this, cx| {
            let disks = listed.await;
            this.update(cx, |this, cx| {
                this.disks = match disks {
                    Ok(disks) => DiskList::Loaded(disks),
                    Err(error) => DiskList::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choice(
        &self,
        id: &'static str,
        icon: impl Into<Icon>,
        title: &'static str,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        h_flex()
            .id(id)
            .w_full()
            .gap_3()
            .px_3()
            .py_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().list_hover))
            .child(icon.into().text_color(cx.theme().primary))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    ),
            )
    }
}

impl Render for SourceChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disks = match &self.disks {
            DiskList::Hidden => None,
            DiskList::Loading => Some(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Spinner::new().small())
                    .child("正在枚举物理磁盘…")
                    .into_any_element(),
            ),
            DiskList::Failed(error) => Some(ui::error_text(error.clone(), cx).into_any_element()),
            DiskList::Loaded(disks) if disks.is_empty() => Some(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("没有可读取的物理磁盘（Windows 通常需要以管理员身份运行）。")
                    .into_any_element(),
            ),
            DiskList::Loaded(disks) => Some(
                v_flex()
                    .gap_1()
                    .children(disks.iter().enumerate().map(|(index, disk)| {
                        let path = disk.path.clone();
                        h_flex()
                            .w_full()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .child(
                                Icon::new(IconName::HardDrive)
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_sm().child(format!(
                                        "{} · {}{}",
                                        disk.display,
                                        format::binary_bytes(disk.size_bytes),
                                        disk.model
                                            .as_ref()
                                            .map(|model| format!(" · {model}"))
                                            .unwrap_or_default()
                                    )))
                            .when(disk.is_removable, |this| {
                                this.child(Tag::secondary().small().child("可移动"))
                            })
                            .child(
                                Button::new(("open-disk", index))
                                    .small()
                                    .label("打开")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_paths(vec![path.clone()], window, cx)
                                    })),
                            )
                    }))
                    .into_any_element(),
            ),
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("选择检材来源，打开后会登记到当前案件。"),
            )
            .child(
                self.choice(
                    "source-images",
                    Icon::new(IconName::HardDrive),
                    "镜像文件",
                    "RAW / E01 / VMDK / VHD(X) / ISO / DMG / LDBK 等磁盘镜像，也可选择归档与逻辑镜像",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(false, window, cx))),
            )
            .child(
                self.choice(
                    "source-folder",
                    Icon::new(IconName::FolderOpen),
                    "文件夹",
                    "本机目录、已解包数据集或 iTunes / Finder 的 iOS 备份",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(true, window, cx))),
            )
            .child(
                self.choice(
                    "source-disk",
                    Icon::new(ExtraIcon::Server),
                    "物理磁盘",
                    "直接读取本机块设备；Windows 通常需要管理员权限",
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.list_disks(cx))),
            )
            .when_some(disks, |this, disks| this.child(disks))
            .child(
                h_flex().justify_end().child(
                    Button::new("cancel-sources")
                        .ghost()
                        .label("取消")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}
