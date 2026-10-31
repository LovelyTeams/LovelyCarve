//! 活动案件:台账连接、台账行的读缓存、卷文件的发布与回收。
//!
//! 一个案件就是一个目录(布局见 [`crate::core::casework::case_paths`]),身份与一切登记
//! 都在台账 `case.db` 里;[`CaseState`] 只缓存台账中的检材与卷记录,外加索引
//! 不可用时回退用的内存快照体。同一时间只有一个活动案件
//! (`AppState::active_case`)。
//!
//! 生命周期:[`create_case`] 建目录、加进程锁、建台账;[`open_case`] 加锁、
//! 校验台账、载入缓存并自愈(`case_heal.rs`:清空 `staging/`、删除不属于任何
//! 检材的目录、回收记录之外的旧代卷文件、修正指向缺失文件的记录);[`CaseState::close`] 记审计
//! 后释放连接与锁;[`remove_case`] 只删除案件自己的文件,目录里的其它内容
//! 原样保留。
//!
//! 案件内容的每一次变化(建案、开关、改名、锁定、检材增删)都与对应的审计
//! 记录在同一个台账事务里提交(`core::casework::case_journal`)。线上形态的类型在
//! `case_types.rs`。

mod evidence;
mod volumes;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rusqlite::Connection;

use crate::core::casework::case_journal::{self, Event};
use crate::core::casework::case_ledger::{self, CaseProfile};
use crate::core::casework::case_ledger_evidence::{
    self as ledger_evidence, EvidenceRecord, Registration,
};
use crate::core::casework::case_ledger_volumes::{
    self as ledger_volumes, IndexOutcome, SnapshotOutcome, SnapshotPublication, VolumeRecord,
};
use crate::core::casework::case_lock::CaseLock;
use crate::core::casework::case_paths;
use crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshot;

#[path = "case_types.rs"]
mod types;

use types::{evidence_ref_from_record, source_format_name};
pub use types::{
    snapshot_ref, CaseInfo, CreateCaseOptions, EvidenceRef, EvidenceRegistration,
    EvidenceRemovalCleanup, SnapshotInfo, SnapshotPublish, SnapshotRef,
};

/// 活动案件。
pub struct CaseState {
    pub dir: PathBuf,
    ledger: Connection,
    profile: CaseProfile,
    evidence: Vec<EvidenceRecord>,
    volumes: Vec<VolumeRecord>,
    /// 索引不可用时回退用的内存快照体,按 `(image_id, volume_key)`。
    pub snapshots: HashMap<(String, String), Arc<VolumeSnapshot>>,
    _lock: CaseLock,
}

impl std::fmt::Debug for CaseState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaseState")
            .field("dir", &self.dir)
            .field("case_number", &self.profile.case_number)
            .field("name", &self.profile.name)
            .field("evidence", &self.evidence.len())
            .field("volumes", &self.volumes.len())
            .field("loaded_snapshots", &self.snapshots.len())
            .finish_non_exhaustive()
    }
}

/// 默认案件编号 `LC-YYYYMMDD-xxxxxx`。
fn default_case_number() -> String {
    let date = chrono::Utc::now().format("%Y%m%d");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("LC-{date}-{}", &suffix[..6])
}

/// 目录里是否有旧版(`.carve`)案件,用于给出明确的不支持提示。
fn has_legacy_case(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "carve")
        })
    })
}

fn remove_file_quietly(path: &Path, label: &str) {
    match case_paths::remove_database_files(path) {
        Ok(()) => {}
        Err(error) => tracing::debug!("case: remove {label} {} deferred: {error}", path.display()),
    }
}

impl CaseState {
    pub fn ledger_path(&self) -> PathBuf {
        case_paths::ledger_path(&self.dir)
    }

    pub fn profile(&self) -> &CaseProfile {
        &self.profile
    }

    pub fn is_locked(&self) -> bool {
        self.profile.locked
    }

