//! 镜像信息: the evidence header, its volumes and partitions, the source
//! integrity hash and the acquisition details.
use std::{collections::BTreeSet, rc::Rc};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        clipboard::Clipboard,
        description_list::DescriptionList,
        h_flex,
        progress::Progress,
        scroll::ScrollableElement as _,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, relative, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_backend::{
    commands::evidence::hash as hash_commands,
    core::{
        common::hash::{HashAlgo, MultiHashOutput},
        evidence::image::ImageInfo,
    },
    host::State,
};

use super::{ios::IosBackupCard, platform::HeaderActions, volumes::VolumesCard, EvidenceTab};
use crate::{backend::Backend, format, raid::RaidPanel, tasks::TaskStore, ui};

type ExtraIcon = gpui_kit::assets::IconName;

const ALGORITHMS: [(HashAlgo, &str); 4] = [
    (HashAlgo::Md5, "MD5"),
    (HashAlgo::Sha1, "SHA1"),
    (HashAlgo::Sha256, "SHA256"),
    (HashAlgo::Sm3, "SM3"),
];

enum HashState {
    Idle,
    Running { task_id: String },
    Done(MultiHashOutput, Option<String>),
    Failed(SharedString),
}

/// How a digest recorded at acquisition compares with the computed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Match,
    Mismatch,
    Pending,
}

/// A digest the acquisition tool recorded in the image (EWF).
struct StoredHash {
    label: &'static str,
    value: String,
    verdict: Verdict,
}

fn normalize_digest(value: &str) -> String {
    value.split_whitespace().collect::<String>().to_lowercase()
}

/// The recorded digests, each compared with the computed one when there is
/// one.
fn stored_hashes(info: &ImageInfo, computed: Option<&MultiHashOutput>) -> Vec<StoredHash> {
    let Some(ewf) = &info.ewf else {
        return Vec::new();
    };
    [
        (
            "MD5",
            ewf.stored_md5.as_ref(),
            computed.and_then(|hashes| hashes.md5.as_ref()),
        ),
        (
            "SHA1",
            ewf.stored_sha1.as_ref(),
            computed.and_then(|hashes| hashes.sha1.as_ref()),
        ),
    ]
    .into_iter()
    .filter_map(|(label, stored, digest)| {
        let stored = stored.filter(|value| !value.trim().is_empty())?;
        let verdict = match digest {
            None => Verdict::Pending,
            Some(digest) if normalize_digest(digest) == normalize_digest(stored) => Verdict::Match,
            Some(_) => Verdict::Mismatch,
        };
        Some(StoredHash {
            label,
            value: stored.clone(),
            verdict,
        })
    })
    .collect()
}

/// Match when every compared digest matches, mismatch when any differs.
fn overall_verdict(rows: &[StoredHash]) -> Option<Verdict> {
    let mut judged = rows
        .iter()
        .filter(|row| row.verdict != Verdict::Pending)
        .peekable();
    judged.peek()?;
    Some(if judged.all(|row| row.verdict == Verdict::Match) {
        Verdict::Match
    } else {
        Verdict::Mismatch
    })
}

pub struct Dashboard {
    info: Rc<ImageInfo>,
    /// The platform conclusion and the RAW export beside the header.
    actions: Entity<HeaderActions>,
    /// Device details, unlock and audit, for iTunes / Finder backups.
    ios: Option<Entity<IosBackupCard>>,
    volumes: Entity<VolumesCard>,
    /// RAID detection, for evidence with a block layer.
    raid: Option<Entity<RaidPanel>>,
    algorithms: BTreeSet<usize>,
    hash: HashState,
    tasks: Entity<TaskStore>,
    _tasks: Subscription,
}

/// The format badge: image formats keep their code (VMDK, EWF), the others
/// their Chinese name.
fn format_label(info: &ImageInfo) -> String {
    if info.ios_backup.is_some() {
        return "iOS 备份".to_owned();
    }
    let wire = super::wire_name(&info.source_format);
    match wire.as_str() {
        "sevenz" => "7z",
        "docker" => "Docker",
        "folder" => "文件夹",
        "physical_disk" => "物理磁盘",
        "firmware" => "固件镜像",
        "xiaomi_firmware" => "小米固件",
        "android_backup" => "Android 备份",
        "ldplayer" => "雷电模拟器备份",
        "assembled" => "RAID 重组",
        "decrypted" => "解密视图",
        "shadow_copy" => "VSS 卷影副本",
        other => return other.to_uppercase(),
    }
    .to_owned()
}

