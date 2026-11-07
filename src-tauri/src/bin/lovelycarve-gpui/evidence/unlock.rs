//! Unlock dialogs for encrypted volumes: a BitLocker partition (password,
//! recovery key, a memory image's VMK candidates or raw keys) and LUKS /
//! VeraCrypt / FileVault containers. Unlocks run as backend tasks; the
//! dialogs follow their progress through the task center store.
use std::{path::PathBuf, rc::Rc};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        progress::Progress,
        radio::RadioGroup,
        spinner::Spinner,
        tab::{Tab, TabBar},
        v_flex, ActiveTheme as _, Disableable as _, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
};
use lovelycarve_lib::{
    commands::{
        app_error::AppError,
        evidence::{bitlocker as bitlocker_commands, encrypted_volume as encrypted_commands},
    },
    core::evidence::image::ImageInfo,
    host::State,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::filesystem_label;
use crate::{backend::Backend, format, tasks::TaskStore, ui};

const RECOVERY_GROUPS: usize = 8;
const RECOVERY_GROUP_DIGITS: usize = 6;
/// 65535 × 11: the largest group whose quotient still fits in 16 bits.
const RECOVERY_GROUP_MAX: u32 = 720_885;

/// How far a typed BitLocker recovery password is from a valid one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryKeyCheck {
    /// The `xxxxxx-…-xxxxxx` form sent to the backend.
    pub normalized: String,
    /// Complete groups that pass the checksum.
    pub valid_groups: usize,
    /// A finished group that can never be right; shown while typing.
    pub group_error: Option<String>,
    /// What is still missing; shown once the field was left or submitted.
    pub format_error: Option<String>,
}

/// Pasted keys arrive with spaces, other dashes or no separators at all; a
/// run of digits without separators is grouped six at a time.
pub fn normalize_recovery_key(raw: &str) -> String {
    let mut joined = String::new();
    let mut separated = false;
    for character in raw.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separated && !joined.is_empty() {
                joined.push('-');
            }
            separated = false;
            joined.push(character);
        } else {
            separated = true;
        }
    }
    if !joined.is_empty() && joined.chars().all(|character| character.is_ascii_digit()) {
        let mut grouped =
            String::with_capacity(joined.len() + joined.len() / RECOVERY_GROUP_DIGITS);
        for (index, digit) in joined.chars().enumerate() {
            if index > 0 && index.is_multiple_of(RECOVERY_GROUP_DIGITS) {
                grouped.push('-');
            }
            grouped.push(digit);
        }
        return grouped;
    }
    joined
}

/// BitLocker recovery passwords are eight dash-separated groups of six
/// digits; every group is divisible by 11 and its quotient fits in 16 bits.
pub fn check_recovery_key(raw: &str) -> RecoveryKeyCheck {
    let normalized = normalize_recovery_key(raw);
    let groups: Vec<&str> = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized.split('-').collect()
    };
    let mut valid_groups = 0;
    let mut group_error = None;
    let mut format_error = None;
    for (index, group) in groups.iter().enumerate() {
        let position = index + 1;
        if !group.chars().all(|character| character.is_ascii_digit()) {
            group_error = Some(format!("只能输入数字，第 {position} 组含有其他字符"));
            break;
        }
        if index >= RECOVERY_GROUPS {
            group_error = Some(format!(
                "恢复密钥只有 {RECOVERY_GROUPS} 组，请删除多出的第 {position} 组"
            ));
            break;
        }
        if group.len() > RECOVERY_GROUP_DIGITS {
            group_error = Some(format!(
                "第 {position} 组超过 {RECOVERY_GROUP_DIGITS} 位，请检查分隔"
            ));
            break;
        }
        if group.len() < RECOVERY_GROUP_DIGITS {
            format_error.get_or_insert_with(|| {
                format!(
                    "第 {position} 组只有 {} 位，应为 {RECOVERY_GROUP_DIGITS} 位",
                    group.len()
                )
            });
            continue;
        }
        let Ok(value) = group.parse::<u32>() else {
            group_error = Some(format!("只能输入数字，第 {position} 组含有其他字符"));
            break;
        };
        if !value.is_multiple_of(11) {
            group_error = Some(format!(
                "第 {position} 组「{group}」校验失败（应能被 11 整除），请检查是否输错"
            ));
            break;
        }
        if value > RECOVERY_GROUP_MAX {
            group_error = Some(format!(
                "第 {position} 组「{group}」超出有效范围（不大于 {RECOVERY_GROUP_MAX}）"
            ));
            break;
        }
        valid_groups += 1;
    }
    if group_error.is_none() && format_error.is_none() && groups.len() < RECOVERY_GROUPS {
        format_error = Some(format!(
            "还差 {} 组（共 {RECOVERY_GROUPS} 组 {RECOVERY_GROUP_DIGITS} 位数字）",
            RECOVERY_GROUPS - groups.len()
        ));
    }
    RecoveryKeyCheck {
        normalized,
        valid_groups,
        group_error,
        format_error,
    }
}

/// Hexadecimal key text without the spaces pasted keys often carry.
fn hex_digits(value: &str) -> String {
    value.split_whitespace().collect()
}

fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|character| character.is_ascii_hexdigit())
}