    pub fn info(&self) -> CaseInfo {
        CaseInfo {
            id: self.profile.case_uid.clone(),
            case_number: self.profile.case_number.clone(),
            name: self.profile.name.clone(),
            dir: self.dir.display().to_string(),
            examiner: self.profile.examiner.clone(),
            description: self.profile.description.clone(),
            locked: self.profile.locked,
            schema_version: case_ledger::SCHEMA_VERSION,
            created_at: self.profile.created_at.clone(),
            updated_at: self.profile.updated_at.clone(),
            evidence: self.evidence_refs(),
            snapshots: self.snapshot_refs(),
        }
    }

    /// 在一个台账写事务里执行 `change` 并追加审计记录,提交后刷新档案缓存。
    fn journaled<T>(
        &mut self,
        event: Event<'_>,
        change: impl FnOnce(&rusqlite::Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let tx = case_ledger::write_transaction(&self.ledger)?;
        let value = change(&tx)?;
        case_journal::append(&tx, &self.profile.case_uid, event)?;
        tx.commit()?;
        self.profile = case_ledger::load_profile(&self.ledger)?;
        Ok(value)
    }

    // ----- profile & settings ----------------------------------------------

    /// 逻辑锁:锁定后界面拒绝编辑。
    pub fn set_locked(&mut self, locked: bool) -> anyhow::Result<()> {
        if self.profile.locked == locked {
            return Ok(());
        }
        let action = if locked { "case.lock" } else { "case.unlock" };
        self.journaled(
            Event {
                action,
                evidence: None,
                detail: "",
            },
            |tx| case_ledger::set_locked(tx, locked),
        )
    }

    pub fn rename(&mut self, name: &str) -> anyhow::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("case name must not be empty");
        }
        let detail = serde_json::json!({ "from": self.profile.name, "to": name }).to_string();
        self.journaled(
            Event {
                action: "case.rename",
                evidence: None,
                detail: &detail,
            },
            |tx| case_ledger::set_name(tx, name),
        )
    }

    /// 关闭案件:记审计、checkpoint 台账 WAL,随后释放连接与进程锁。
    pub fn close(mut self) {
        let name = self.profile.name.clone();
        if let Err(error) = self.journaled(
            Event {
                action: "case.close",
                evidence: None,
                detail: &name,
            },
            |_| Ok(()),
        ) {
            tracing::warn!("case: journal close: {error:#}");
        }
        if let Err(error) = self
            .ledger
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        {
            tracing::warn!("case: checkpoint {} on close: {error}", self.dir.display());
        }
        tracing::info!("case: closed {}", self.dir.display());
    }
}

/// 在 `dir` 建一个新案件。目录里已有案件(包括旧版 `.carve` 案件)时拒绝。
pub fn create_case(dir: &Path, options: CreateCaseOptions) -> anyhow::Result<CaseState> {
    let name = options.name.trim();
    if name.is_empty() {
        anyhow::bail!("case name must not be empty");
    }
    std::fs::create_dir_all(dir)?;
    let ledger_path = case_paths::ledger_path(dir);
    if ledger_path.exists() {
        anyhow::bail!("directory already contains a case: {}", dir.display());
    }
    if has_legacy_case(dir) {
        anyhow::bail!(
            "directory holds a case from an older LovelyCarve layout, which this build no \
             longer supports; choose an empty directory"
        );
    }
    let lock = CaseLock::acquire(dir)?;
    for directory in [case_paths::staging_dir(dir), case_paths::evidence_root(dir)] {
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create {}", directory.display()))?;
    }
    let now = case_ledger::now();
    let profile = CaseProfile {
        case_uid: uuid::Uuid::new_v4().to_string(),
        case_number: options
            .case_number
            .map(|number| number.trim().to_string())
            .filter(|number| !number.is_empty())
            .unwrap_or_else(default_case_number),
        name: name.to_string(),
        examiner: options
            .examiner
            .map(|examiner| examiner.trim().to_string())
            .filter(|examiner| !examiner.is_empty())
            .unwrap_or_else(case_journal::current_actor),
        description: options.description.unwrap_or_default(),
        locked: false,
        created_at: now.clone(),
        updated_at: now,
    };
    let ledger = case_ledger::create(&ledger_path, &profile)?;
    let mut case = CaseState {
        dir: dir.to_path_buf(),
        ledger,
        profile,
        evidence: Vec::new(),
        volumes: Vec::new(),
        snapshots: HashMap::new(),
        _lock: lock,
    };
    let detail = serde_json::json!({
        "caseNumber": case.profile.case_number,
        "name": case.profile.name,
        "examiner": case.profile.examiner,
    })
    .to_string();
    case.journaled(
        Event {
            action: "case.create",
            evidence: None,
            detail: &detail,
        },
        |_| Ok(()),
    )?;
    tracing::info!("case: created {}", dir.display());
    Ok(case)
}

