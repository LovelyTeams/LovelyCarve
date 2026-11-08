//! 案件的线上形态(检材、卷发布、案件视图)与台账行之间的转换。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::casework::case_ledger_evidence::EvidenceRecord;
use crate::casework::case_ledger_volumes::VolumeRecord;
use crate::casework::case_paths;
use crate::evidence::image::ImageFormat;
/// 案件里的一个检材(前端线上形态)。`evidence_type` 是自动识别回写的结论;
/// `platform` 是生效的平台:检验人手动指定的平台,没有时取自动识别结论。
/// 加载器只读这三项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub image_id: String,
    pub path: String,
    pub source_format: ImageFormat,
    pub size_bytes: u64,
    pub added_at: String,
    #[serde(default)]
    pub platform: Option<String>,
    /// 检验人手动指定的平台;`None` 时 `platform` 来自自动识别。
    #[serde(default)]
    pub manual_platform: Option<String>,
    #[serde(default)]
    pub evidence_type: Option<String>,
}

/// 一个卷的发布状态(前端线上形态)。`indexed` 表示有可服务的文件系统索引。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotRef {
    pub image_id: String,
    pub volume_key: String,
    pub entry_count: u64,
    pub truncated: bool,
    pub built_at: String,
    pub source_volume_size: u64,
    pub indexed: bool,
}

/// 卷发布状态加一个新鲜度标记,`stale` 由能看到已加载镜像的命令层计算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    #[serde(flatten)]
    pub snapshot: SnapshotRef,
    pub stale: bool,
}

/// 活动案件的完整视图(前端线上形态)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseInfo {
    /// 案件 uid,建案时生成,移动目录后不变。
    pub id: String,
    pub case_number: String,
    pub name: String,
    pub dir: String,
    pub examiner: String,
    pub description: String,
    /// 逻辑锁:锁定后拒绝改名、移除检材与重建快照。
    pub locked: bool,
    /// 台账结构版本。
    pub schema_version: i32,
    pub created_at: String,
    pub updated_at: String,
    pub evidence: Vec<EvidenceRef>,
    pub snapshots: Vec<SnapshotRef>,
}

/// 建案参数。未给的编号 / 检验人取默认值。
#[derive(Debug, Clone, Default)]
pub struct CreateCaseOptions {
    pub name: String,
    pub case_number: Option<String>,
    pub description: Option<String>,
    pub examiner: Option<String>,
}

/// [`CaseState::register_evidence`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceRegistration {
    /// 首次登记(检材上线:自动识别与首任务由调用方拉起)。
    Added,
    /// 已登记;来源属性有变化时已刷新。
    Known,
    /// 同一来源已是另一个检材,本次不登记。
    SourceTaken { existing_image_id: String },
}

/// 快照发布结果(见 [`CaseState::publish_snapshot`])。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotPublish {
    Published(VolumeRecord),
    /// 案件里已有更新的快照,或检材已不在案件中;暂存文件已删除。
    Discarded,
}

/// 检材移除后的文件清理,在案件锁外的阻塞线程上执行。
pub struct EvidenceRemovalCleanup {
    pub(super) evidence_dir: PathBuf,
}

impl EvidenceRemovalCleanup {
    /// 删除检材目录。Windows 上仍被打开的文件删不掉时短暂重试;仍失败的残留
    /// 在下次打开案件时清理。
    pub fn run(self) {
        for attempt in 0..3 {
            match case_paths::remove_dir_if_exists(&self.evidence_dir) {
                Ok(()) => return,
                Err(error) if attempt < 2 => {
                    tracing::debug!(
                        "case: remove {} (attempt {}): {error}",
                        self.evidence_dir.display(),
                        attempt + 1
                    );
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(error) => tracing::warn!(
                    "case: remove evidence directory {} failed, will retry on next open: {error}",
                    self.evidence_dir.display()
                ),
            }
        }
    }
}

pub(super) fn source_format_name(format: ImageFormat) -> String {
    match serde_json::to_value(format) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(_) | Err(_) => "raw".to_string(),
    }
}

fn parse_source_format(name: &str) -> Option<ImageFormat> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

pub(super) fn evidence_ref_from_record(record: &EvidenceRecord) -> Option<EvidenceRef> {
    Some(EvidenceRef {
        image_id: record.uid.clone(),
        path: record.path.clone(),
        source_format: parse_source_format(&record.format)?,
        size_bytes: record.size_bytes,
        added_at: record.added_at.clone(),
        platform: record
            .manual_platform
            .clone()
            .or_else(|| record.platform.clone()),
        manual_platform: record.manual_platform.clone(),
        evidence_type: record.kind.clone(),
    })
}

/// The wire form of a volume record.
pub fn snapshot_ref(record: &VolumeRecord) -> SnapshotRef {
    SnapshotRef {
        image_id: record.image_id.clone(),
        volume_key: record.volume_key.clone(),
        entry_count: record.entry_count,
        truncated: record.truncated,
        built_at: record.built_at.clone(),
        source_volume_size: record.source_size,
        indexed: record.index_file.is_some(),
    }
}
