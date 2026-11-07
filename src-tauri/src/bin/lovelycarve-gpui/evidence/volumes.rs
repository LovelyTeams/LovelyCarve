//! 卷与分区: the partition layout and the work done per volume — BitLocker
//! and container unlocks, Volume Shadow Copies and read-only mounts.
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        group_box::{GroupBox, GroupBoxVariants as _},
        h_flex,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, relative, AppContext as _, ClipboardItem, Context, Entity, Hsla, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::{
        app_error::AppError,
        evidence::{bitlocker as bitlocker_commands, shadow_copy as shadow_commands},
        system::mount as mount_commands,
    },
    core::evidence::image::{ImageInfo, Partition},
    host::State,
};
use serde::Deserialize;

use super::{
    filesystem_label,
    unlock::{BitlockerDialog, BitlockerUnlocked, ContainerDialog, ContainerUnlockRequest},
};
use crate::{backend::Backend, evidence::EvidenceTab, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// Partition kinds whose `part:{id}` node the read-only mount layer projects.
const MOUNTABLE_KINDS: [&str; 41] = [
    "ext4",
    "btrfs",
    "fat32",
    "fat12",
    "fat16",
    "exfat",
    "cdfs",
    "squashfs",
    "cramfs",
    "romfs",
    "minifs",
    "erofs",
    "f2fs",
    "qnx4",
    "qnx6",
    "udf",
    "minix",
    "jfs",
    "reiserfs",
    "amiga",
    "befs",
    "gfs2",
    "ocfs2",
    "nilfs2",
    "bcachefs",
    "hfs",
    "ubifs",
    "jffs2",
    "yaffs1",
    "yaffs2",
    "ubi",
    "apfscontainer",
    "hfsplus",
    "hfsx",
    "lvm",
    "ntfs",
    "refs",
    "ufs",
    "xfs",
    "zfs",
    "bitlocker",
];

/// Logical evidence has no block layer: no encrypted containers, shadow
/// copies or decrypted exports.
const LOGICAL_FORMATS: [&str; 7] = [
    "folder",
    "tar",
    "zip",
    "sevenz",
    "ad1",
    "docker",
    "android_backup",
];

/// A serialized enum's wire name (`ntfs`, `shadow_copy`, …).
pub fn wire_name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether this evidence has a block layer to unlock, scan or export.
pub fn supports_block_operations(info: &ImageInfo) -> bool {
    !info.index_only
        && info.ios_backup.is_none()
        && !LOGICAL_FORMATS.contains(&wire_name(&info.source_format).as_str())
}

/// The merged root of evidence without a partition table, when it can be
/// mounted (archives, logical images and containers; folders already are
/// directories).
fn synthetic_root(info: &ImageInfo) -> Option<(&'static str, &'static str)> {
    match wire_name(&info.source_format).as_str() {
        "tar" | "android_backup" => Some(("tar:", "归档根")),
        "zip" => Some(("zip:", "归档根")),
        "sevenz" => Some(("sevenz:", "归档根")),
        "ad1" => Some(("ad1:", "逻辑镜像根")),
        "docker" => Some(("docker:", "容器根")),
        _ => None,
    }
}

/// A UNC share name from a volume title: Windows-safe, short and unique per
/// mount.
fn share_name_for(title: &str, mount_id: &str) -> String {
    let mut cleaned = String::new();
    for character in title.chars() {
        let replacement = if "\"/\\[]:|<>+=;,?*".contains(character)
            || character.is_control()
            || character.is_whitespace()
        {
            '_'
        } else {
            character
        };
        if replacement == '_' && cleaned.ends_with('_') {
            continue;
        }
        cleaned.push(replacement);
    }
    let cleaned: String = cleaned.trim_matches('_').chars().take(48).collect();
    let prefix = if cleaned.is_empty() {
        "mount".to_owned()
    } else {
        cleaned
    };
    let suffix: String = mount_id.chars().take(8).collect();
    format!("LC_{prefix}_{suffix}")
}

#[derive(Clone, Debug, Deserialize)]
struct ShadowVolume {
    partition_id: u8,
    shadow_copies: Vec<ShadowCopy>,
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ShadowCopy {
    index: usize,
    store_id: String,
    shadow_copy_id: Option<String>,
    created_unix_ms: Option<i64>,
    persistent: bool,
    client_accessible: bool,
    operating_machine: Option<String>,
    unavailable_reason: Option<String>,
    open_image_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct MountView {
    id: String,
    image_id: String,
    root_node_id: String,
    mount_point: String,
    share: Option<MountShare>,
}

#[derive(Clone, Debug, Deserialize)]
struct MountShare {
    unc_path: String,
}

#[derive(Clone, Debug, Default)]
struct BitlockerStatus {
    locked: bool,
    encryption: Option<String>,
}

enum Shadows {
    Unsupported,
    Loading,
    Loaded(Vec<ShadowVolume>),
    Failed(SharedString),
}

pub struct VolumesCard {
    info: Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    bitlocker: HashMap<u8, BitlockerStatus>,
    dialogs: HashMap<u8, Entity<BitlockerDialog>>,
    /// The BitLocker dialog currently on screen.
    showing: Option<u8>,
    shadows: Shadows,
    expanded: HashSet<u8>,
    shadow_opening: Option<(u8, usize)>,
    shadow_error: Option<(u8, SharedString)>,
    mounts: Vec<MountView>,
    /// Mount node ids and `share:{mount id}` keys with work in flight.
    busy: HashSet<String>,
    exporting: HashSet<u8>,
    _subscriptions: Vec<Subscription>,
}

impl VolumesCard {
    pub fn new(info: Rc<ImageInfo>, tab: WeakEntity<EvidenceTab>, cx: &mut Context<Self>) -> Self {
        let shadow_capable =
            supports_block_operations(&info) && wire_name(&info.source_format) != "shadow_copy";
        let mut card = Self {
            info,
            tab,
            bitlocker: HashMap::new(),
            dialogs: HashMap::new(),
            showing: None,
            shadows: if shadow_capable {
                Shadows::Loading
            } else {
                Shadows::Unsupported
            },
            expanded: HashSet::new(),
            shadow_opening: None,
            shadow_error: None,
            mounts: Vec::new(),
            busy: HashSet::new(),
            exporting: HashSet::new(),
            _subscriptions: Vec::new(),
        };
        card.refresh_bitlocker(cx);
        if shadow_capable {
            card.load_shadows(cx);
        }
        card.refresh_mounts(cx);
        card
    }

    fn refresh_bitlocker(&mut self, cx: &mut Context<Self>) {
        let partitions: Vec<u8> = self
            .info
            .partitions
            .iter()
            .filter(|partition| wire_name(&partition.kind) == "bitlocker")
            .map(|partition| partition.id)
            .collect();
        if partitions.is_empty() || !supports_block_operations(&self.info) {
            return;
        }
        let image_id = self.info.id.clone();
        let probing = Backend::global(cx).run(move |_, state| async move {
            let mut statuses = Vec::new();
            for partition in partitions {
                match bitlocker_commands::bitlocker_status(
                    State::new(&state),
                    image_id.clone(),
                    partition,
                )
                .await
                {
                    Ok(status) => {
                        statuses.push((partition, serde_json::to_value(status).unwrap_or_default()))
                    }
                    // The partition keeps its last known (locked) state.
                    Err(error) => tracing::debug!(
                        "bitlocker status of partition {partition}: {}",
                        error.message
                    ),
                }
            }
            Ok::<_, AppError>(statuses)
        });
        cx.spawn(async move |this, cx| {
            let Ok(statuses) = probing.await else {
                return;
            };
            this.update(cx, |this, cx| {
                for (partition, status) in statuses {
                    this.bitlocker.insert(
                        partition,
                        BitlockerStatus {
                            locked: status["locked"].as_bool().unwrap_or(true),
                            encryption: status["encryption_method"].as_str().map(str::to_owned),
                        },
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_shadows(&mut self, cx: &mut Context<Self>) {
        self.shadows = Shadows::Loading;
        let image_id = self.info.id.clone();
        let listing = Backend::global(cx).run(move |_, state| async move {
            let volumes = shadow_commands::shadow_copy_list(State::new(&state), image_id).await?;
            serde_json::to_value(volumes)
                .map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let listed = listing.await;
            this.update(cx, |this, cx| {
                this.shadows = match listed.map(serde_json::from_value::<Vec<ShadowVolume>>) {
                    Ok(Ok(volumes)) => Shadows::Loaded(volumes),
                    Ok(Err(error)) => Shadows::Failed(error.to_string().into()),
                    Err(error) => Shadows::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh_mounts(&mut self, cx: &mut Context<Self>) {
        let listing = Backend::global(cx).run(|_, state| async move {
            let mounts = mount_commands::fs_mounts(State::new(&state)).await?;
            serde_json::to_value(mounts).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            // Without Dokan (or off Windows) there are no mounts to show.
            let mounts = match listing.await {
                Ok(value) => serde_json::from_value::<Vec<MountView>>(value).unwrap_or_default(),
                Err(error) => {
                    tracing::debug!("mount list unavailable: {}", error.message);
                    Vec::new()
                }
            };
            this.update(cx, |this, cx| {
                this.mounts = mounts;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn mount_for(&self, node_id: &str) -> Option<&MountView> {
        self.mounts
            .iter()
            .find(|mount| mount.image_id == self.info.id && mount.root_node_id == node_id)
    }

    fn open_bitlocker(&mut self, partition_id: u8, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = match self.dialogs.get(&partition_id) {
            Some(dialog) => dialog.clone(),
            None => {
                let image_id = self.info.id.clone();
                let dialog = cx.new(|cx| BitlockerDialog::new(image_id, partition_id, window, cx));
                let subscription = cx.subscribe_in(
                    &dialog,
                    window,
                    move |this, _, _: &BitlockerUnlocked, window, cx| {
                        if this.showing == Some(partition_id) {
                            this.showing = None;
                            window.close_dialog(cx);
                        }
                        this.refresh_bitlocker(cx);
                        // The decrypted volume is browsable now, and its VSS area readable.
                        if matches!(this.shadows, Shadows::Loaded(_) | Shadows::Failed(_)) {
                            this.load_shadows(cx);
                        }
                        this.tab
                            .update(cx, |tab, cx| tab.reload_files(window, cx))
                            .ok();
                    },
                );
                self._subscriptions.push(subscription);
                self.dialogs.insert(partition_id, dialog.clone());
                dialog
            }
        };
        self.showing = Some(partition_id);
        let card = cx.entity().downgrade();
        window.open_dialog(cx, move |shell, _, _| {
            let card = card.clone();
            shell
                .title(format!("解锁 BitLocker 分区 {partition_id}"))
                .w(px(580.))
                .child(dialog.clone())
                .on_close(move |_, _, cx| {
                    card.update(cx, |card, _| card.showing = None).ok();
                })
        });
    }

    fn open_container(
        &mut self,
        partition: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = Rc::clone(&self.info);
        let dialog = cx.new(|cx| ContainerDialog::new(info, partition, window, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |this, _, request: &ContainerUnlockRequest, window, cx| {
                window.close_dialog(cx);
                let options = request.0.clone();
                this.tab
                    .update(cx, |tab, cx| {
                        tab.start_container_unlock(options, window, cx)
                    })
                    .ok();
                ui::notify_success(window, cx, "已开始后台解锁", "可在任务中心查看状态或取消");
            },
        );
        self._subscriptions.push(subscription);
        let title = match partition {
            Some(partition) => format!("解锁分区 {partition}"),
            None => "解锁加密容器".to_owned(),
        };
        window.open_dialog(cx, move |shell, _, _| {
            shell.title(title.clone()).w(px(560.)).child(dialog.clone())
        });
    }

    fn export_decrypted(&mut self, partition_id: u8, window: &mut Window, cx: &mut Context<Self>) {
        if self.exporting.contains(&partition_id) {
            return;
        }
        let stem = format::file_name(&self.info.path)
            .rsplit_once('.')
            .map(|(stem, _)| stem.to_owned())
            .unwrap_or_else(|| format::file_name(&self.info.path).to_owned());
        let prompt = ui::prompt_save(&format!("{stem}-p{partition_id}-decrypted.dd"), cx);
        let image_id = self.info.id.clone();
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(destination) = prompt.await else {
                return;
            };
            let shown = destination.display().to_string();
            let exporting = backend.run(move |host, state| async move {
                bitlocker_commands::bitlocker_export_decrypted_image(
                    host,
                    State::new(&state),
                    image_id,
                    partition_id,
                    destination.display().to_string(),
                    uuid::Uuid::new_v4().to_string(),
                )
                .await
            });
            this.update(cx, |this, cx| {
                this.exporting.insert(partition_id);
                cx.notify();
            })
            .ok();
            let exported = exporting.await;
            this.update_in(cx, |this, window, cx| {
                this.exporting.remove(&partition_id);
                match exported {
                    Ok(bytes) => ui::notify_success(
                        window,
                        cx,
                        "解密镜像已导出",
                        format!("{shown}（{}）", format::binary_bytes(bytes)),
                    ),
                    Err(error) if error.cancelled => {}
                    Err(error) => ui::notify_error(window, cx, "导出解密镜像失败", error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_shadow(&mut self, partition_id: u8, copy: ShadowCopy, cx: &mut Context<Self>) {
        if self.shadow_opening.is_some() {
            return;
        }
        self.shadow_opening = Some((partition_id, copy.index));
        self.shadow_error = None;
        let image_id = self.info.id.clone();
        let opening = Backend::global(cx).run(move |_, state| async move {
            shadow_commands::shadow_copy_open(
                State::new(&state),
                image_id,
                partition_id,
                copy.index,
                Some(uuid::Uuid::new_v4().to_string()),
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let opened = opening.await;
            this.update(cx, |this, cx| {
                this.shadow_opening = None;
                match opened {
                    Ok(image) => {
                        if let Shadows::Loaded(volumes) = &mut this.shadows {
                            if let Some(copy) = volumes
                                .iter_mut()
                                .filter(|volume| volume.partition_id == partition_id)
                                .flat_map(|volume| volume.shadow_copies.iter_mut())
                                .find(|candidate| candidate.index == copy.index)
                            {
                                copy.open_image_id = Some(image.id.clone());
                            }
                        }
                        let path = image.path;
                        this.tab.update(cx, |tab, cx| tab.open_image(path, cx)).ok();
                    }
                    Err(error) => this.shadow_error = Some((partition_id, error.message)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle_mount(
        &mut self,
        node_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.contains(&node_id) {
            return;
        }
        self.busy.insert(node_id.clone());
        if let Some(view) = self.mount_for(&node_id).cloned() {
            let unmounting = Backend::global(cx).run(move |_, state| async move {
                mount_commands::fs_unmount_readonly(State::new(&state), view.id).await
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = unmounting.await;
                this.update_in(cx, |this, window, cx| {
                    this.busy.remove(&node_id);
                    match result {
                        Ok(()) => ui::notify_success(
                            window,
                            cx,
                            "卸载成功",
                            format!("{} 已释放", view.mount_point),
                        ),
                        Err(error) => ui::notify_error(window, cx, "卸载失败", error.message),
                    }
                    this.refresh_mounts(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
            cx.notify();
            return;
        }
        let listing = Backend::global(cx).run(|_, state| async move {
            mount_commands::fs_available_mount_points(State::new(&state)).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let drives = listing.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&node_id);
                match drives {
                    Ok(drives) => this.open_mount_picker(node_id, title, drives, window, cx),
                    Err(error) => ui::notify_error(window, cx, "挂载失败", error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_mount_picker(
        &mut self,
        node_id: String,
        title: String,
        drives: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let card = cx.entity().downgrade();
        window.open_dialog(cx, move |shell, _, cx| {
            let muted = cx.theme().muted_foreground;
            let folder_card = card.clone();
            let (folder_node, folder_title) = (node_id.clone(), title.clone());
            shell.title(format!("挂载 {title}")).w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().text_color(muted).child(
                        "以只读方式挂载为 Windows 盘符，或挂载到一个位于 NTFS 卷上的空文件夹。",
                    ))
                    .child(ui::section_title("盘符", cx))
                    .when(drives.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("没有可用盘符（H–Y 全部占用）"),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .children(drives.iter().enumerate().map(|(index, drive)| {
                                let (card, node_id, title, drive) =
                                    (card.clone(), node_id.clone(), title.clone(), drive.clone());
                                Button::new(("mount-drive", index))
                                    .small()
                                    .label(drive.clone())
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        card.update(cx, |card, cx| {
                                            card.mount_at(
                                                node_id.clone(),
                                                title.clone(),
                                                drive.clone(),
                                                window,
                                                cx,
                                            )
                                        })
                                        .ok();
                                    })
                            })),
                    )
                    .child(ui::section_title("文件夹", cx))
                    .child(
                        Button::new("mount-folder")
                            .small()
                            .icon(IconName::FolderOpen)
                            .label("选择空文件夹…")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let (node_id, title) = (folder_node.clone(), folder_title.clone());
                                folder_card
                                    .update(cx, |card, cx| {
                                        card.mount_to_folder(node_id, title, window, cx)
                                    })
                                    .ok();
                            }),
                    ),
            )
        });
    }

    fn mount_to_folder(
        &mut self,
        node_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = ui::prompt_directory("选择挂载目录（须为空且位于 NTFS 卷）", cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(folder) = prompt.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.mount_at(node_id, title, folder.display().to_string(), window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn mount_at(
        &mut self,
        node_id: String,
        title: String,
        mount_point: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.busy.insert(node_id.clone()) {
            return;
        }
        let (image_id, node) = (self.info.id.clone(), node_id.clone());
        let mounting = Backend::global(cx).run(move |_, state| async move {
            let view =
                mount_commands::fs_mount_readonly(State::new(&state), image_id, node, mount_point)
                    .await?;
            serde_json::to_value(view).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let mounted = mounting.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&node_id);
                match mounted {
                    Ok(view) => ui::notify_success(
                        window,
                        cx,
                        "挂载成功",
                        format!(
                            "{title} → {}",
                            view["mount_point"].as_str().unwrap_or_default()
                        ),
                    ),
                    Err(error) => ui::notify_error(window, cx, "挂载失败", error.message),
                }
                this.refresh_mounts(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle_share(
        &mut self,
        view: MountView,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("share:{}", view.id);
        if !self.busy.insert(key.clone()) {
            return;
        }
        let shared = view.share.clone();
        let share_name = share_name_for(&title, &view.id);
        let (mount_id, unsharing) = (view.id.clone(), shared.is_some());
        let toggling = Backend::global(cx).run(move |_, state| async move {
            if unsharing {
                mount_commands::fs_unshare_mount(State::new(&state), mount_id).await?;
                return Ok(None);
            }
            let share =
                mount_commands::fs_share_mount(State::new(&state), mount_id, Some(share_name))
                    .await?;
            Ok(serde_json::to_value(share).ok())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = toggling.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&key);
                match (result, shared) {
                    (Ok(_), Some(share)) => {
                        ui::notify_success(window, cx, "共享已关闭", share.unc_path)
                    }
                    (Ok(share), None) => {
                        let unc = share
                            .as_ref()
                            .and_then(|share| share["unc_path"].as_str())
                            .unwrap_or_default()
                            .to_owned();
                        cx.write_to_clipboard(ClipboardItem::new_string(unc.clone()));
                        ui::notify_success(window, cx, "共享成功", format!("{unc} 已复制"));
                    }
                    (Err(error), shared) => ui::notify_error(
                        window,
                        cx,
                        if shared.is_some() {
                            "取消共享失败"
                        } else {
                            "共享失败"
                        },
                        error.message,
                    ),
                }
                this.refresh_mounts(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_mount_controls(
        &self,
        node_id: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = self.mount_for(node_id).cloned();
        let busy = self.busy.contains(node_id);
        let id = SharedString::from(format!("mount-{node_id}"));
        let (toggle_node, toggle_title) = (node_id.to_owned(), title.to_owned());
        h_flex()
            .gap_1()
            .child(
                Button::new(id)
                    .small()
                    .when(view.is_some(), |button| button.outline())
                    .when(view.is_none(), |button| button.ghost())
                    .icon(Icon::new(ExtraIcon::Download))
                    .loading(busy)
                    .label(
                        view.as_ref()
                            .map(|view| view.mount_point.clone())
                            .unwrap_or_else(|| "挂载".to_owned()),
                    )
                    .tooltip(match &view {
                        Some(view) => format!("已挂载到 {}，点击卸载", view.mount_point),
                        None => "只读挂载为盘符或文件夹".to_owned(),
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_mount(toggle_node.clone(), toggle_title.clone(), window, cx)
                    })),
            )
            .when_some(view, |this, view| {
                let share_busy = self.busy.contains(&format!("share:{}", view.id));
                let unc = view.share.as_ref().map(|share| share.unc_path.clone());
                let share_title = title.to_owned();
                let share_view = view.clone();
                this.child(
                    Button::new(SharedString::from(format!("share-{}", view.id)))
                        .small()
                        .ghost()
                        .loading(share_busy)
                        .label(if view.share.is_some() {
                            "共享中"
                        } else {
                            "共享"
                        })
                        .tooltip(match &unc {
                            Some(unc) => format!("已共享为 {unc}，点击取消共享"),
                            None => "把该只读挂载发布为局域网共享".to_owned(),
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_share(share_view.clone(), share_title.clone(), window, cx)
                        })),
                )
                .when_some(unc, |this, unc| {
                    this.child(
                        Button::new(SharedString::from(format!("copy-share-{}", view.id)))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip("复制共享名")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(unc.clone()))
                            }),
                    )
                })
            })
    }

    fn render_shadow_toggle(
        &self,
        partition_id: u8,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let Shadows::Loaded(volumes) = &self.shadows else {
            return None;
        };
        let volume = volumes
            .iter()
            .find(|volume| volume.partition_id == partition_id)?;
        if volume.error.is_none() && volume.shadow_copies.is_empty() {
            return Some(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("无卷影副本")
                    .into_any_element(),
            );
        }
        let expanded = self.expanded.contains(&partition_id);
        Some(
            Button::new(("shadow-toggle", usize::from(partition_id)))
                .small()
                .ghost()
                .when(volume.error.is_some(), |button| button.danger())
                .icon(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .label(match &volume.error {
                    Some(_) => "卷影副本读取失败".to_owned(),
                    None => format!("卷影副本 {}", volume.shadow_copies.len()),
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded.remove(&partition_id) {
                        this.expanded.insert(partition_id);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    fn render_shadow_list(
        &self,
        partition_id: u8,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        if !self.expanded.contains(&partition_id) {
            return None;
        }
        let Shadows::Loaded(volumes) = &self.shadows else {
            return None;
        };
        let volume = volumes
            .iter()
            .find(|volume| volume.partition_id == partition_id)?;
        let muted = cx.theme().muted_foreground;
        let opening = self.shadow_opening;
        let list = match &volume.error {
            Some(error) => {
                ui::error_text(format!("VSS 元数据无法读取：{error}"), cx).into_any_element()
            }
            None => v_flex()
                .gap_1()
                .children(volume.shadow_copies.iter().map(|copy| {
                    let time = copy
                        .created_unix_ms
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                        .unwrap_or_else(|| "—".to_owned());
                    let this_opening = opening == Some((partition_id, copy.index));
                    let open_copy = copy.clone();
                    h_flex()
                        .gap_3()
                        .py_1()
                        .child(
                            ui::mono(format!("#{}", copy.index + 1), cx)
                                .text_xs()
                                .w(px(36.)),
                        )
                        .child(ui::mono(time, cx).text_xs().w(px(168.)))
                        .child(
                            h_flex()
                                .gap_1()
                                .when(copy.persistent, |this| {
                                    this.child(Tag::secondary().xsmall().child("持久"))
                                })
                                .when(copy.client_accessible, |this| {
                                    this.child(Tag::secondary().xsmall().child("客户端可访问"))
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(
                                    copy.operating_machine
                                        .clone()
                                        .or_else(|| copy.shadow_copy_id.clone())
                                        .unwrap_or_else(|| copy.store_id.clone()),
                                ),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "open-shadow-{partition_id}-{}",
                                copy.index
                            )))
                            .small()
                            .label(if this_opening {
                                "正在重建…"
                            } else if copy.open_image_id.is_some() {
                                "切换到"
                            } else {
                                "打开"
                            })
                            .loading(this_opening)
                            .disabled(opening.is_some() || copy.unavailable_reason.is_some())
                            .tooltip(copy.unavailable_reason.clone().unwrap_or_else(|| {
                                if copy.open_image_id.is_some() {
                                    "切换到已打开的卷影副本".to_owned()
                                } else {
                                    "重建为只读镜像并在新标签页打开".to_owned()
                                }
                            }))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.open_shadow(partition_id, open_copy.clone(), cx)
                                },
                            )),
                        )
                }))
                .into_any_element(),
        };
        Some(
            v_flex()
                .w_full()
                .pl(px(28.))
                .pr_2()
                .pb_2()
                .gap_1()
                .child(list)
                .when_some(
                    self.shadow_error
                        .as_ref()
                        .filter(|(partition, _)| *partition == partition_id)
                        .map(|(_, error)| error.clone()),
                    |this, error| {
                        this.child(
                            ui::error_text(format!("打开卷影副本失败：{error}"), cx).text_xs(),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    fn render_actions(&self, partition: &Partition, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = wire_name(&partition.kind);
        let block = supports_block_operations(&self.info);
        let partition_id = partition.id;
        let title = partition_title(partition);
        let mut actions = h_flex().gap_1().justify_end().flex_wrap();
        if let Some(toggle) = self.render_shadow_toggle(partition_id, cx) {
            actions = actions.child(toggle);
        }
        let status = self.bitlocker.get(&partition_id);
        if kind == "bitlocker" && block {
            let locked = status.is_none_or(|status| status.locked);
            let running = self
                .dialogs
                .get(&partition_id)
                .is_some_and(|dialog| dialog.read(cx).is_running());
            let progress = self
                .dialogs
                .get(&partition_id)
                .and_then(|dialog| dialog.read(cx).progress(cx));
            actions = actions.child(
                if locked {
                    Tag::secondary()
                } else {
                    Tag::info()
                }
                .xsmall()
                .child(if locked { "LOCKED" } else { "UNLOCKED" }),
            );
            actions = if running {
                actions.child(
                    Button::new(("bitlocker-progress", usize::from(partition_id)))
                        .small()
                        .ghost()
                        .loading(true)
                        .label(match progress {
                            Some(percent) => format!("解锁中 {percent:.1}%"),
                            None => "解锁中".to_owned(),
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_bitlocker(partition_id, window, cx)
                        })),
                )
            } else if locked {
                actions.child(
                    Button::new(("bitlocker-unlock", usize::from(partition_id)))
                        .small()
                        .icon(Icon::new(ExtraIcon::KeyRound))
                        .label("解锁")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_bitlocker(partition_id, window, cx)
                        })),
                )
            } else {
                let exporting = self.exporting.contains(&partition_id);
                actions.child(
                    Button::new(("bitlocker-export", usize::from(partition_id)))
                        .small()
                        .ghost()
                        .icon(Icon::new(ExtraIcon::Download))
                        .loading(exporting)
                        .label(if exporting { "导出中" } else { "导出解密镜像" })
                        .tooltip(match status.and_then(|status| status.encryption.clone()) {
                            Some(method) => format!("导出完整 RAW 镜像，并将此 BitLocker 分区（{method}）替换为解密后内容"),
                            None => "导出完整 RAW 镜像，并将此 BitLocker 分区替换为解密后内容".to_owned(),
                        })
                        .on_click(cx.listener(move |this, _, window, cx| this.export_decrypted(partition_id, window, cx))),
                )
            };
        }
        if (kind == "luks" || kind == "filevault")
            && block
            && !self.info.unlocked_partitions.contains(&partition_id)
        {
            actions = actions.child(
                Button::new(("container-unlock", usize::from(partition_id)))
                    .small()
                    .icon(Icon::new(ExtraIcon::KeyRound))
                    .label("解锁")
                    .tooltip("解锁该加密分区并以只读方式浏览文件")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_container(Some(partition_id), window, cx)
                    })),
            );
        }
        let mount_ready = kind != "bitlocker" || status.is_some_and(|status| !status.locked);
        if MOUNTABLE_KINDS.contains(&kind.as_str()) && mount_ready && !self.info.index_only {
            actions = actions.child(self.render_mount_controls(
                &format!("part:{partition_id}"),
                &title,
                cx,
            ));
        }
        actions
    }
}

/// `分区N` or `分区N · 标签`.
fn partition_title(partition: &Partition) -> String {
    match &partition.label {
        Some(label) if !label.is_empty() => format!("分区{} · {label}", partition.id),
        _ => format!("分区{}", partition.id),
    }
}

impl Render for VolumesCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info = Rc::clone(&self.info);
        let sector = info.sector_size.max(1) as u64;
        let total_sectors = (info.size_bytes / sector).max(1);
        let palette: [Hsla; 5] = [
            cx.theme().chart_1,
            cx.theme().chart_2,
            cx.theme().chart_3,
            cx.theme().chart_4,
            cx.theme().chart_5,
        ];
        let muted = cx.theme().muted_foreground;
        let scheme = info
            .table_scheme
            .map(|scheme| wire_name(&scheme).to_uppercase());
        let block = supports_block_operations(&info);
        let title =
            h_flex()
                .w_full()
                .gap_2()
                .child(Icon::new(ExtraIcon::Layers))
                .child("卷与分区")
                .when_some(scheme, |this, scheme| {
                    this.child(Tag::secondary().small().child(scheme))
                })
                .child(
                    Tag::secondary()
                        .small()
                        .child(info.partitions.len().to_string()),
                )
                .child(div().flex_1())
                .when(block, |this| {
                    this.child(
                        Button::new("unlock-container")
                            .small()
                            .ghost()
                            .icon(Icon::new(ExtraIcon::Lock))
                            .label("解锁加密容器")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_container(None, window, cx)
                            })),
                    )
                });
        let header_row = h_flex()
            .w_full()
            .gap_3()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(muted)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().min_w_0().child("名称"))
            .child(div().w(px(150.)).child("文件系统"))
            .child(div().w(px(110.)).text_right().child("起始扇区"))
            .child(div().w(px(96.)).text_right().child("大小"))
            .child(div().w(px(300.)).text_right().child("操作"));
        let rows: Vec<_> = info
            .partitions
            .iter()
            .enumerate()
            .map(|(index, partition)| {
                let unlocked = info.unlocked_partitions.contains(&partition.id)
                    || self
                        .bitlocker
                        .get(&partition.id)
                        .is_some_and(|status| !status.locked);
                v_flex()
                    .w_full()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .px_2()
                            .py_1p5()
                            .text_sm()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .size_2()
                                            .rounded_full()
                                            .bg(palette[index % palette.len()]),
                                    )
                                    .child(div().truncate().child(partition_title(partition))),
                            )
                            .child(
                                h_flex()
                                    .w(px(150.))
                                    .gap_1()
                                    .child(
                                        Tag::secondary()
                                            .small()
                                            .child(filesystem_label(&partition.kind)),
                                    )
                                    .when(unlocked, |this| {
                                        this.child(Tag::success().xsmall().child("已解锁"))
                                    }),
                            )
                            .child(
                                ui::mono(format::count(partition.start_lba), cx)
                                    .w(px(110.))
                                    .text_right(),
                            )
                            .child(
                                ui::mono(
                                    format::binary_bytes(
                                        partition.length_lba.saturating_mul(sector),
                                    ),
                                    cx,
                                )
                                .w(px(96.))
                                .text_right(),
                            )
                            .child(div().w(px(300.)).child(self.render_actions(partition, cx))),
                    )
                    .children(self.render_shadow_list(partition.id, cx))
            })
            .collect();
        let synthetic = if info.partitions.is_empty() && !info.index_only {
            synthetic_root(&info)
        } else {
            None
        };
        GroupBox::new()
            .id("volumes")
            .outline()
            .title(title)
            .when(info.partitions.is_empty(), |this| {
                this.child(match synthetic {
                    Some((node_id, label)) => h_flex()
                        .w_full()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().child(label))
                                .child(div().text_xs().text_color(muted).child(
                                    "逻辑检材以单一合成根呈现（无分区表），可只读挂载为 Windows 盘符或文件夹后用资源管理器浏览。",
                                )),
                        )
                        .child(self.render_mount_controls(node_id, label, cx))
                        .into_any_element(),
                    None => div()
                        .text_sm()
                        .text_color(muted)
                        .child(if info.index_only {
                            "离线索引会话只保留已发布的文件索引，不读取分区表。"
                        } else {
                            "未检测到分区表；文件列表按整盘卷或逻辑检材浏览。"
                        })
                        .into_any_element(),
                })
            })
            .when(!info.partitions.is_empty(), |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .h_6()
                        .gap_px()
                        .rounded(cx.theme().radius)
                        .overflow_hidden()
                        .bg(cx.theme().muted)
                        .children(info.partitions.iter().enumerate().map(|(index, partition)| {
                            let fraction = (partition.length_lba as f64 / total_sectors as f64).clamp(0.004, 1.0);
                            div().h_full().w(relative(fraction as f32)).bg(palette[index % palette.len()])
                        })),
                )
                .child(v_flex().w_full().child(header_row).children(rows))
            })
            .when_some(
                match &self.shadows {
                    Shadows::Failed(error) => Some(error.clone()),
                    _ => None,
                },
                |this, error| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .child(ui::error_text(format!("读取卷影副本失败：{error}"), cx).text_xs())
                            .child(
                                Button::new("retry-shadows")
                                    .xsmall()
                                    .label("重试")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_shadows(cx))),
                            ),
                    )
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::share_name_for;

    #[test]
    fn share_names_are_windows_safe() {
        assert_eq!(
            share_name_for("分区1 · Data: x/y", "1234567890"),
            "LC_分区1_·_Data_x_y_12345678"
        );
        assert_eq!(share_name_for("  ", "abc"), "LC_mount_abc");
    }
}