/// 打开 `dir` 里的已有案件。快照体不在这里加载,由命令层按需安装。
pub fn open_case(dir: &Path) -> anyhow::Result<CaseState> {
    let ledger_path = case_paths::ledger_path(dir);
    if !ledger_path.is_file() {
        if has_legacy_case(dir) {
            anyhow::bail!(
                "{} holds a case from an older LovelyCarve layout, which this build no longer \
                 supports; create a new case",
                dir.display()
            );
        }
        anyhow::bail!("no LovelyCarve case (case.db) in {}", dir.display());
    }
    let lock = CaseLock::acquire(dir)?;
    let ledger = case_ledger::open(&ledger_path)?;
    let profile = case_ledger::load_profile(&ledger)?;
    let evidence = ledger_evidence::list(&ledger)?;
    let volumes = ledger_volumes::list(&ledger)?;
    let mut case = CaseState {
        dir: dir.to_path_buf(),
        ledger,
        profile,
        evidence,
        volumes,
        snapshots: HashMap::new(),
        _lock: lock,
    };
    heal::heal_case(&mut case);
    let name = case.profile.name.clone();
    case.journaled(
        Event {
            action: "case.open",
            evidence: None,
            detail: &name,
        },
        |_| Ok(()),
    )?;
    tracing::info!("case: opened {}", dir.display());
    Ok(case)
}

/// 删除案件:只删除案件自己的文件(台账、锁、检材与暂存目录),目录里的
/// 其它内容原样保留,目录空了才删除目录本身。案件正被其它进程打开时失败。
pub fn remove_case(dir: &Path) -> anyhow::Result<()> {
    let ledger_path = case_paths::ledger_path(dir);
    if !ledger_path.is_file() {
        anyhow::bail!("no LovelyCarve case (case.db) in {}", dir.display());
    }
    // 先确认没有进程持有;Windows 上必须先关掉锁句柄才能删除锁文件。
    drop(CaseLock::acquire(dir)?);
    for directory in [case_paths::evidence_root(dir), case_paths::staging_dir(dir)] {
        case_paths::remove_dir_if_exists(&directory)
            .with_context(|| format!("remove {}", directory.display()))?;
    }
    case_paths::remove_database_files(&ledger_path)
        .with_context(|| format!("remove {}", ledger_path.display()))?;
    case_paths::remove_database_files(&case_paths::lock_path(dir))
        .with_context(|| format!("remove case lock in {}", dir.display()))?;
    match std::fs::remove_dir(dir) {
        Ok(()) => {}
        Err(error) => tracing::info!(
            "case: kept directory {} after removing the case: {error}",
            dir.display()
        ),
    }
    tracing::info!("case: removed case in {}", dir.display());
    Ok(())
}

/// 读取(不加锁)案件档案,供案件列表展示。
pub fn peek_case(dir: &Path) -> anyhow::Result<CaseProfile> {
    let ledger = case_ledger::open(&case_paths::ledger_path(dir))?;
    case_ledger::load_profile(&ledger)
}

/// 证据路径的比较键。Windows 文件系统大小写不敏感且两种分隔符等价,
/// `D:\a.E01` 与 `d:/a.e01` 是同一份证据;所有路径身份判断(`image_load`
/// 单飞锁、运行时 id 复用、案件检材查找、台账 `path_key`)都必须经过它,
/// 否则并发加载会造出重复 id。
pub use crate::core::common::evidence_path::evidence_path_key;