/// A partition's filesystem as the interface names it.
pub fn filesystem_label(kind: &impl serde::Serialize) -> String {
    let key = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    match key.as_str() {
        "ntfs" => "NTFS",
        "refs" => "ReFS",
        "ext4" => "Ext4",
        "btrfs" => "Btrfs",
        "fat32" => "FAT32",
        "fat16" => "FAT16",
        "fat12" => "FAT12",
        "exfat" => "exFAT",
        "cdfs" => "ISO 9660",
        "squashfs" => "SquashFS",
        "erofs" => "EROFS",
        "f2fs" => "F2FS",
        "xfs" => "XFS",
        "zfs" => "ZFS",
        "ufs" => "UFS",
        "lvm" => "LVM",
        "bitlocker" => "BitLocker",
        "luks" => "LUKS",
        "filevault" => "FileVault",
        "apfscontainer" => "APFS",
        "hfs" => "HFS",
        "hfsplus" => "HFS+",
        "hfsx" => "HFSX",
        "swap" => "Swap",
        "extended" => "扩展分区",
        "mdadm" => "MD RAID",
        "biosboot" => "BIOS Boot",
        "unknown" => "未知",
        other => return other.to_uppercase(),
    }
    .to_owned()
}

impl Dashboard {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let actions = cx.new(|cx| HeaderActions::new(Rc::clone(&info), window, cx));
        let ios = info
            .ios_backup
            .is_some()
            .then(|| cx.new(|cx| IosBackupCard::new(Rc::clone(&info), tab.clone(), window, cx)));
        let volumes = cx.new(|cx| VolumesCard::new(Rc::clone(&info), tab.clone(), cx));
        let raid = super::supports_block_operations(&info)
            .then(|| cx.new(|cx| RaidPanel::new(Rc::clone(&info), tab, cx)));
        let tasks = TaskStore::global(cx);
        let subscription = cx.observe(&tasks, |this, _, cx| {
            if matches!(this.hash, HashState::Running { .. }) {
                cx.notify();
            }
        });
        Self {
            info,
            actions,
            ios,
            volumes,
            raid,
            algorithms: BTreeSet::from([0, 2]),
            hash: HashState::Idle,
            tasks,
            _tasks: subscription,
        }
    }

    fn start_hash(&mut self, cx: &mut Context<Self>) {
        let algorithms: Vec<HashAlgo> = self
            .algorithms
            .iter()
            .filter_map(|index| ALGORITHMS.get(*index).map(|(algorithm, _)| *algorithm))
            .collect();
        if algorithms.is_empty() {
            return;
        }
        let task_id = uuid::Uuid::new_v4().to_string();
        self.hash = HashState::Running {
            task_id: task_id.clone(),
        };
        let image_id = self.info.id.clone();
        let computing = Backend::global(cx).run(move |host, state| async move {
            hash_commands::image_hash_compute(
                host,
                State::new(&state),
                image_id,
                algorithms,
                task_id,
                None,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let computed = computing.await;
            this.update(cx, |this, cx| {
                this.hash = match computed {
                    Ok(result) => {
                        let warning = result.read_faults.read_faults_detected.then(|| {
                            format!(
                                "含 {} 补零字节（{} 个坏区），摘要不代表源数据完整",
                                format::bytes(result.read_faults.synthetic_bytes),
                                result.read_faults.bad_regions.len()
                            )
                        });
                        HashState::Done(result.hashes, warning)
                    }
                    Err(error) if error.cancelled => HashState::Idle,
                    Err(error) => HashState::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn cancel_hash(&mut self, cx: &mut Context<Self>) {
        if let HashState::Running { task_id } = &self.hash {
            let task_id = task_id.clone();
            self.tasks.update(cx, |tasks, cx| tasks.cancel(task_id, cx));
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = &self.info;
        let format_name = format_label(info);
        let wire = super::wire_name(&info.source_format);
        let icon = match wire.as_str() {
            "folder" => Icon::new(ExtraIcon::FolderOpen),
            "zip" | "tar" | "android_backup" | "sevenz" | "ad1" => Icon::new(ExtraIcon::Archive),
            "docker" => Icon::new(ExtraIcon::Container),
            "decrypted" => Icon::new(ExtraIcon::LockOpen),
            "shadow_copy" => Icon::new(ExtraIcon::ClockArrowLeft),
            "assembled" => Icon::new(ExtraIcon::Boxes),
            _ => Icon::new(IconName::HardDrive),
        };
        // An assembled array has no file of its own to point at.
        let show_path = wire != "assembled";
        let on_disk = info
            .on_disk_bytes
            .filter(|bytes| *bytes > 0 && *bytes != info.size_bytes);
        h_flex()
            .w_full()
            .gap_3()
            .items_start()
            .child(
                div()
                    .flex_shrink_0()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().accent)
                    .child(icon.size(px(24.)).text_color(cx.theme().accent_foreground)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(format::file_name(&info.path).to_owned()),
                            )
                            .child(Tag::secondary().small().child(format_name))
                            .when(info.index_only, |this| {
                                this.child(Tag::warning().small().child("离线索引"))
                            }),
                    )
                    .when(show_path, |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(ui::mono(info.path.clone(), cx).truncate())
                                .child(
                                    Clipboard::new("copy-path")
                                        .value(info.path.clone())
                                        .tooltip("复制路径")
                                        .accessibility_label("复制检材路径"),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(format::binary_bytes(info.size_bytes)),
                            )
                            .when_some(on_disk, |this, on_disk| {
                                this.child(div().text_color(cx.theme().muted_foreground).child(
                                    format!(
                                        "实际占用 {}（{:.1}%）",
                                        format::binary_bytes(on_disk),
                                        on_disk as f64 / info.size_bytes.max(1) as f64 * 100.0
                                    ),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .w(relative(0.48))
                    .max_w(px(520.))
                    .min_w_0()
                    .child(self.actions.clone()),
            )
    }

    fn render_hash(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = matches!(self.hash, HashState::Running { .. });
        let folder = serde_json::to_value(self.info.source_format)
            .ok()
            .is_some_and(|value| value.as_str() == Some("folder"));
        let unavailable = folder || self.info.index_only;
        let computed = match &self.hash {
            HashState::Done(hashes, _) => Some(hashes),
            _ => None,
        };
        let stored = stored_hashes(&self.info, computed);
        let title = ui::card_heading(Icon::new(ExtraIcon::FingerprintPattern), "完整性校验", cx)
            .flex_wrap()
            .map(|this| match overall_verdict(&stored) {
                Some(Verdict::Match) => this.child(
                    Tag::success().small().child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(ExtraIcon::ShieldCheck).xsmall())
                            .child("与采集记录一致"),
                    ),
                ),
                Some(Verdict::Mismatch) => this.child(
                    Tag::danger().small().child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(ExtraIcon::ShieldAlert).xsmall())
                            .child("与采集记录不一致"),
                    ),
                ),
                Some(Verdict::Pending) | None => this,
            });
        let selection = h_flex()
            .gap_2()
            .flex_wrap()
            .children(ALGORITHMS.iter().enumerate().map(|(index, (_, label))| {
                Checkbox::new(("hash-algorithm", index))
                    .label(*label)
                    .checked(self.algorithms.contains(&index))
                    .disabled(running || unavailable)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.algorithms.insert(index);
                        } else {
                            this.algorithms.remove(&index);
                        }
                        cx.notify();
                    }))
            }));
        let actions = if running {
            Button::new("cancel-hash")
                .small()
                .danger()
                .label("终止")
                .on_click(cx.listener(|this, _, _, cx| this.cancel_hash(cx)))
        } else {
            Button::new("start-hash")
                .small()
                .primary()
                .label("计算哈希")
                .disabled(unavailable || self.algorithms.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.start_hash(cx)))
        };
        let body = match &self.hash {
            HashState::Idle if unavailable => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("文件夹与离线索引检材没有可校验的源字节流。")
                .into_any_element(),
            HashState::Idle => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("一次读取源镜像，同时计算所选摘要。")
                .into_any_element(),
            HashState::Running { task_id } => {
                let row = self.tasks.read(cx).row(task_id).cloned();
                let percent = row.as_ref().and_then(|row| row.percent());
                v_flex()
                    .gap_2()
                    .child(
                        Progress::new("hash-progress")
                            .when_some(percent, |progress, percent| progress.value(percent))
                            .when(percent.is_none(), |progress| progress.loading(true)),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(row.map(|row| row.message()).unwrap_or_default())
                            .child(
                                percent
                                    .map(|percent| format!("{percent:.1}%"))
                                    .unwrap_or_default(),
                            ),
                    )
                    .into_any_element()
            }
            HashState::Done(hashes, warning) => {
                let digests = [
                    ("MD5", hashes.md5.clone()),
                    ("SHA1", hashes.sha1.clone()),
                    ("SHA256", hashes.sha256.clone()),
                    ("SM3", hashes.sm3.clone()),
                ];
                v_flex()
                    .gap_1()
                    .when_some(warning.clone(), |this, warning| {
                        this.child(ui::error_text(warning, cx))
                    })
                    .children(digests.into_iter().filter_map(|(label, digest)| {
                        let digest = digest?;
                        Some(ui::field_row(
                            label,
                            h_flex()
                                .gap_1()
                                .child(ui::mono(digest.clone(), cx).text_xs())
                                .child(
                                    Clipboard::new(SharedString::from(format!("copy-{label}")))
                                        .value(digest)
                                        .accessibility_label(format!("复制 {label}")),
                                ),
                            cx,
                        ))
                    }))
                    .into_any_element()
            }
            HashState::Failed(message) => ui::error_text(message.clone(), cx).into_any_element(),
        };
        let recorded = (!stored.is_empty()).then(|| {
            v_flex()
                .gap_1()
                .child(ui::section_title("采集时记录的哈希", cx))
                .children(stored.into_iter().map(|row| {
                    let (verdict, color) = match row.verdict {
                        Verdict::Match => ("一致", cx.theme().success),
                        Verdict::Mismatch => ("不一致", cx.theme().danger),
                        Verdict::Pending => ("未校验", cx.theme().muted_foreground),
                    };
                    ui::field_row(
                        row.label,
                        h_flex()
                            .gap_2()
                            .child(ui::mono(row.value.clone(), cx).text_xs())
                            .child(div().text_xs().text_color(color).child(verdict))
                            .child(
                                Clipboard::new(SharedString::from(format!(
                                    "copy-recorded-{}",
                                    row.label
                                )))
                                .value(row.value)
                                .accessibility_label(format!("复制采集时记录的 {}", row.label)),
                            ),
                        cx,
                    )
                }))
        });
        ui::card(cx)
            .p_4()
            .gap_3()
            .child(title)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .flex_wrap()
                    .child(selection)
                    .child(actions),
            )
            .child(body)
            .children(recorded)
    }

    fn render_details(&self, cx: &Context<Self>) -> impl IntoElement {
        let info = &self.info;
        let mut list = DescriptionList::horizontal()
            .columns(1)
            .bordered(false)
            .label_width(gpui_kit::px(112.))
            .item(
                "逻辑大小",
                format!("{} 字节", format::count(info.size_bytes)),
                1,
            )
            .item("扇区大小", format!("{} B", info.sector_size), 1);
        if let Some(disk_id) = &info.disk_id {
            list = list.item("磁盘标识", disk_id.clone(), 1);
        }
        if let Some(ewf) = &info.ewf {
            // The parsed date is a display interpretation; it says whether the
            // header held wall time or UTC. The raw header stays authoritative.
            let acquired = ewf.acquiry_date_parsed.as_ref().map(|date| {
                let basis = match date.time_basis {
                    lovelycarve_backend::core::evidence::readers::ewf_image::EwfDateTimeBasis::Local => "采集机本地时间",
                    lovelycarve_backend::core::evidence::readers::ewf_image::EwfDateTimeBasis::Unix => "UTC",
                };
                Some(format!("{}（{basis}）", date.display))
            });
            let acquired = acquired.unwrap_or_else(|| ewf.acquiry_date.clone());
            let rows = [
                ("案件编号", &ewf.case_number),
                ("证据编号", &ewf.evidence_number),
                ("检验人", &ewf.examiner_name),
                ("描述", &ewf.description),
                ("采集时间", &acquired),
                ("采集系统", &ewf.acquiry_operating_system),
                ("采集软件", &ewf.acquiry_software_version),
                ("设备型号", &ewf.model),
                ("序列号", &ewf.serial_number),
                ("压缩", &ewf.compression),
            ];
            list = list.separator();
            for (label, value) in rows {
                if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                    list = list.item(label, value.clone(), 1);
                }
            }
            if let Some(segments) = ewf.segment_count {
                list = list.item("分段数", segments.to_string(), 1);
            }
        }
        ui::card(cx)
            .p_4()
            .gap_3()
            .child(ui::card_heading(Icon::new(IconName::Info), "详细信息", cx))
            .child(list)
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = window.viewport_size().width >= px(1120.);
        v_flex()
            .size_full()
            .p_6()
            .gap_4()
            .child(self.render_header(cx))
            .children(self.ios.clone())
            .child(self.volumes.clone())
            .children(self.raid.clone())
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap_4()
                    .when(!wide, |this| this.flex_col())
                    .when(wide, |this| this.items_start())
                    .child(
                        div()
                            .min_w_0()
                            .when(wide, |this| this.flex_1())
                            .child(self.render_hash(cx)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .when(wide, |this| this.flex_1())
                            .child(self.render_details(cx)),
                    ),
            )
            .overflow_y_scrollbar()
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_digest, overall_verdict, StoredHash, Verdict};

    fn row(verdict: Verdict) -> StoredHash {
        StoredHash {
            label: "MD5",
            value: String::new(),
            verdict,
        }
    }

    #[test]
    fn digests_compare_without_case_or_spacing() {
        assert_eq!(normalize_digest(" AA bb\tCC "), "aabbcc");
    }

    #[test]
    fn evidence_matches_only_when_every_compared_digest_matches() {
        assert_eq!(overall_verdict(&[]), None);
        assert_eq!(overall_verdict(&[row(Verdict::Pending)]), None);
        assert_eq!(
            overall_verdict(&[row(Verdict::Match), row(Verdict::Pending)]),
            Some(Verdict::Match)
        );
        assert_eq!(
            overall_verdict(&[row(Verdict::Match), row(Verdict::Mismatch)]),
            Some(Verdict::Mismatch)
        );
    }
}
