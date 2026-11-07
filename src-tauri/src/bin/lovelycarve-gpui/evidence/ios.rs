//! iOS 备份: an iTunes / Finder backup's device details, its read-only
//! unlock into a plaintext session and the upstream audit of its manifest,
//! data files and state.
use std::{collections::BTreeSet, rc::Rc};

use gpui_kit::{
    component::{
        accordion::Accordion,
        alert::Alert,
        button::{Button, ButtonVariants as _},
        description_list::DescriptionList,
        group_box::{GroupBox, GroupBoxVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::evidence::{image as image_commands, ios_backup as ios_commands},
    core::evidence::image::ImageInfo,
    host::State,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::EvidenceTab;
use crate::{backend::Backend, format, tasks::TaskStore, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// Where an upstream finding came from.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct FindingSource {
    analyzer: String,
    scope: String,
    version: Option<String>,
}

/// One field a finding cites.
#[derive(Clone, Debug, Deserialize)]
struct FindingEvidence {
    field: String,
    #[serde(default)]
    value: Value,
}

/// An upstream observation; unrated findings keep a `None` severity.
#[derive(Clone, Debug, Deserialize)]
struct Finding {
    code: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    source: FindingSource,
    #[serde(default)]
    evidence: Vec<FindingEvidence>,
}

/// The audit result: the findings to show and the upstream JSON to export
/// unchanged.
struct Audit {
    findings: Vec<Finding>,
    raw: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Unlock,
    Audit,
}

/// A finding code as the interface names it; unknown codes stay as they are.
fn finding_title(code: &str) -> &str {
    match code {
        "IOS-BACKUP-ENCRYPTED" => "备份加密状态",
        "IOS-BACKUP-UNENCRYPTED" => "未加密备份",
        "IOS-BACKUP-BLOB-MISSING" => "清单引用的数据文件缺失",
        "IOS-BACKUP-BLOB-ORPHAN" => "未被清单引用的数据文件",
        "IOS-BACKUP-FILEID-MISMATCH" => "文件标识与路径哈希不一致",
        "IOS-BACKUP-DOMAIN-ABSENT" => "预期应用域未包含在备份中",
        "IOS-BACKUP-NOT-FINISHED" => "备份未记录为完成",
        "IOS-BACKUP-NO-FILE-ROWS" => "清单没有文件记录",
        "IOS-BACKUP-ENCRYPTION-STATE-CONTRADICTION" => "加密声明与清单内容不一致",
        other => other,
    }
}

fn severity_tag(severity: Option<&str>) -> Tag {
    let (tag, label) = match severity {
        Some("Critical") => (Tag::danger(), "严重"),
        Some("High") => (Tag::danger(), "高"),
        Some("Medium") => (Tag::warning(), "中"),
        Some("Low") => (Tag::warning(), "低"),
        Some("Info") => (Tag::info(), "信息"),
        Some(_) | None => (Tag::secondary(), "未评级"),
    };
    tag.small().child(label)
}

/// A cited value: strings as written, anything else as JSON.
fn evidence_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub struct IosBackupCard {
    info: Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    password: Entity<InputState>,
    /// The running operation and its task.
    busy: Option<(Operation, String)>,
    cancelling: bool,
    error: Option<SharedString>,
    audit: Option<Audit>,
    open: BTreeSet<usize>,
    tasks: Entity<TaskStore>,
    _subscriptions: Vec<Subscription>,
}

impl IosBackupCard {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("备份密码")
        });
        let subscriptions = vec![cx.subscribe_in(
            &password,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.run(Operation::Unlock, window, cx),
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                InputEvent::Focus | InputEvent::Blur => {}
            },
        )];
        Self {
            info,
            tab,
            password,
            busy: None,
            cancelling: false,
            error: None,
            audit: None,
            open: BTreeSet::new(),
            tasks: TaskStore::global(cx),
            _subscriptions: subscriptions,
        }
    }

    fn run(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let task_id = uuid::Uuid::new_v4().to_string();
        let image_id = self.info.id.clone();
        self.busy = Some((operation, task_id.clone()));
        self.cancelling = false;
        self.error = None;
        let backend = Backend::global(cx);
        match operation {
            Operation::Unlock => {
                let password = self.password.read(cx).value().to_string();
                self.password
                    .update(cx, |input, cx| input.set_value("", window, cx));
                let unlocking = backend.run(move |_, state| async move {
                    ios_commands::ios_backup_unlock(
                        State::new(&state),
                        image_id,
                        password,
                        Some(task_id),
                    )
                    .await
                });
                cx.spawn_in(window, async move |this, cx| {
                    let unlocked = unlocking.await;
                    this.update_in(cx, |this, _, cx| {
                        let cancelled = this.cancelling;
                        this.busy = None;
                        this.cancelling = false;
                        match unlocked {
                            Ok(image) if !cancelled => {
                                this.tab
                                    .update(cx, |tab, cx| tab.open_image(image.path, cx))
                                    .ok();
                            }
                            // Cancelled after the session was made: drop it.
                            Ok(image) => {
                                let unloading =
                                    Backend::global(cx).run(move |_, state| async move {
                                        image_commands::image_unload(State::new(&state), image.id)
                                            .await
                                    });
                                cx.background_spawn(async move {
                                    if let Err(error) = unloading.await {
                                        tracing::warn!("unload cancelled iOS session: {error}");
                                    }
                                })
                                .detach();
                            }
                            Err(error) if error.cancelled => {}
                            Err(error) => this.error = Some(error.message),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Operation::Audit => {
                let auditing = backend.run(move |_, state| async move {
                    ios_commands::ios_backup_audit(State::new(&state), image_id, Some(task_id))
                        .await
                });
                cx.spawn_in(window, async move |this, cx| {
                    let audited = auditing.await;
                    this.update_in(cx, |this, _, cx| {
                        let cancelled = this.cancelling;
                        this.busy = None;
                        this.cancelling = false;
                        match audited {
                            Ok(_) if cancelled => {}
                            Ok(raw) => match serde_json::from_value::<Vec<Finding>>(raw.clone()) {
                                Ok(findings) => {
                                    this.open.clear();
                                    this.audit = Some(Audit { findings, raw });
                                }
                                Err(error) => this.error = Some(error.to_string().into()),
                            },
                            Err(error) if error.cancelled => {}
                            Err(error) => this.error = Some(error.message),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some((_, task_id)) = self.busy.clone() else {
            return;
        };
        self.cancelling = true;
        self.tasks.update(cx, |tasks, cx| tasks.cancel(task_id, cx));
        cx.notify();
    }

    /// Save the findings as the JSON envelope the other exports use.
    fn export_findings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(audit) = &self.audit else {
            return;
        };
        let envelope = json!({
            "exported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "title": "iOS 备份检查",
            "data": audit.raw,
            "meta": {
                "image_id": self.info.id,
                "evidence_path": self.info.path,
                "backup": self.info.ios_backup,
            },
        });
        let prompt = ui::prompt_save("ios-backup-audit.json", cx);
        cx.spawn_in(window, async move |_, cx| {
            let Some(destination) = prompt.await else {
                return;
            };
            let written = serde_json::to_string_pretty(&envelope)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    std::fs::write(&destination, text).map_err(|error| error.to_string())
                });
            cx.update(|window, cx| match written {
                Ok(()) => ui::notify_success(
                    window,
                    cx,
                    "已导出 iOS 备份检查结果",
                    destination.display().to_string(),
                ),
                Err(error) => ui::notify_error(window, cx, "导出失败", error),
            })
            .ok();
        })
        .detach();
    }

    fn render_findings(&self, audit: &Audit, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let accordion = audit
            .findings
            .iter()
            .enumerate()
            .fold(
                Accordion::new("ios-findings").multiple(true),
                |accordion, (index, finding)| {
                    let open = self.open.contains(&index);
                    let source = [
                        Some(finding.source.analyzer.as_str()),
                        finding.source.version.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                    let source = [source.as_str(), finding.source.scope.as_str()]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    accordion.item(|item| {
                        item.open(open)
                            .title(
                                h_flex()
                                    .gap_2()
                                    .child(severity_tag(finding.severity.as_deref()))
                                    .child(finding_title(&finding.code).to_owned()),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(ui::mono(finding.code.clone(), cx).text_xs())
                                    .when(!finding.note.is_empty(), |this| {
                                        this.child(finding.note.clone())
                                    })
                                    .children(finding.evidence.iter().map(|evidence| {
                                        ui::field_row(
                                            evidence.field.clone(),
                                            ui::mono(evidence_value(&evidence.value), cx).text_xs(),
                                            cx,
                                        )
                                    }))
                                    .when(!source.is_empty(), |this| {
                                        this.child(div().text_xs().text_color(muted).child(source))
                                    }),
                            )
                    })
                },
            )
            .on_toggle_click(cx.listener(|this, open: &[usize], _, cx| {
                this.open = open.iter().copied().collect();
                cx.notify();
            }));
        v_flex()
            .gap_2()
            .child(div().text_xs().text_color(muted).child(format!(
                "共 {} 项观察。应用域缺失仅说明备份未包含该域，原因需结合其他证据判断；原始观察与字段保留上游表述。",
                format::count(audit.findings.len() as u64)
            )))
            .child(accordion)
    }
}

impl Render for IosBackupCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(backup) = self.info.ios_backup.clone() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let busy = self.busy.as_ref().map(|(operation, _)| *operation);
        let status = if backup.locked {
            Tag::warning().small().child("待解锁")
        } else if backup.observed_encrypted == Some(true) {
            Tag::success().small().child("已解锁")
        } else {
            Tag::secondary().small().child("未加密")
        };
        let title = h_flex()
            .w_full()
            .gap_2()
            .child(Icon::new(ExtraIcon::Smartphone))
            .child("iOS 备份")
            .child(status)
            .child(div().flex_1())
            .when(!backup.locked, |this| {
                this.child(
                    Button::new("ios-audit")
                        .small()
                        .icon(Icon::new(ExtraIcon::ShieldCheck))
                        .label("检查备份")
                        .disabled(busy.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run(Operation::Audit, window, cx)
                        })),
                )
            })
            .when(self.audit.is_some(), |this| {
                this.child(
                    Button::new("ios-export-findings")
                        .small()
                        .icon(Icon::new(ExtraIcon::Download))
                        .label("导出 JSON")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.export_findings(window, cx)),
                        ),
                )
            });
        let unrecorded = || "未记录".to_owned();
        let details = DescriptionList::horizontal()
            .columns(1)
            .bordered(false)
            .label_width(gpui_kit::px(96.))
            .item(
                "设备",
                backup.device_name.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "型号",
                backup.product_type.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "iOS 版本",
                backup.product_version.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "序列号",
                backup.serial_number.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "设备标识",
                backup.unique_identifier.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "备份状态",
                backup.snapshot_state.clone().unwrap_or_else(unrecorded),
                1,
            )
            .item(
                "清单记录",
                backup
                    .file_count
                    .map(|count| format::count(count as u64))
                    .unwrap_or_else(unrecorded),
                1,
            );
        let unreadable = backup.unreadable_rows.filter(|rows| *rows > 0).map(|rows| {
            Alert::warning(
                "ios-unreadable",
                format!(
                    "清单有 {} 条记录无法识别，当前文件树并不完整；请结合原始 Manifest.db 复核。",
                    format::count(rows as u64)
                ),
            )
        });
        let unresolved = (backup.unresolved_sizes > 0).then(|| {
            Alert::warning(
                "ios-unresolved",
                format!(
                    "{} 个文件未记录长度且无法读取数据，列表以 0 显示；请结合原始清单和数据文件复核。",
                    format::count(backup.unresolved_sizes as u64)
                ),
            )
        });
        let unlock = backup.locked.then(|| {
            v_flex()
                .gap_2()
                .child(div().text_sm().child("备份密码"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.password)
                                    .mask_toggle()
                                    .disabled(busy.is_some()),
                            ),
                        )
                        .child(
                            Button::new("ios-unlock")
                                .primary()
                                .label("解锁并打开")
                                .disabled(busy.is_some())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.run(Operation::Unlock, window, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("解锁后打开只读会话，关闭来源检材或退出应用后需重新输入密码。"),
                )
        });
        let operation = busy.map(|operation| {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(Spinner::new().small())
                        .child(match operation {
                            Operation::Unlock => "正在派生密钥并读取清单…",
                            Operation::Audit => "正在检查清单、数据文件与备份状态…",
                        })
                        .child(div().flex_1())
                        .child(
                            Button::new("ios-cancel")
                                .small()
                                .label(if self.cancelling {
                                    "正在取消…"
                                } else {
                                    "取消"
                                })
                                .disabled(self.cancelling)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                )
                .when(operation == Operation::Unlock, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("密钥派生可能需要较长时间，取消会在当前派生阶段结束后生效。"),
                    )
                })
        });
        GroupBox::new()
            .id("ios-backup")
            .outline()
            .title(title)
            .child(
                v_flex()
                    .gap_3()
                    .child(details)
                    .children(unreadable)
                    .children(unresolved)
                    .children(unlock)
                    .children(operation)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(ui::error_text(error, cx))
                    })
                    .when_some(self.audit.as_ref(), |this, audit| {
                        this.child(self.render_findings(audit, cx))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{evidence_value, finding_title, Finding};

    #[test]
    fn findings_read_the_upstream_shape() {
        let findings: Vec<Finding> = serde_json::from_value(json!([{
            "code": "IOS-BACKUP-BLOB-MISSING",
            "note": "blob missing",
            "severity": null,
            "category": "integrity",
            "source": { "analyzer": "ios-backup-forensic", "scope": "backup", "version": null },
            "evidence": [{ "field": "file_id", "value": "abc", "location": null }],
            "subjects": [],
            "context": null,
        }]))
        .expect("findings");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, None);
        assert_eq!(finding_title(&findings[0].code), "清单引用的数据文件缺失");
        assert_eq!(evidence_value(&findings[0].evidence[0].value), "abc");
        assert_eq!(evidence_value(&json!(3)), "3");
        assert_eq!(finding_title("IOS-NEW-CODE"), "IOS-NEW-CODE");
    }
}