/// 一份暂存快照对应的发布信息。卷目录里的文件名由
/// [`CaseState::publish_snapshot`] 移入时填写。
pub fn snapshot_publication(
    vol_id: i64,
    built: &crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshotBuilt,
    fingerprint: &str,
) -> SnapshotPublication {
    SnapshotPublication {
        image_id: built.header.image_id.clone(),
        volume_key: built.header.volume_key.clone(),
        vol_id,
        source_size: built.header.source_volume_size,
        fingerprint: fingerprint.to_string(),
        snapshot_file: String::new(),
        built_at: built.header.built_at.clone(),
        entry_count: built.entry_count,
        truncated: built.truncated,
    }
}

#[path = "case_heal.rs"]
mod heal;

/// 测试用案件:建案、登记检材、发布带真实索引的卷。
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use super::{
        create_case, snapshot_publication, CaseState, CreateCaseOptions, EvidenceRef,
        SnapshotPublish,
    };
    use crate::core::casework::case_paths;
    use crate::core::catalog::filedb::filedb_store::{self, IndexSource};
    use crate::core::catalog::snapshot::volume_snapshot::{
        SnapshotEntry, SnapshotHeader, VolumeSnapshotBuilt, SNAPSHOT_SCHEMA,
    };
    use crate::core::evidence::image::ImageFormat;

    /// A new case in `dir` with the evidence `image_id` registered.
    pub(crate) fn case_with_evidence(dir: &Path, image_id: &str) -> CaseState {
        let mut case = create_case(
            dir,
            CreateCaseOptions {
                name: "test".to_string(),
                ..CreateCaseOptions::default()
            },
        )
        .expect("create case");
        case.register_evidence(&EvidenceRef {
            image_id: image_id.to_string(),
            path: format!("/evidence/{image_id}.E01"),
            source_format: ImageFormat::Ewf,
            size_bytes: 1,
            added_at: "2026-01-01T00:00:00.000Z".to_string(),
            platform: None,
            manual_platform: None,
            evidence_type: None,
        })
        .expect("register evidence");
        case
    }

    /// Publishes a (placeholder) snapshot of `volume_key` built at `built_at`
    /// and the index imported from `entries`; returns the index path.
    pub(crate) fn publish_volume(
        case: &mut CaseState,
        image_id: &str,
        volume_key: &str,
        built_at: &str,
        entries: &[SnapshotEntry],
    ) -> PathBuf {
        let staged = case
            .staging_dir()
            .join(format!("{}.dirz", case_paths::volume_segment(volume_key)));
        std::fs::write(&staged, b"dirz").expect("stage snapshot");
        let built = VolumeSnapshotBuilt {
            header: SnapshotHeader {
                schema_version: SNAPSHOT_SCHEMA,
                volume_key: volume_key.to_string(),
                image_id: image_id.to_string(),
                source_volume_size: 1,
                built_at: built_at.to_string(),
            },
            file_name: String::new(),
            entry_count: entries.len() as u64,
            truncated: false,
        };
        assert!(matches!(
            case.publish_snapshot(&staged, snapshot_publication(0, &built, "fp"))
                .expect("publish snapshot"),
            SnapshotPublish::Published(_)
        ));
        let staged_index = filedb_store::staged_index_path(&case.staging_dir(), volume_key);
        filedb_store::build_index_from_entries(
            entries,
            &staged_index,
            &IndexSource {
                image_id,
                volume_key,
                vol_id: 0,
                built_at,
                fingerprint: "fp",
                truncated: false,
            },
        )
        .expect("build index");
        assert!(case
            .publish_index(image_id, volume_key, built_at, &staged_index)
            .expect("publish index"));
        let record = case.volume(image_id, volume_key).expect("volume record");
        case.index_path(record).expect("index path")
    }
}

#[cfg(test)]
#[path = "case_tests.rs"]
mod tests;
