//! RAID 检测: LVM volume groups and md / firmware RAID arrays across every
//! loaded image. Detection is session-wide (members span images), so one
//! store holds the result and each evidence page shows the arrays its image
//! takes part in.
mod builder;

use std::collections::BTreeSet;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        group_box::{GroupBox, GroupBoxVariants as _},
        h_flex,
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::{app_error::AppError, evidence::image as image_commands},
    core::evidence::image::ImageInfo,
    host::State,
};
use serde::Deserialize;

use crate::{backend::Backend, evidence::EvidenceTab, format, ui};
pub use builder::ManualBuilder;

type ExtraIcon = gpui_kit::assets::IconName;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Stack {
    pub images: Vec<StackImage>,
    pub volume_groups: Vec<VolumeGroup>,
    pub arrays: Vec<Array>,
    #[serde(default)]
    pub filesystems: Vec<ArrayFilesystem>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StackImage {
    pub image_id: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VolumeGroup {
    pub vg_uuid: String,
    pub vg_name: String,
    pub members: Vec<VgMember>,
    pub logical_volumes: Vec<LogicalVolume>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VgMember {
    pub image_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LogicalVolume {
    pub name: String,
    pub size_bytes: u64,
    pub assembled_image_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Array {
    pub set_uuid: String,
    pub set_name: String,
    pub level: i32,
    pub layout: u32,
    pub chunksize_sectors: u64,
    pub raid_disks: u32,
    pub data_offset_sectors: u64,
    pub assembled_bytes: u64,
    pub members: Vec<Member>,
    pub assembled_image_id: Option<String>,
    pub heuristic: bool,
    pub metadata_version: String,
    pub degraded: bool,
    pub degradable: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Member {
    pub slot: Option<u32>,
    pub state: String,
    pub reconstructed: bool,
    pub origin: Origin,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Lv {
        vg_name: String,
        lv_name: String,
        vg_uuid: String,
    },
    Whole {
        image_id: String,
        partition_id: u8,
        byte_offset: u64,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct ArrayFilesystem {
    pub set_uuid: String,
    pub kind: String,
}

/// `Linear`, `RAID 5`, …
pub fn level_short(level: i32) -> String {
    if level == -1 {
        "Linear".to_owned()
    } else {
        format!("RAID {level}")
    }
}

fn level_name(level: i32) -> String {
    match level {
        -1 => "线性（JBOD）".to_owned(),
        0 => "RAID 0（条带）".to_owned(),
        1 => "RAID 1（镜像）".to_owned(),
        4 => "RAID 4（专用校验）".to_owned(),
        5 => "RAID 5（单校验）".to_owned(),
        6 => "RAID 6（双校验）".to_owned(),
        10 => "RAID 10（条带镜像）".to_owned(),
        other => format!("RAID {other}"),
    }
}

/// md RAID5 / RAID6 parity rotations, named as domestic rebuild tools do
/// (异步 = asymmetric, 同步 = symmetric).
pub fn parity_layout_name(layout: u32) -> String {
    match layout {
        0 => "左异步",
        1 => "右异步",
        2 => "左同步",
        3 => "右同步",
        4 => "首盘校验",
        5 => "末盘校验",
        8 => "DDF 零起轮转",
        9 => "DDF 末盘起轮转",
        10 => "DDF 连续轮转",
        16 => "左异步（Q 固定末盘）",
        17 => "右异步（Q 固定末盘）",
        18 => "左同步（Q 固定末盘）",
        19 => "右同步（Q 固定末盘）",
        20 => "首盘校验（Q 固定末盘）",
        other => return format!("布局 #{other}"),
    }
    .to_owned()
}

/// RAID10 copies: `near | far << 8 | offset << 16`.
pub fn raid10_layout_name(layout: u32) -> String {
    let (near, far, offset) = (layout & 0xff, (layout >> 8) & 0xff, layout & (1 << 16) != 0);
    let mut parts = Vec::new();
    if near > 1 {
        parts.push(format!("近端 {near} 份"));
    }
    if far > 1 {
        parts.push(if offset {
            format!("偏移 {far} 份")
        } else {
            format!("远端 {far} 份")
        });
    }
    if parts.is_empty() {
        format!("布局 #{layout}")
    } else {
        parts.join(" · ")
    }
}

fn chunk_label(sectors: u64) -> String {
    let kib = sectors / 2;
    if kib >= 1024 && kib.is_multiple_of(1024) {
        format!("{} MiB", kib / 1024)
    } else {
        format!("{kib} KiB")
    }
}

/// Members read from their disk: active, in a slot, not rebuilt.
fn active_members(array: &Array) -> usize {
    array
        .members
        .iter()
        .filter(|member| member.slot.is_some() && member.state == "active" && !member.reconstructed)
        .count()
}

/// The array's state in words, and whether it is a warning.
fn array_status(array: &Array) -> (String, bool) {
    let active = active_members(array);
    let disks = array.raid_disks as usize;
    if array.assembled_image_id.is_some() {
        return if array.degraded {
            ("已降级重组".to_owned(), true)
        } else {
            ("已重组".to_owned(), false)
        };
    }
    if array.degradable {
        return (
            format!("缺 {} 块，可降级重组", disks.saturating_sub(active)),
            true,
        );
    }
    if array
        .members
        .iter()
        .any(|member| member.state == "unidentified")
    {
        return ("成员顺序未知，见诊断".to_owned(), true);
    }
    if active < disks {
        return (format!("成员不全 {active}/{disks}"), true);
    }
    if ![-1, 0, 1, 4, 5, 6, 10].contains(&array.level) {
        return (format!("RAID {} 暂不支持重组", array.level), false);
    }
    ("未能重组，见诊断".to_owned(), true)
}

fn member_state_label(state: &str) -> Option<&'static str> {
    Some(match state {
        "spare" => "备用",
        "faulty" => "故障",
        "journal" => "日志盘",
        "stale" => "陈旧",
        "rebuilding" => "重建中",
        "duplicate" => "重复加载",
        "unidentified" => "未匹配",
        _ => return None,
    })
}

/// Arrays this image takes part in: a whole-disk member lives on it, an LV
/// member's volume group has a PV on it, or it is the assembled device.
fn relevant_arrays<'a>(stack: &'a Stack, image_id: &str) -> Vec<&'a Array> {
    let volume_groups: BTreeSet<&str> = stack
        .volume_groups
        .iter()
        .filter(|group| {
            group
                .members
                .iter()
                .any(|member| member.image_id == image_id)
        })
        .map(|group| group.vg_uuid.as_str())
        .collect();
    stack
        .arrays
        .iter()
        .filter(|array| {
            array.assembled_image_id.as_deref() == Some(image_id)
                || array.members.iter().any(|member| match &member.origin {
                    Origin::Whole {
                        image_id: member_image,
                        ..
                    } => member_image == image_id,
                    Origin::Lv { vg_uuid, .. } => volume_groups.contains(vg_uuid.as_str()),
                })
        })
        .collect()
}

fn relevant_groups<'a>(stack: &'a Stack, image_id: &str) -> Vec<&'a VolumeGroup> {
    stack
        .volume_groups
        .iter()
        .filter(|group| {
            group
                .members
                .iter()
                .any(|member| member.image_id == image_id)
                || group
                    .logical_volumes
                    .iter()
                    .any(|volume| volume.assembled_image_id.as_deref() == Some(image_id))
        })
        .collect()
}

/// A RAW file name for an assembled array, safe on Windows.
fn export_name(array: &Array) -> String {
    let raw = if array.set_name.is_empty() {
        format!("md-{}", array.set_uuid.chars().take(12).collect::<String>())
    } else {
        array.set_name.clone()
    };
    let safe: String = raw
        .chars()
        .map(|character| {
            if "\\/:*?\"<>|".contains(character) || character.is_whitespace() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let level = if array.level == -1 {
        "linear".to_owned()
    } else {
        format!("RAID{}", array.level)
    };
    format!(
        "{}_{level}.dd",
        if safe.is_empty() {
            "raid".to_owned()
        } else {
            safe
        }
    )
}

/// The session-wide detection result.
pub struct RaidStore {
    stack: Option<Stack>,
    loading: bool,
    error: Option<SharedString>,
    /// md sets the examiner chose to assemble without their missing members;
    /// every later run keeps the choice.
    degraded: BTreeSet<String>,
    generation: u64,
}

struct RaidStoreGlobal(Entity<RaidStore>);

impl Global for RaidStoreGlobal {}

impl RaidStore {
    pub fn install(cx: &mut App) {
        let store = cx.new(|_| Self {
            stack: None,
            loading: false,
            error: None,
            degraded: BTreeSet::new(),
            generation: 0,
        });
        cx.set_global(RaidStoreGlobal(store));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<RaidStoreGlobal>().0.clone()
    }

    pub fn stack(&self) -> Option<&Stack> {
        self.stack.as_ref()
    }

    /// Run LVM → md → filesystem detection over every loaded image.
    pub fn detect(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let degraded: Vec<String> = self.degraded.iter().cloned().collect();
        let detecting = Backend::global(cx).run(move |host, state| async move {
            let stack =
                image_commands::raid_auto_detect(host, State::new(&state), Some(degraded), None)
                    .await?;
            serde_json::to_value(stack).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let detected = detecting.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match detected.map(serde_json::from_value::<Stack>) {
                    Ok(Ok(stack)) => this.stack = Some(stack),
                    Ok(Err(error)) => this.error = Some(error.to_string().into()),
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn assemble_degraded(&mut self, set_uuid: String, cx: &mut Context<Self>) {
        self.degraded.insert(set_uuid);
        self.detect(cx);
    }

    /// A different case is open: forget its arrays and choices.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.stack = None;
        self.loading = false;
        self.error = None;
        self.degraded.clear();
        cx.notify();
    }
}

/// Open a registered synthetic image (an assembled array or volume) by id.
pub fn open_synthetic(
    image_id: String,
    tab: WeakEntity<EvidenceTab>,
    window: &mut Window,
    cx: &mut App,
) {
    let resolving = Backend::global(cx).run(move |_, state| async move {
        image_commands::image_info(State::new(&state), image_id).await
    });
    window
        .spawn(cx, async move |cx| {
            let resolved = resolving.await;
            cx.update(|window, cx| match resolved {
                Ok(info) => {
                    tab.update(cx, |tab, cx| tab.open_image(info.path, cx)).ok();
                }
                Err(error) => ui::notify_error(window, cx, "打开重组镜像失败", error.message),
            })
            .ok();
        })
        .detach();
}

pub struct RaidPanel {
    info: std::rc::Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    store: Entity<RaidStore>,
    exporting: BTreeSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl RaidPanel {
    pub fn new(
        info: std::rc::Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = RaidStore::global(cx);
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        // A newly loaded image can complete an array: detect again.
        store.update(cx, |store, cx| store.detect(cx));
        Self {
            info,
            tab,
            store,
            exporting: BTreeSet::new(),
            _subscriptions: vec![subscription],
        }
    }

    fn hinted(&self) -> bool {
        let scheme = self
            .info
            .table_scheme
            .map(|scheme| crate::evidence::wire_name(&scheme));
        matches!(scheme.as_deref(), Some("mdadm" | "lvm2"))
            || self.info.partitions.iter().any(|partition| {
                matches!(
                    crate::evidence::wire_name(&partition.kind).as_str(),
                    "mdadm" | "lvm"
                )
            })
            || self.info.partitions.is_empty()
    }

    fn export(&mut self, array: Array, window: &mut Window, cx: &mut Context<Self>) {
        let Some(image_id) = array.assembled_image_id.clone() else {
            return;
        };
        let prompt = ui::prompt_save(&export_name(&array), cx);
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(destination) = prompt.await else {
                return;
            };
            let shown = destination.display().to_string();
            this.update(cx, |this, cx| {
                this.exporting.insert(image_id.clone());
                cx.notify();
            })
            .ok();
            let source = image_id.clone();
            let exported = backend
                .run(move |host, state| async move {
                    image_commands::image_export_raw(
                        host,
                        State::new(&state),
                        source,
                        destination.display().to_string(),
                        uuid::Uuid::new_v4().to_string(),
                        None,
                        None,
                    )
                    .await
                    .map(|_| ())
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.exporting.remove(&image_id);
                match exported {
                    Ok(()) => ui::notify_success(window, cx, "重组镜像已导出", shown),
                    Err(error) if error.cancelled => {}
                    Err(error) => ui::notify_error(window, cx, "导出重组镜像失败", error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_builder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let images = self
            .store
            .read(cx)
            .stack()
            .map(|stack| stack.images.clone())
            .unwrap_or_default();
        let tab = self.tab.clone();
        let builder = cx.new(|cx| ManualBuilder::new(images, tab, window, cx));
        window.open_dialog(cx, move |shell, _, _| {
            shell
                .title("手动重组 RAID")
                .w(px(640.))
                .child(builder.clone())
        });
    }

    fn render_array(
        &self,
        array: &Array,
        stack: &Stack,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (status, warn) = array_status(array);
        let name = |image_id: &str| {
            stack
                .images
                .iter()
                .find(|image| image.image_id == image_id)
                .map(|image| format::file_name(&image.path).to_owned())
                .unwrap_or_else(|| image_id.chars().take(8).collect())
        };
        let mut slots: Vec<Option<&Member>> = vec![None; array.raid_disks as usize];
        for member in &array.members {
            if let Some(slot) = member.slot.filter(|_| member.state != "duplicate") {
                if let Some(entry) = slots.get_mut(slot as usize) {
                    *entry = Some(member);
                }
            }
        }
        let extra: Vec<&Member> = array
            .members
            .iter()
            .filter(|member| member.slot.is_none() || member.state == "duplicate")
            .collect();
        let filesystem = stack
            .filesystems
            .iter()
            .find(|candidate| candidate.set_uuid == array.set_uuid)
            .map(|candidate| candidate.kind.clone());
        let is_current = array.assembled_image_id.as_deref() == Some(self.info.id.as_str());
        let exporting = array
            .assembled_image_id
            .as_ref()
            .is_some_and(|image| self.exporting.contains(image));
        let loading = self.store.read(cx).loading;
        let member_label = |member: &Member| match &member.origin {
            Origin::Lv { lv_name, .. } => lv_name.clone(),
            Origin::Whole { image_id, .. } => name(image_id),
        };
        let member_title = |member: &Member| match &member.origin {
            Origin::Lv {
                vg_name,
                lv_name,
                vg_uuid,
            } => format!("逻辑卷 {vg_name}/{lv_name}（VG {vg_uuid}）"),
            Origin::Whole {
                image_id,
                partition_id,
                byte_offset,
            } => {
                format!(
                    "{} · 分区 {partition_id} · 偏移 0x{byte_offset:X}",
                    name(image_id)
                )
            }
        };
        let open_array = array.clone();
        let export_array = array.clone();
        let degrade_uuid = array.set_uuid.clone();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(level_short(array.level)),
                    )
                    .when(!array.set_name.is_empty(), |this| {
                        this.child(div().text_sm().child(array.set_name.clone()))
                    })
                    .child(
                        if warn { Tag::warning() } else { Tag::success() }
                            .small()
                            .child(status),
                    )
                    .when(array.heuristic, |this| {
                        this.child(Tag::secondary().small().child("参数推定"))
                    })
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{} / {} 成员盘",
                        active_members(array),
                        array.raid_disks
                    )))
                    .child(div().flex_1())
                    .when(array.assembled_image_id.is_some() && !is_current, |this| {
                        this.child(
                            Button::new(SharedString::from(format!(
                                "export-array-{}",
                                array.set_uuid
                            )))
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .loading(exporting)
                            .label("导出重组镜像")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.export(export_array.clone(), window, cx)
                                },
                            )),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "open-array-{}",
                                array.set_uuid
                            )))
                            .small()
                            .primary()
                            .icon(IconName::ExternalLink)
                            .label("打开重组镜像")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if let Some(image) = open_array.assembled_image_id.clone() {
                                        open_synthetic(image, this.tab.clone(), window, cx);
                                    }
                                },
                            )),
                        )
                    })
                    .when(
                        array.assembled_image_id.is_none() && array.degradable,
                        |this| {
                            this.child(
                                Button::new(SharedString::from(format!(
                                    "degrade-array-{}",
                                    array.set_uuid
                                )))
                                .small()
                                .icon(Icon::new(ExtraIcon::Wrench))
                                .loading(loading)
                                .disabled(loading)
                                .label("降级重组")
                                .tooltip("不等待缺失成员，用其余成员的校验或镜像数据重组")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let uuid = degrade_uuid.clone();
                                        this.store.update(cx, |store, cx| {
                                            store.assemble_degraded(uuid, cx)
                                        });
                                    },
                                )),
                            )
                        },
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(slots.iter().enumerate().map(|(slot, member)| {
                        h_flex()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted)
                            .text_xs()
                            .child(ui::mono(slot.to_string(), cx).text_color(muted))
                            .map(|this| match member {
                                Some(member) => this
                                    .child(div().child(member_label(member)))
                                    .when(member.reconstructed, |this| {
                                        this.child(Tag::warning().xsmall().child("校验重建"))
                                    })
                                    .when_some(
                                        member_state_label(&member.state)
                                            .filter(|_| !member.reconstructed),
                                        |this, label| {
                                            this.child(Tag::warning().xsmall().child(label))
                                        },
                                    ),
                                None => this
                                    .child(div().text_color(cx.theme().danger).child("缺失成员")),
                            })
                    })),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .children(slots.iter().enumerate().filter_map(|(slot, member)| {
                        member.map(|member| {
                            ui::mono(format!("列 {slot} · {}", member_title(member)), cx)
                                .text_xs()
                                .text_color(muted)
                        })
                    })),
            )
            .when(!extra.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children(extra.iter().map(|member| {
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(muted)
                                .child(member_label(member))
                                .when_some(member_state_label(&member.state), |this, label| {
                                    this.child(Tag::secondary().xsmall().child(label))
                                })
                        })),
                )
            })
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .text_xs()
                    .text_color(muted)
                    .when(array.heuristic, |this| {
                        this.child("无 RAID 元数据，参数经文件系统校验推定")
                    })
                    .child(format!("级别 {}", level_name(array.level)))
                    .when(array.level != 1 && array.chunksize_sectors > 0, |this| {
                        this.child(format!("块大小 {}", chunk_label(array.chunksize_sectors)))
                    })
                    .when(matches!(array.level, 5 | 6), |this| {
                        this.child(format!("校验布局 {}", parity_layout_name(array.layout)))
                    })
                    .when(array.level == 10, |this| {
                        this.child(format!("副本布局 {}", raid10_layout_name(array.layout)))
                    })
                    .child(format!(
                        "数据偏移 {} 扇区",
                        format::count(array.data_offset_sectors)
                    ))
                    .when(!array.metadata_version.is_empty(), |this| {
                        this.child(format!("元数据 {}", array.metadata_version))
                    })
                    .when(array.assembled_bytes > 0, |this| {
                        this.child(format!(
                            "容量 {}",
                            format::binary_bytes(array.assembled_bytes)
                        ))
                    })
                    .when_some(filesystem, |this, kind| {
                        this.child(format!("文件系统 {kind}"))
                    }),
            )
    }

    fn render_group(&self, group: &VolumeGroup, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!("LVM 卷组 {}", group.vg_name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{} PV", group.members.len())),
                    ),
            )
            .children(
                group
                    .logical_volumes
                    .iter()
                    .enumerate()
                    .map(|(index, volume)| {
                        let image = volume.assembled_image_id.clone();
                        let current = image.as_deref() == Some(self.info.id.as_str());
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(div().flex_1().child(volume.name.clone()))
                            .child(ui::mono(format::binary_bytes(volume.size_bytes), cx).text_xs())
                            .map(|this| match image.filter(|_| !current) {
                                Some(image) => this.child(
                                    Button::new(SharedString::from(format!(
                                        "open-lv-{}-{index}",
                                        group.vg_uuid
                                    )))
                                    .xsmall()
                                    .label("打开")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        open_synthetic(image.clone(), this.tab.clone(), window, cx)
                                    })),
                                ),
                                None => this.child(div().text_xs().text_color(muted).child(
                                    if current {
                                        "当前镜像"
                                    } else {
                                        "不可重组"
                                    },
                                )),
                            })
                    }),
            )
    }
}

