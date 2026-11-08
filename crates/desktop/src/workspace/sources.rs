//! The add-evidence chooser: disk images, folders or physical disks. Each
//! choice ends in one or more evidence tabs of the active case.
use gpui_kit::{
    component::{
        button::{Button, ButtonCustomVariant, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, App, AppContext as _, Context, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, WeakEntity, Window,
};
use lovelycarve_backend::{
    commands::evidence::image as image_commands,
    core::{casework::case::evidence_path_key, evidence::physical_disk::PhysicalDisk},
};
use std::path::Path;

use super::Workspace;
use crate::{backend::Backend, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// Open the chooser over the workspace.
pub fn open(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
    let chooser = cx.new(|_| SourceChooser {
        workspace,
        disks: DiskList::Hidden,
        raid_members: None,
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .centered()
            .title(ui::dialog_heading(
                IconName::Plus,
                "添加检材",
                "选择检材来源后会登记到当前案件",
                cx,
            ))
            .w(px(480.))
            .child(chooser.clone())
    });
}

enum DiskList {
    Hidden,
    Loading,
    Loaded(Vec<PhysicalDisk>),
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Image,
    Folder,
    Bcp,
    RaidMember,
}

struct SourceChooser {
    workspace: WeakEntity<Workspace>,
    disks: DiskList,
    raid_members: Option<Vec<String>>,
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

    fn pick(&mut self, kind: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let directories = kind == Pick::Folder;
        let prompt = ui::prompt_paths(
            PathPromptOptions {
                files: !directories,
                directories,
                multiple: !directories,
                prompt: Some(
                    match kind {
                        Pick::Image => "选择磁盘镜像（可多选）",
                        Pick::Folder => "选择文件夹作为逻辑检材",
                        Pick::Bcp => "选择 BCP 文件（可多选）",
                        Pick::RaidMember => "选择 RAID 成员盘（可多选）",
                    }
                    .into(),
                ),
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = prompt.await;
            if paths.is_empty() {
                return;
            }
            let paths = paths
                .into_iter()
                .filter(|path| {
                    kind != Pick::Bcp
                        || path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("bcp"))
                })
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            this.update_in(cx, |this, window, cx| {
                if kind == Pick::RaidMember {
                    if let Some(members) = &mut this.raid_members {
                        for path in paths {
                            let key = evidence_path_key(&path);
                            if !members
                                .iter()
                                .any(|member| evidence_path_key(member) == key)
                            {
                                members.push(path);
                            }
                        }
                        cx.notify();
                    }
                } else if paths.is_empty() {
                    ui::notify_error(
                        window,
                        cx,
                        "未选择 BCP 文件",
                        "请选择扩展名为 .bcp 的文件。",
                    );
                } else if kind == Pick::Bcp {
                    window.close_dialog(cx);
                    this.workspace
                        .update(cx, |workspace, cx| {
                            for path in paths {
                                if let Some(folder) = Path::new(&path).parent() {
                                    workspace.open_bcp_tab(
                                        folder.display().to_string(),
                                        window,
                                        cx,
                                    );
                                }
                            }
                        })
                        .ok();
                } else {
                    this.open_paths(paths, window, cx);
                }
            })
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
        cx: &App,
    ) -> Button {
        Button::new(id)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(cx.theme().popover)
                    .hover(cx.theme().accent)
                    .active(cx.theme().sidebar_accent),
            )
            .w_full()
            .h(px(68.))
            .px_4()
            .rounded(px(crate::theme::PANEL_RADIUS))
            .border_1()
            .border_color(cx.theme().border)
            .accessibility_label(title)
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .text_left()
                    .child(
                        h_flex()
                            .size(px(40.))
                            .flex_shrink_0()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().accent)
                            .child(icon.into().size(px(20.)).text_color(cx.theme().primary)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(description),
                            ),
                    )
                    .when(id == "source-disk", |row| {
                        row.child(Tag::warning().outline().small().child("需提权"))
                    })
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(18.))
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
    }
}

impl Render for SourceChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(members) = &self.raid_members {
            return v_flex()
                .w_full()
                .gap_3()
                .child(ui::card_heading(
                    Icon::new(ExtraIcon::Layers),
                    "RAID 磁盘",
                    cx,
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "将成员镜像加入同一批次。载入后可在镜像信息中查看检测结果与重组入口。",
                        ),
                )
                .child(
                    ui::card(cx)
                        .p_2()
                        .max_h(px(240.))
                        .when(members.is_empty(), |card| {
                            card.child(
                                div()
                                    .p_2()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("尚未添加成员盘"),
                            )
                        })
                        .children(members.iter().enumerate().map(|(index, path)| {
                            h_flex()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .child(Icon::new(IconName::HardDrive).small())
                                .child(
                                    ui::mono(path.clone(), cx)
                                        .text_xs()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate(),
                                )
                                .child(
                                    Button::new(("remove-raid-member", index))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Close)
                                        .accessibility_label("从队列移除成员盘")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(members) = &mut this.raid_members {
                                                if index < members.len() {
                                                    members.remove(index);
                                                }
                                            }
                                            cx.notify();
                                        })),
                                )
                        }))
                        .overflow_y_scrollbar(),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("back-from-raid")
                                .ghost()
                                .label("返回")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.raid_members = None;
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("pick-raid-members")
                                .icon(IconName::Plus)
                                .label("添加成员盘")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick(Pick::RaidMember, window, cx)
                                })),
                        )
                        .child(
                            Button::new("load-raid-members")
                                .primary()
                                .label("载入成员盘")
                                .disabled(members.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(members) = &this.raid_members {
                                        this.open_paths(members.clone(), window, cx);
                                    }
                                })),
                        ),
                );
        }
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
        if let Some(disks) = disks {
            return v_flex()
                .w_full()
                .gap_3()
                .child(ui::card_heading(
                    Icon::new(ExtraIcon::HardDriveDownload),
                    "物理磁盘",
                    cx,
                ))
                .child(div().max_h(px(320.)).child(disks).overflow_y_scrollbar())
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            Button::new("back-from-disks")
                                .ghost()
                                .label("返回")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.disks = DiskList::Hidden;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("refresh-physical-disks")
                                .icon(IconName::RefreshCw)
                                .label("刷新")
                                .disabled(matches!(self.disks, DiskList::Loading))
                                .on_click(cx.listener(|this, _, _, cx| this.list_disks(cx))),
                        ),
                );
        }
        v_flex()
            .w_full()
            .gap_2()
            .child(
                self.choice(
                    "source-images",
                    Icon::new(IconName::HardDrive),
                    "镜像文件",
                    "RAW / E01 / VMDK / VHD(X) / ISO / DMG / LDBK 等磁盘镜像",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Image, window, cx))),
            )
            .child(
                self.choice(
                    "source-folder",
                    Icon::new(IconName::FolderOpen),
                    "文件夹",
                    "本机目录、已解包数据集或 iTunes / Finder 的 iOS 备份",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Folder, window, cx))),
            )
            .child(
                self.choice(
                    "source-bcp",
                    ExtraIcon::Database,
                    "BCP 文件",
                    "打开网安数据交换 BCP，按索引浏览分类与记录",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Bcp, window, cx))),
            )
            .child(
                self.choice(
                    "source-raid",
                    ExtraIcon::Layers,
                    "RAID 磁盘",
                    "逐块放入成员盘，载入后检测与重组阵列",
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.raid_members = Some(Vec::new());
                    cx.notify();
                })),
            )
            .child(
                self.choice(
                    "source-disk",
                    Icon::new(ExtraIcon::HardDriveDownload),
                    "物理磁盘",
                    "直接读取本机块设备；Windows 通常需要管理员权限",
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.list_disks(cx))),
            )
    }
}