/// Why a raw BitLocker key cannot be used, if it cannot: a VMK is 32 bytes,
/// an FVEK 16, 32 or 64 bytes and an optional TWEAK 16 or 32 bytes.
fn raw_key_error(kind: KeyKind, key: &str, tweak: &str) -> Option<&'static str> {
    if !is_hex(key) {
        return Some("请输入有效的十六进制密钥。");
    }
    let length_ok = match kind {
        KeyKind::Vmk => key.len() == 64,
        _ => [32, 64, 128].contains(&key.len()),
    };
    if !length_ok {
        return Some("密钥长度与所选类型不符。");
    }
    if matches!(kind, KeyKind::Fvek)
        && !tweak.is_empty()
        && (!is_hex(tweak) || ![32, 64].contains(&tweak.len()))
    {
        return Some("TWEAK 应为 16 或 32 字节的十六进制密钥。");
    }
    None
}

fn protector_label(value: u64) -> String {
    match value {
        0 => "明文密钥".to_owned(),
        0x0100 => "TPM".to_owned(),
        0x0200 => "BEK".to_owned(),
        0x0800 => "恢复密码".to_owned(),
        0x2000 => "密码".to_owned(),
        other => format!("0x{other:04x}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Password,
    Recovery,
    Memory,
    Key,
}

const MODES: [(Mode, &str); 4] = [
    (Mode::Password, "密码"),
    (Mode::Recovery, "恢复密钥"),
    (Mode::Memory, "内存镜像"),
    (Mode::Key, "其他密钥"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    Startup,
    Vmk,
    Fvek,
    Clear,
}

const KEY_KINDS: [(KeyKind, &str); 4] = [
    (KeyKind::Startup, "BEK 启动密钥"),
    (KeyKind::Vmk, "VMK"),
    (KeyKind::Fvek, "FVEK / TWEAK"),
    (KeyKind::Clear, "明文密钥"),
];

/// One VMK a memory scan found for this volume.
#[derive(Clone, Debug, Deserialize)]
struct VmkCandidate {
    offset: u64,
    vmk_hex: String,
    recovery_key: Option<String>,
    already_tried: bool,
}

/// The credential an unlock sends.
enum Credential {
    Recovery(String),
    Password(String),
    StartupKey(String),
    Keys { fvek: String, tweak: Option<String> },
    Clear,
    Vmk(String),
}

struct Job {
    task_id: String,
    label: &'static str,
    note: &'static str,
}

/// The dialog unlocked its partition.
#[derive(Clone, Copy, Debug)]
pub struct BitlockerUnlocked;

pub struct BitlockerDialog {
    image_id: String,
    partition_id: u8,
    mode: Mode,
    password: Entity<InputState>,
    recovery: Entity<InputState>,
    recovery_touched: bool,
    key_kind: KeyKind,
    key_hex: Entity<InputState>,
    tweak_hex: Entity<InputState>,
    key_touched: bool,
    startup_key: Option<PathBuf>,
    memory_path: Option<PathBuf>,
    candidates: Option<Vec<VmkCandidate>>,
    metadata: Option<String>,
    job: Option<Job>,
    error: Option<SharedString>,
    tasks: Entity<TaskStore>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BitlockerUnlocked> for BitlockerDialog {}

impl BitlockerDialog {
    pub fn new(
        image_id: String,
        partition_id: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("BitLocker 用户密码")
        });
        let recovery = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("000000-000000-000000-000000-000000-000000-000000-000000")
        });
        let key_hex = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("十六进制密钥")
        });
        let tweak_hex = cx.new(|cx| InputState::new(window, cx).placeholder("无 Diffuser 时留空"));
        let tasks = TaskStore::global(cx);
        let mut subscriptions = vec![cx.observe(&tasks, |this, _, cx| {
            if this.job.is_some() {
                cx.notify();
            }
        })];
        for input in [&password, &recovery, &key_hex, &tweak_hex] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::Blur => {
                        if *input == this.recovery {
                            this.recovery_touched = true;
                        } else if *input == this.key_hex || *input == this.tweak_hex {
                            this.key_touched = true;
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Focus => {}
                },
            ));
        }
        let mut dialog = Self {
            image_id,
            partition_id,
            mode: Mode::Recovery,
            password,
            recovery,
            recovery_touched: false,
            key_kind: KeyKind::Startup,
            key_hex,
            tweak_hex,
            key_touched: false,
            startup_key: None,
            memory_path: None,
            candidates: None,
            metadata: None,
            job: None,
            error: None,
            tasks,
            _subscriptions: subscriptions,
        };
        dialog.load_metadata(cx);
        dialog
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Percent of the running job, for the partition row's chip.
    pub fn progress(&self, cx: &gpui_kit::App) -> Option<f32> {
        let job = self.job.as_ref()?;
        self.tasks
            .read(cx)
            .row(&job.task_id)
            .and_then(|row| row.percent())
    }

    fn load_metadata(&mut self, cx: &mut Context<Self>) {
        let (image_id, partition_id) = (self.image_id.clone(), self.partition_id);
        let loading = Backend::global(cx).run(move |_, state| async move {
            let metadata =
                bitlocker_commands::bitlocker_metadata(State::new(&state), image_id, partition_id)
                    .await?;
            serde_json::to_value(metadata)
                .map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            // Damaged metadata leaves the status usable; the hint just stays empty.
            let Ok(metadata) = loading.await else {
                return;
            };
            this.update(cx, |this, cx| {
                let protectors = metadata["protector_types"]
                    .as_array()
                    .map(|types| {
                        types
                            .iter()
                            .filter_map(Value::as_u64)
                            .map(protector_label)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                this.metadata = Some(format!(
                    "元数据 v{} · {} 字节扇区{} · 保护器 {}",
                    metadata["version"].as_u64().unwrap_or_default(),
                    metadata["bytes_per_sector"].as_u64().unwrap_or_default(),
                    if metadata["used_space_only"].as_bool().unwrap_or(false) {
                        " · 仅加密已用空间"
                    } else {
                        ""
                    },
                    if protectors.is_empty() {
                        "无".to_owned()
                    } else {
                        protectors.join("、")
                    }
                ));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn input_text(input: &Entity<InputState>, cx: &gpui_kit::App) -> String {
        input.read(cx).value().to_string()
    }

    fn clear_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.password,
            &self.recovery,
            &self.key_hex,
            &self.tweak_hex,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.recovery_touched = false;
        self.key_touched = false;
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return;
        }
        let credential = match self.mode {
            Mode::Password => {
                let password = Self::input_text(&self.password, cx);
                if password.is_empty() {
                    return;
                }
                Credential::Password(password)
            }
            Mode::Recovery => {
                let check = check_recovery_key(&Self::input_text(&self.recovery, cx));
                self.recovery_touched = true;
                if check.group_error.is_some() || check.format_error.is_some() {
                    cx.notify();
                    return;
                }
                Credential::Recovery(check.normalized)
            }
            Mode::Memory => return,
            Mode::Key => {
                self.key_touched = true;
                let key = hex_digits(&Self::input_text(&self.key_hex, cx));
                let tweak = hex_digits(&Self::input_text(&self.tweak_hex, cx));
                match self.key_kind {
                    KeyKind::Clear => Credential::Clear,
                    KeyKind::Startup => match &self.startup_key {
                        Some(path) => Credential::StartupKey(path.display().to_string()),
                        None => {
                            cx.notify();
                            return;
                        }
                    },
                    kind if raw_key_error(kind, &key, &tweak).is_some() => {
                        cx.notify();
                        return;
                    }
                    KeyKind::Vmk => Credential::Vmk(key),
                    KeyKind::Fvek => Credential::Keys {
                        fvek: key,
                        tweak: (!tweak.is_empty()).then_some(tweak),
                    },
                }
            }
        };
        self.start_unlock(credential, window, cx);
    }

    fn start_unlock(
        &mut self,
        credential: Credential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task_id = uuid::Uuid::new_v4().to_string();
        let label = match credential {
            Credential::Password(_) => "密码解锁中",
            Credential::Recovery(_) => "恢复密钥解锁中",
            Credential::Vmk(_) => "VMK 解锁中",
            _ => "密钥解锁中",
        };
        self.job = Some(Job {
            task_id: task_id.clone(),
            label,
            note: "解锁任务会在后台继续，关闭对话框不会中断。",
        });
        self.error = None;
        self.clear_secrets(window, cx);
        let (image_id, partition_id) = (self.image_id.clone(), self.partition_id);
        let unlocking = Backend::global(cx).run(move |host, state| async move {
            let state = State::new(state.as_ref());
            let task = Some(task_id);
            match credential {
                Credential::Vmk(vmk) => {
                    let result = bitlocker_commands::bitlocker_unlock_with_vmk(
                        host,
                        state,
                        image_id,
                        partition_id,
                        vmk,
                        task,
                    )
                    .await?;
                    let value = serde_json::to_value(result).unwrap_or_default();
                    Ok(value["recovery_key"].as_str().map(str::to_owned))
                }
                other => {
                    let (recovery, password, startup, fvek, tweak) = match other {
                        Credential::Recovery(value) => (Some(value), None, None, None, None),
                        Credential::Password(value) => (None, Some(value), None, None, None),
                        Credential::StartupKey(path) => (None, None, Some(path), None, None),
                        Credential::Keys { fvek, tweak } => (None, None, None, Some(fvek), tweak),
                        Credential::Clear | Credential::Vmk(_) => (None, None, None, None, None),
                    };
                    bitlocker_commands::bitlocker_unlock(
                        host,
                        state,
                        image_id,
                        partition_id,
                        recovery,
                        password,
                        startup,
                        fvek,
                        tweak,
                        task,
                    )
                    .await?;
                    Ok(None)
                }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let unlocked = unlocking.await;
            this.update_in(cx, |this, window, cx| {
                this.job = None;
                match unlocked {
                    Ok(recovery_key) => {
                        this.candidates = None;
                        ui::notify_success(
                            window,
                            cx,
                            "BitLocker 解锁成功",
                            match &recovery_key {
                                Some(key) => {
                                    format!("分区 {} 已解锁，恢复密钥：{key}", this.partition_id)
                                }
                                None => format!("分区 {} 已解锁", this.partition_id),
                            },
                        );
                        cx.emit(BitlockerUnlocked);
                    }
                    Err(error) if error.cancelled => {}
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn pick_startup_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = ui::prompt_paths(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择 BEK 启动密钥".into()),
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = prompt.await.into_iter().next() {
                this.update(cx, |this, cx| {
                    this.startup_key = Some(path);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn pick_memory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = ui::prompt_paths(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择内存镜像（raw / vmem / hiberfil 等）".into()),
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = prompt.await.into_iter().next() {
                this.update(cx, |this, cx| {
                    this.memory_path = Some(path);
                    this.candidates = None;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn scan_memory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(memory_path) = self.memory_path.clone() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let task_id = uuid::Uuid::new_v4().to_string();
        self.job = Some(Job {
            task_id: task_id.clone(),
            label: "扫描内存镜像",
            note: "扫描会在后台继续，关闭对话框不会中断。",
        });
        self.error = None;
        let (image_id, partition_id) = (self.image_id.clone(), self.partition_id);
        let scanning = Backend::global(cx).run(move |host, state| async move {
            let result = bitlocker_commands::bitlocker_scan_memory(
                host,
                State::new(&state),
                image_id,
                partition_id,
                memory_path.display().to_string(),
                task_id,
            )
            .await?;
            serde_json::to_value(result).map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let scanned = scanning.await;
            this.update(cx, |this, cx| {
                this.job = None;
                match scanned {
                    Ok(result) => {
                        this.candidates = Some(
                            serde_json::from_value(result["candidates"].clone())
                                .unwrap_or_default(),
                        );
                    }
                    Err(error) if error.cancelled => {}
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn cancel_job(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.job {
            let task_id = job.task_id.clone();
            self.tasks.update(cx, |tasks, cx| tasks.cancel(task_id, cx));
        }
    }

    fn render_job(&self, job: &Job, cx: &mut Context<Self>) -> impl IntoElement {
        let row = self.tasks.read(cx).row(&job.task_id).cloned();
        let percent = row.as_ref().and_then(|row| row.percent());
        let cancelling = row.as_ref().is_some_and(|row| row.cancelling);
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(Spinner::new().small())
                    .child(div().text_sm().child(if cancelling {
                        "取消中…"
                    } else {
                        job.label
                    })),
            )
            .child(
                Progress::new("bitlocker-job")
                    .when_some(percent, |progress, percent| progress.value(percent))
                    .when(percent.is_none(), |progress| progress.loading(true)),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        row.map(|row| row.message())
                            .unwrap_or_else(|| "准备中…".to_owned()),
                    )
                    .child(
                        percent
                            .map(|percent| format!("{percent:.1}%"))
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(job.note),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("cancel-bitlocker-job")
                        .small()
                        .label(if cancelling {
                            "正在取消…"
                        } else {
                            "取消"
                        })
                        .disabled(cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_job(cx))),
                ),
            )
    }

    fn render_recovery(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let check = check_recovery_key(&Self::input_text(&self.recovery, cx));
        let error = check
            .group_error
            .clone()
            .or_else(|| check.format_error.clone().filter(|_| self.recovery_touched));
        let ready = check.group_error.is_none() && check.format_error.is_none();
        v_flex()
            .gap_2()
            .child(Input::new(&self.recovery).cleanable(true))
            .child(match error {
                Some(error) => ui::error_text(error, cx).text_xs(),
                None => div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "已通过校验 {} / {RECOVERY_GROUPS} 组；可直接粘贴带空格或不带分隔的密钥。",
                        check.valid_groups
                    )),
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("unlock-recovery")
                        .small()
                        .primary()
                        .label("解锁")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }

    fn render_password(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = Self::input_text(&self.password, cx).is_empty();
        v_flex()
            .gap_2()
            .child(Input::new(&self.password).mask_toggle())
            .child(
                h_flex().justify_end().child(
                    Button::new("unlock-password")
                        .small()
                        .primary()
                        .label("解锁")
                        .disabled(empty)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }

    fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .child(div().text_xs().text_color(muted).child(
                "扫描运行中系统的内存镜像，寻找该卷的 VMK；找到后可直接解锁并得到恢复密钥。",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("pick-memory")
                            .small()
                            .label("选择内存镜像…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_memory(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                self.memory_path
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "尚未选择文件".to_owned()),
                            ),
                    )
                    .child(
                        Button::new("scan-memory")
                            .small()
                            .primary()
                            .label("开始扫描")
                            .disabled(self.memory_path.is_none())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.scan_memory(window, cx)),
                            ),
                    ),
            )
            .when_some(self.candidates.as_ref(), |this, candidates| {
                this.child(ui::section_title(
                    format!("候选 VMK · {}", candidates.len()),
                    cx,
                ))
                .when(candidates.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("未在内存镜像中找到该卷可用的 VMK。"),
                    )
                })
                .children(candidates.iter().enumerate().map(|(index, candidate)| {
                    let vmk = candidate.vmk_hex.clone();
                    h_flex()
                        .gap_2()
                        .py_1()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(ui::mono(format::hex_offset(candidate.offset), cx).text_xs())
                                .child(
                                    ui::mono(
                                        candidate
                                            .recovery_key
                                            .clone()
                                            .unwrap_or_else(|| candidate.vmk_hex.clone()),
                                        cx,
                                    )
                                    .text_xs()
                                    .truncate(),
                                ),
                        )
                        .when(candidate.already_tried, |this| {
                            this.child(div().text_xs().text_color(muted).child("已尝试"))
                        })
                        .child(
                            Button::new(("unlock-candidate", index))
                                .small()
                                .label("用此 VMK 解锁")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_unlock(Credential::Vmk(vmk.clone()), window, cx)
                                })),
                        )
                }))
            })
    }

    fn render_key(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let key = hex_digits(&Self::input_text(&self.key_hex, cx));
        let tweak = hex_digits(&Self::input_text(&self.tweak_hex, cx));
        let kind = self.key_kind;
        let error = matches!(kind, KeyKind::Vmk | KeyKind::Fvek)
            .then(|| raw_key_error(kind, &key, &tweak))
            .flatten();
        let ready = match kind {
            KeyKind::Clear => true,
            KeyKind::Startup => self.startup_key.is_some(),
            KeyKind::Vmk | KeyKind::Fvek => error.is_none(),
        };
        v_flex()
            .gap_2()
            .child(
                RadioGroup::horizontal("bitlocker-key-kind")
                    .children(KEY_KINDS.iter().map(|(_, label)| *label))
                    .selected_index(
                        KEY_KINDS
                            .iter()
                            .position(|(candidate, _)| *candidate == kind),
                    )
                    .on_change(cx.listener(|this, index: &usize, window, cx| {
                        if let Some((kind, _)) = KEY_KINDS.get(*index) {
                            this.key_kind = *kind;
                            this.startup_key = None;
                            this.clear_secrets(window, cx);
                            cx.notify();
                        }
                    })),
            )
            .map(|this| match kind {
                KeyKind::Startup => this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("pick-bek")
                                .small()
                                .label("选择 BEK 启动密钥文件…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_startup_key(window, cx)
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(
                                    self.startup_key
                                        .as_ref()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|| "尚未选择文件".to_owned()),
                                ),
                        ),
                ),
                KeyKind::Vmk | KeyKind::Fvek => this
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(if kind == KeyKind::Vmk {
                                "VMK（32 字节）"
                            } else {
                                "FVEK（16、32 或 64 字节）"
                            }),
                    )
                    .child(Input::new(&self.key_hex).mask_toggle())
                    .when(kind == KeyKind::Fvek, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("TWEAK（仅 Elephant Diffuser 需要）"),
                        )
                        .child(Input::new(&self.tweak_hex))
                    })
                    .child(match error.filter(|_| self.key_touched) {
                        Some(error) => ui::error_text(error, cx).text_xs(),
                        None => div()
                            .text_xs()
                            .text_color(muted)
                            .child(if kind == KeyKind::Vmk {
                                "通过 FVEK 认证验证 VMK，不要求恢复密码保护器。"
                            } else {
                                "原始扇区密钥没有认证标签；解锁后请核验文件系统和内容。"
                            }),
                    }),
                KeyKind::Clear => this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("尝试卷内明文密钥保护器，适用于已暂停保护的卷。"),
                ),
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("unlock-key")
                        .small()
                        .primary()
                        .label("解锁")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }
}

impl Render for BitlockerDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(job) = &self.job {
            let job = Job {
                task_id: job.task_id.clone(),
                label: job.label,
                note: job.note,
            };
            return v_flex()
                .w_full()
                .gap_3()
                .child(self.render_job(&job, cx))
                .into_any_element();
        }
        let mode = self.mode;
        let body = match mode {
            Mode::Password => self.render_password(cx).into_any_element(),
            Mode::Recovery => self.render_recovery(cx).into_any_element(),
            Mode::Memory => self.render_memory(cx).into_any_element(),
            Mode::Key => self.render_key(cx).into_any_element(),
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(
                TabBar::new("bitlocker-modes")
                    .segmented()
                    .children(MODES.iter().map(|(_, label)| Tab::new().label(*label)))
                    .selected_index(
                        MODES
                            .iter()
                            .position(|(candidate, _)| *candidate == mode)
                            .unwrap_or(0),
                    )
                    .on_click(cx.listener(|this, index: &usize, window, cx| {
                        if let Some((mode, _)) = MODES.get(*index) {
                            this.mode = *mode;
                            this.error = None;
                            this.clear_secrets(window, cx);
                            cx.notify();
                        }
                    })),
            )
            .child(body)
            .when_some(self.metadata.clone(), |this, metadata| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(metadata),
                )
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Luks,
    VeraCrypt,
    FileVault,
}

const SCHEMES: [(Scheme, &str, &str); 3] = [
    (Scheme::Luks, "luks", "LUKS1 / LUKS2"),
    (Scheme::VeraCrypt, "veracrypt", "VeraCrypt"),
    (Scheme::FileVault, "filevault", "FileVault 2 / CoreStorage"),
];

/// One logical volume of a CoreStorage container.
#[derive(Clone, Debug, Deserialize)]
struct LogicalVolume {
    index: u64,
    name: Option<String>,
    identifier: Option<String>,
    size: u64,
    pbkdf2_iterations: u64,
    protectors: Vec<String>,
    conversion_status: Option<String>,
    #[serde(default)]
    findings: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct Inventory {
    volumes: Vec<LogicalVolume>,
    header_offset: u64,
}

enum InventoryState {
    Unread,
    Loading,
    Loaded(Inventory),
    Failed(SharedString),
}

/// The container dialog asks to unlock with these `UnlockOptions` (camelCase
/// JSON, as the IPC layer sends them).
#[derive(Clone, Debug)]
pub struct ContainerUnlockRequest(pub Value);

pub struct ContainerDialog {
    info: Rc<ImageInfo>,
    fixed_partition: Option<u8>,
    scheme: Scheme,
    /// 0 is the whole image, `n` the n-th partition.
    region: usize,
    master_key: bool,
    password: Entity<InputState>,
    key: Entity<InputState>,
    pim: Entity<InputState>,
    hidden: bool,
    root_plist: Entity<InputState>,
    inventory: InventoryState,
    logical_volume: u64,
    key_touched: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ContainerUnlockRequest> for ContainerDialog {}

impl ContainerDialog {
    pub fn new(
        info: Rc<ImageInfo>,
        partition: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("十六进制主密钥")
        });
        let pim = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        let root_plist = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("EncryptedRoot.plist.wipekey 或 XML 的本地路径（可选）")
        });
        let mut subscriptions = Vec::new();
        for input in [&password, &key, &pim] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::Blur => {
                        if *input == this.key {
                            this.key_touched = true;
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Focus => {}
                },
            ));
        }
        subscriptions.push(
            cx.subscribe(&root_plist, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.inventory = InventoryState::Unread;
                    cx.notify();
                }
            }),
        );
        // A partition row preselects its partition and, for LUKS or
        // FileVault partitions, the matching scheme.
        let preset = partition.and_then(|id| {
            info.partitions
                .iter()
                .position(|candidate| candidate.id == id)
        });
        let scheme = preset
            .and_then(|index| info.partitions.get(index))
            .map(|partition| filesystem_label(&partition.kind))
            .map(|label| match label.as_str() {
                "FileVault" => Scheme::FileVault,
                _ => Scheme::Luks,
            })
            .unwrap_or(Scheme::Luks);
        Self {
            region: preset.map(|index| index + 1).unwrap_or(0),
            fixed_partition: partition,
            info,
            scheme,
            master_key: false,
            password,
            key,
            pim,
            hidden: false,
            root_plist,
            inventory: InventoryState::Unread,
            logical_volume: 0,
            key_touched: false,
            _subscriptions: subscriptions,
        }
    }

    fn partition_id(&self) -> Option<u8> {
        self.fixed_partition.or_else(|| {
            self.region
                .checked_sub(1)
                .and_then(|index| self.info.partitions.get(index))
                .map(|partition| partition.id)
        })
    }

    fn uses_master_key(&self) -> bool {
        self.scheme != Scheme::VeraCrypt && self.master_key
    }

    fn key_error(&self, cx: &gpui_kit::App) -> Option<&'static str> {
        let key = hex_digits(&self.key.read(cx).value());
        if !key.is_empty() && !key.chars().all(|character| character.is_ascii_hexdigit()) {
            return Some("主密钥只能包含十六进制字符 0–9、A–F。");
        }
        if self.scheme == Scheme::FileVault
            && (self.key_touched || !key.is_empty())
            && ![32, 64].contains(&key.len())
        {
            return Some("FileVault 密钥需要 16 或 32 字节（32 或 64 个十六进制字符）。");
        }
        if key.len() > 512 {
            return Some("主密钥不能超过 256 字节。");
        }
        if self.key_touched && (key.is_empty() || key.len() % 2 == 1) {
            return Some("每个字节需要两个十六进制字符。");
        }
        None
    }

    fn selected_volume(&self) -> Option<&LogicalVolume> {
        match &self.inventory {
            InventoryState::Loaded(inventory) => inventory
                .volumes
                .iter()
                .find(|volume| volume.index == self.logical_volume),
            _ => None,
        }
    }

    fn can_unlock(&self, cx: &gpui_kit::App) -> bool {
        let volume_ready = self.scheme != Scheme::FileVault
            || self
                .selected_volume()
                .is_some_and(|volume| volume.conversion_status.as_deref() == Some("Complete"));
        let credential_ready = if self.uses_master_key() {
            let key = hex_digits(&self.key.read(cx).value());
            !key.is_empty() && key.len().is_multiple_of(2) && self.key_error(cx).is_none()
        } else {
            !self.password.read(cx).value().is_empty()
        };
        volume_ready && credential_ready
    }

    fn inspect_options(&self, cx: &gpui_kit::App) -> Value {
        let mut options = json!({});
        if let Some(partition) = self.partition_id() {
            options["partitionId"] = json!(partition);
        }
        let plist = self.root_plist.read(cx).value().trim().to_owned();
        if !plist.is_empty() {
            options["encryptedRootPlistPath"] = json!(plist);
        }
        options
    }

    fn load_inventory(&mut self, cx: &mut Context<Self>) {
        self.inventory = InventoryState::Loading;
        let image_id = self.info.id.clone();
        let options = self.inspect_options(cx);
        let reading = Backend::global(cx).run(move |_, state| async move {
            let options = serde_json::from_value(options)
                .map_err(|error| AppError::invalid_input(error.to_string()))?;
            let inventory = encrypted_commands::filevault_inspect(
                State::new(&state),
                image_id,
                options,
                Some(uuid::Uuid::new_v4().to_string()),
            )
            .await?;
            serde_json::to_value(inventory)
                .map_err(|error| AppError::invalid_input(error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            this.update(cx, |this, cx| {
                this.inventory = match read.map(serde_json::from_value::<Inventory>) {
                    Ok(Ok(inventory)) => {
                        this.logical_volume = inventory
                            .volumes
                            .first()
                            .map(|volume| volume.index)
                            .unwrap_or(0);
                        InventoryState::Loaded(inventory)
                    }
                    Ok(Err(error)) => InventoryState::Failed(error.to_string().into()),
                    Err(error) => InventoryState::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.key_touched = self.uses_master_key();
        if !self.can_unlock(cx) {
            cx.notify();
            return;
        }
        let scheme = SCHEMES
            .iter()
            .find(|(candidate, _, _)| *candidate == self.scheme)
            .map(|(_, key, _)| *key)
            .unwrap_or("luks");
        let mut options = json!({ "scheme": scheme, "target": "sourcePartition" });
        if self.uses_master_key() {
            options["masterKeyHex"] = json!(hex_digits(&self.key.read(cx).value()));
        } else {
            options["password"] = json!(self.password.read(cx).value().to_string());
        }
        if let Some(partition) = self.partition_id() {
            options["partitionId"] = json!(partition);
        }
        match self.scheme {
            Scheme::VeraCrypt => {
                let pim = self.pim.read(cx).value().trim().parse::<u32>().unwrap_or(0);
                options["pim"] = json!(pim);
                options["hidden"] = json!(self.hidden);
            }
            Scheme::FileVault => {
                if let InventoryState::Loaded(inventory) = &self.inventory {
                    if inventory.volumes.len() > 1 {
                        options["target"] = json!("newImage");
                    }
                }
                options["logicalVolumeIndex"] = json!(self.logical_volume);
                let plist = self.root_plist.read(cx).value().trim().to_owned();
                if !plist.is_empty() {
                    options["encryptedRootPlistPath"] = json!(plist);
                }
            }
            Scheme::Luks => {}
        }
        for input in [&self.password, &self.key] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.emit(ContainerUnlockRequest(options));
    }

    fn render_filevault(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let busy = matches!(self.inventory, InventoryState::Loading);
        v_flex()
            .gap_2()
            .child(Input::new(&self.root_plist).cleanable(true))
            .child(
                h_flex().child(
                    Button::new("read-filevault")
                        .small()
                        .ghost()
                        .label(if busy { "正在读取卷信息…" } else { "读取卷信息" })
                        .loading(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load_inventory(cx))),
                ),
            )
            .map(|this| match &self.inventory {
                InventoryState::Failed(error) => this.child(ui::error_text(error.clone(), cx).text_xs()),
                InventoryState::Loaded(inventory) => {
                    let labels: Vec<SharedString> = inventory
                        .volumes
                        .iter()
                        .map(|volume| {
                            let name = volume
                                .name
                                .clone()
                                .or_else(|| volume.identifier.clone())
                                .unwrap_or_else(|| format!("逻辑卷 {}", volume.index));
                            SharedString::from(format!("{name} · {}", format::bytes(volume.size)))
                        })
                        .collect();
                    let selected = inventory.volumes.iter().position(|volume| volume.index == self.logical_volume);
                    this.when(inventory.header_offset > 0, |this| {
                        this.child(div().text_xs().text_color(muted).child(format!(
                            "已从备用卷头恢复元数据（区域内偏移 {}）。",
                            inventory.header_offset
                        )))
                    })
                    .child(
                        RadioGroup::vertical("filevault-volume")
                            .children(labels)
                            .selected_index(selected)
                            .on_change(cx.listener(|this, index: &usize, _, cx| {
                                if let InventoryState::Loaded(inventory) = &this.inventory {
                                    if let Some(volume) = inventory.volumes.get(*index) {
                                        this.logical_volume = volume.index;
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .when_some(self.selected_volume(), |this, volume| {
                        let status = match volume.conversion_status.as_deref() {
                            Some("Complete") => "已完成加密".to_owned(),
                            Some(other) => other.to_owned(),
                            None => "未知".to_owned(),
                        };
                        let protectors: Vec<&str> = volume
                            .protectors
                            .iter()
                            .map(|kind| match kind.as_str() {
                                "password" => "密码",
                                "recovery" => "个人恢复",
                                "institutional" => "机构恢复",
                                "unknown" => "未知",
                                other => other,
                            })
                            .collect();
                        let weak = volume
                            .findings
                            .iter()
                            .any(|finding| finding["code"].as_str() == Some("FVDE-WEAK-KDF-ITERATIONS"));
                        this.child(div().text_xs().text_color(muted).child(format!(
                            "状态：{status} · PBKDF2 最低迭代数：{} 次 · 保护器：{}",
                            format::count(volume.pbkdf2_iterations),
                            if protectors.is_empty() { "未发现".to_owned() } else { protectors.join("、") }
                        )))
                        .when(weak, |this| {
                            this.child(div().text_xs().text_color(cx.theme().warning).child(
                                "密码派生迭代数低于 20,000；这是强度观察项，不能据此判断卷已被破解。",
                            ))
                        })
                    })
                }
                InventoryState::Unread | InventoryState::Loading => this,
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("支持 CoreStorage / FileVault 2。APFS 原生加密和未完成加密的卷暂不支持。"),
            )
    }
}

impl Render for ContainerDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let scheme = self.scheme;
        let mut regions: Vec<SharedString> = vec!["整个镜像 / 容器".into()];
        regions.extend(self.info.partitions.iter().map(|partition| {
            SharedString::from(format!(
                "分区 {} · {}",
                partition.id,
                partition
                    .label
                    .clone()
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| filesystem_label(&partition.kind))
            ))
        }));
        let master = self.uses_master_key();
        let key_error = master.then(|| self.key_error(cx)).flatten();
        v_flex()
            .w_full()
            .gap_3()
            .child(ui::section_title("加密格式", cx))
            .child(
                RadioGroup::horizontal("container-scheme")
                    .children(SCHEMES.iter().map(|(_, _, label)| *label))
                    .selected_index(
                        SCHEMES
                            .iter()
                            .position(|(candidate, _, _)| *candidate == scheme),
                    )
                    .on_change(cx.listener(|this, index: &usize, window, cx| {
                        if let Some((scheme, _, _)) = SCHEMES.get(*index) {
                            this.scheme = *scheme;
                            this.key_touched = false;
                            for input in [&this.password, &this.key] {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.notify();
                        }
                    })),
            )
            .when(self.fixed_partition.is_none(), |this| {
                this.child(ui::section_title("加密区域", cx)).child(
                    RadioGroup::vertical("container-region")
                        .children(regions)
                        .selected_index(Some(self.region))
                        .on_change(cx.listener(|this, index: &usize, _, cx| {
                            this.region = *index;
                            this.inventory = InventoryState::Unread;
                            cx.notify();
                        })),
                )
            })
            .when(scheme == Scheme::FileVault, |this| {
                this.child(ui::section_title("外部加密上下文与逻辑卷", cx))
                    .child(self.render_filevault(cx))
            })
            .child(ui::section_title("解锁凭据", cx))
            .when(scheme != Scheme::VeraCrypt, |this| {
                this.child(
                    RadioGroup::horizontal("container-credential")
                        .children(["密码", "主密钥"])
                        .selected_index(Some(usize::from(self.master_key)))
                        .on_change(cx.listener(|this, index: &usize, window, cx| {
                            this.master_key = *index == 1;
                            this.key_touched = false;
                            for input in [&this.password, &this.key] {
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.notify();
                        })),
                )
            })
            .map(|this| {
                if master {
                    this.child(Input::new(&self.key).mask_toggle())
                        .child(match key_error {
                            Some(error) => ui::error_text(error, cx).text_xs(),
                            None => div().text_xs().text_color(muted).child(
                                if scheme == Scheme::FileVault {
                                    "16 字节 VMK 或 32 字节 XTS 密钥对；会验证解密后的卷头。"
                                } else {
                                    "填写已恢复的卷主密钥，可粘贴带空格的十六进制字节。"
                                },
                            ),
                        })
                } else {
                    this.child(div().text_xs().text_color(muted).child(
                        if scheme == Scheme::FileVault {
                            "密码 / 个人恢复密码"
                        } else {
                            "密码"
                        },
                    ))
                    .child(Input::new(&self.password).mask_toggle())
                }
            })
            .when(scheme == Scheme::VeraCrypt, |this| {
                this.child(
                    h_flex()
                        .gap_3()
                        .child(div().text_xs().text_color(muted).child("PIM（0 为默认）"))
                        .child(
                            div()
                                .w(gpui_kit::px(96.))
                                .child(Input::new(&self.pim).small()),
                        )
                        .child(
                            Checkbox::new("veracrypt-hidden")
                                .label("打开隐藏卷")
                                .checked(self.hidden)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.hidden = *checked;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("start-container-unlock")
                        .primary()
                        .label("开始后台解锁")
                        .disabled(!self.can_unlock(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_recovery_key, normalize_recovery_key, raw_key_error, KeyKind, RecoveryKeyCheck,
    };

    const VALID: &str = "000011-000022-000033-000044-000055-000066-000077-000088";

    #[test]
    fn recovery_keys_normalize_pasted_separators() {
        assert_eq!(
            normalize_recovery_key(" 000011 000022–000033\t000044 "),
            "000011-000022-000033-000044"
        );
        assert_eq!(normalize_recovery_key("000011000022"), "000011-000022");
        assert_eq!(normalize_recovery_key(""), "");
    }

    #[test]
    fn a_complete_recovery_key_passes() {
        assert_eq!(
            check_recovery_key(VALID),
            RecoveryKeyCheck {
                normalized: VALID.to_owned(),
                valid_groups: 8,
                group_error: None,
                format_error: None,
            }
        );
    }

    #[test]
    fn recovery_key_errors_name_the_group() {
        let checksum = check_recovery_key("000012");
        assert_eq!(
            checksum.group_error.as_deref(),
            Some("第 1 组「000012」校验失败（应能被 11 整除），请检查是否输错")
        );
        let short = check_recovery_key("000011-0000");
        assert_eq!(short.valid_groups, 1);
        assert_eq!(
            short.format_error.as_deref(),
            Some("第 2 组只有 4 位，应为 6 位")
        );
        let range = check_recovery_key("720896");
        assert_eq!(
            range.group_error.as_deref(),
            Some("第 1 组「720896」超出有效范围（不大于 720885）")
        );
        let missing = check_recovery_key("000011");
        assert_eq!(
            missing.format_error.as_deref(),
            Some("还差 7 组（共 8 组 6 位数字）")
        );
        let extra = check_recovery_key(&format!("{VALID}-000099"));
        assert_eq!(
            extra.group_error.as_deref(),
            Some("恢复密钥只有 8 组，请删除多出的第 9 组")
        );
        let letters = check_recovery_key("00001a");
        assert_eq!(
            letters.group_error.as_deref(),
            Some("只能输入数字，第 1 组含有其他字符")
        );
    }

    #[test]
    fn raw_keys_follow_their_lengths() {
        let vmk = "ab".repeat(32);
        assert_eq!(raw_key_error(KeyKind::Vmk, &vmk, ""), None);
        assert_eq!(
            raw_key_error(KeyKind::Vmk, &"ab".repeat(16), ""),
            Some("密钥长度与所选类型不符。")
        );
        assert_eq!(
            raw_key_error(KeyKind::Fvek, &"ab".repeat(16), &"cd".repeat(16)),
            None
        );
        assert_eq!(
            raw_key_error(KeyKind::Fvek, &"ab".repeat(16), "cd"),
            Some("TWEAK 应为 16 或 32 字节的十六进制密钥。")
        );
        assert_eq!(
            raw_key_error(KeyKind::Fvek, "xyz", ""),
            Some("请输入有效的十六进制密钥。")
        );
    }
}