impl Render for RaidPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let (loading, error) = (store.loading, store.error.clone());
        let stack = store.stack.clone();
        let image_id = self.info.id.clone();
        let arrays: Vec<Array> = stack
            .as_ref()
            .map(|stack| {
                relevant_arrays(stack, &image_id)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let groups: Vec<VolumeGroup> = stack
            .as_ref()
            .map(|stack| {
                relevant_groups(stack, &image_id)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let findings = !arrays.is_empty() || !groups.is_empty();
        if !findings && !self.hinted() {
            return div().into_any_element();
        }
        let muted = cx.theme().muted_foreground;
        let assembled = wire_name_is(&self.info, "assembled");
        let title = h_flex()
            .w_full()
            .gap_2()
            .child(Icon::new(ExtraIcon::Server))
            .child("RAID 检测")
            .when(!arrays.is_empty(), |this| {
                this.child(
                    Tag::secondary()
                        .small()
                        .child(format!("{} 阵列", arrays.len())),
                )
            })
            .child(div().flex_1())
            .when(!assembled, |this| {
                this.child(
                    Button::new("raid-manual")
                        .small()
                        .ghost()
                        .icon(Icon::new(ExtraIcon::Wrench))
                        .label("手动重组")
                        .tooltip("手动指定成员盘、块大小和数据偏移")
                        .on_click(cx.listener(|this, _, window, cx| this.open_builder(window, cx))),
                )
                .child(
                    Button::new("raid-detect")
                        .small()
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .loading(loading)
                        .disabled(loading)
                        .tooltip("重新检测（扫描所有已加载镜像）")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.store.update(cx, |store, cx| store.detect(cx))
                        })),
                )
            });
        let stack_for_render = stack.clone().unwrap_or_default();
        let array_views: Vec<_> = arrays
            .iter()
            .map(|array| {
                self.render_array(array, &stack_for_render, cx)
                    .into_any_element()
            })
            .collect();
        let group_views: Vec<_> = groups
            .iter()
            .map(|group| self.render_group(group, cx).into_any_element())
            .collect();
        GroupBox::new()
            .id("raid")
            .outline()
            .title(title)
            .when(loading && stack.is_none(), |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(Spinner::new().small())
                        .child("正在扫描已加载镜像中的 LVM / mdadm 结构…"),
                )
            })
            .when_some(error, |this, error| this.child(ui::error_text(format!("检测失败：{error}"), cx)))
            .children(array_views)
            .children(group_views)
            .when(stack.is_some() && !findings, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("未检测到与该镜像相关的 LVM 卷组或 mdadm RAID 阵列。无分区表的磁盘可用「手动重组」指定几何参数。"),
                )
            })
            .when_some(stack.filter(|stack| !stack.diagnostics.is_empty()), |this, stack| {
                this.child(ui::section_title(format!("诊断信息 · {}", stack.diagnostics.len()), cx))
                    .children(stack.diagnostics.iter().take(20).map(|message| {
                        ui::mono(message.clone(), cx).text_xs().text_color(muted)
                    }))
            })
            .into_any_element()
    }
}

fn wire_name_is(info: &ImageInfo, format: &str) -> bool {
    crate::evidence::wire_name(&info.source_format) == format
}

#[cfg(test)]
mod tests {
    use super::{chunk_label, parity_layout_name, raid10_layout_name};

    #[test]
    fn layouts_and_chunks_read_as_rebuild_tools_name_them() {
        assert_eq!(parity_layout_name(2), "左同步");
        assert_eq!(parity_layout_name(7), "布局 #7");
        assert_eq!(raid10_layout_name(0x0102), "近端 2 份");
        assert_eq!(raid10_layout_name(0x0201), "远端 2 份");
        assert_eq!(raid10_layout_name(0x1_0201), "偏移 2 份");
        assert_eq!(chunk_label(128), "64 KiB");
        assert_eq!(chunk_label(2048), "1 MiB");
    }
}
