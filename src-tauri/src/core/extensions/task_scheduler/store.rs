//! 任务持久化:案件台账的 `task` 表(结构见 `core::casework::case_ledger`)。
//!
//! 台账是唯一事实源,调度器进程内只保留运行句柄。连接独立于 `CaseState`
//! 的连接(WAL 多连接),调度循环与 UI 命令互不阻塞;调度器只在阻塞线程池
//! 上调用本模块。
//!
//! 多条任务并行时,执行提交、控制面与看门狗会同时改写任务行:状态迁移
//! ([`TaskStore::transition`] / [`TaskStore::complete`])在一个 `BEGIN IMMEDIATE`
//! 事务里先校验当前状态再写,已离开预期状态的行不会被旧的一方覆盖。单实例
//! function 每个作业(function, 检材, 目标)最多一条未终结任务:建任务时在
//! 同一写事务里检查;整个检材的作业另由台账的部分唯一索引
//! `task_single_active` 兜底。台账的 `can_pause` / `depends_on` 列只在旧案件
//! 的任务行里有值。

use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::model::{NewTask, SchedTaskRecord, SchedTaskStatus, TaskInfo, TaskParams, TaskResult};
use crate::core::casework::case_ledger::{self, now, seconds_between};
use crate::core::extensions::plugin::TaskTrigger;

const TASK_COLUMNS: &str = "t.id, t.function, t.display_name, t.kind, t.evidence_id, e.uid, \
t.plugin_id, t.analyzer, t.trigger, t.parent_id, t.error, t.progress, t.params, t.result, \
t.depends_on, t.status, t.can_pause, t.single_instance, t.internal, t.remain_secs, \
t.created_at, t.started_at, t.finished_at, t.updated_at";

const TASK_SOURCE: &str = "task t JOIN evidence e ON e.id = t.evidence_id";

/// 未终结的状态(单实例约束的作用范围,与台账索引 `task_single_active` 一致)。
const UNFINISHED: &str = "'created', 'queued', 'running', 'pause', 'offline'";

/// 建任务结果:新建,或命中单实例约束返回已存在的未终结任务。
#[derive(Clone, Debug, PartialEq)]
pub enum InsertOutcome {
    Created(SchedTaskRecord),
    Existing(SchedTaskRecord),
}

impl InsertOutcome {
    pub fn record(&self) -> &SchedTaskRecord {
        match self {
            Self::Created(record) | Self::Existing(record) => record,
        }
    }

    pub fn into_record(self) -> SchedTaskRecord {
        match self {
            Self::Created(record) | Self::Existing(record) => record,
        }
    }
}

/// 任务列表过滤:检材与内部任务标志。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskFilter {
    pub evidence_id: Option<i64>,
    pub include_internal: bool,
}

pub struct TaskStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for TaskStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TaskStore").finish_non_exhaustive()
    }
}

fn trigger_name(trigger: TaskTrigger) -> &'static str {
    match trigger {
        TaskTrigger::Auto => "auto",
        TaskTrigger::Manually => "manually",
    }
}

fn parse_trigger(text: &str) -> Option<TaskTrigger> {
    match text {
        "auto" => Some(TaskTrigger::Auto),
        "manually" => Some(TaskTrigger::Manually),
        _ => None,
    }
}

impl TaskStore {
    pub fn open(ledger_path: &Path) -> anyhow::Result<Self> {
        let conn = case_ledger::open(ledger_path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(super) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ----- task -------------------------------------------------------------

    /// Inserts a task, or returns the unfinished task of the same job
    /// (function, evidence, target) when the function is single-instance.
    /// The ledger's unique index backs this up for whole-evidence jobs;
    /// jobs on a system or a file are checked here, inside the same write
    /// transaction.
    pub fn insert_task(&self, new: &NewTask) -> anyhow::Result<InsertOutcome> {
        let conn = self.conn();
        let tx = case_ledger::write_transaction(&conn)?;
        if new.single_instance {
            let existing = {
                let mut statement = tx.prepare(&format!(
                    "SELECT {TASK_COLUMNS} FROM {TASK_SOURCE}
                     WHERE t.function = ?1 AND t.evidence_id = ?2
                       AND t.status IN ({UNFINISHED})
                     ORDER BY t.id DESC"
                ))?;
                let rows =
                    statement.query_map(params![new.name, new.evidence_id], task_from_row)?;
                let mut found = None;
                for row in rows {
                    let task = row.context("query single-instance task")?;
                    if task.params.target_key() == new.params.target_key() {
                        found = Some(task);
                        break;
                    }
                }
                found
            };
            if let Some(existing) = existing {
                tx.commit()?;
                return Ok(InsertOutcome::Existing(existing));
            }
        }
        let indexed_single_instance = new.single_instance && new.params.target_key().is_empty();
        let now = now();
        tx.execute(
            "INSERT INTO task (evidence_id, function, plugin_id, analyzer, display_name, kind,
                               depends_on, trigger, parent_id, params, status, progress,
                               can_pause, single_instance, internal, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10, 0, 0, ?11, ?12, ?13, ?13)",
            params![
                new.evidence_id,
                new.name,
                new.plugin_id,
                new.analyzer,
                new.display_name,
                new.type_name,
                trigger_name(new.trigger),
                new.parent_tid,
                serde_json::to_string(&new.params)?,
                SchedTaskStatus::Created.as_str(),
                indexed_single_instance,
                new.internal,
                now,
            ],
        )
        .context("insert task")?;
        let id = tx.last_insert_rowid();
        let record = query_task(&tx, id)?.context("inserted task row vanished")?;
        tx.commit()?;
        Ok(InsertOutcome::Created(record))
    }

    pub fn get(&self, task_id: i64) -> anyhow::Result<Option<SchedTaskRecord>> {
        query_task(&self.conn(), task_id)
    }

    pub fn list(&self, filter: &TaskFilter) -> anyhow::Result<Vec<SchedTaskRecord>> {
        let conn = self.conn();
        let mut sql = format!("SELECT {TASK_COLUMNS} FROM {TASK_SOURCE} WHERE 1 = 1");
        let mut values: Vec<i64> = Vec::new();
        if let Some(evidence_id) = filter.evidence_id {
            sql.push_str(" AND t.evidence_id = ?");
            values.push(evidence_id);
        }
        if !filter.include_internal {
            sql.push_str(" AND t.internal = 0");
        }
        sql.push_str(" ORDER BY t.id");
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), task_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().context("list tasks")
    }

    pub fn list_by_status(&self, status: SchedTaskStatus) -> anyhow::Result<Vec<SchedTaskRecord>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM {TASK_SOURCE} WHERE t.status = ?1 ORDER BY t.id"
        ))?;
        let rows = statement.query_map(params![status.as_str()], task_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("list tasks by status")
    }

    pub fn list_for_evidence(&self, evidence_id: i64) -> anyhow::Result<Vec<SchedTaskRecord>> {
        self.list(&TaskFilter {
            evidence_id: Some(evidence_id),
            include_internal: true,
        })
    }

    /// `(status, target key)` of every task of `function` on the evidence.
    fn job_states(
        &self,
        function: &str,
        evidence_id: i64,
    ) -> anyhow::Result<Vec<(SchedTaskStatus, String)>> {
        let conn = self.conn();
        let mut statement = conn
            .prepare("SELECT status, params FROM task WHERE function = ?1 AND evidence_id = ?2")?;
        let rows = statement.query_map(params![function, evidence_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut states = Vec::new();
        for row in rows {
            let (status, params) = row?;
            let status = SchedTaskStatus::parse(&status)
                .ok_or_else(|| anyhow::anyhow!("unknown task status {status:?}"))?;
            let params: TaskParams = serde_json::from_str(&params).unwrap_or_default();
            states.push((status, params.target_key().to_owned()));
        }
        Ok(states)
    }

    /// Whether the job (function, evidence, target) ever had a task.
    pub fn has_task(
        &self,
        function: &str,
        evidence_id: i64,
        target_key: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .job_states(function, evidence_id)?
            .iter()
            .any(|(_, target)| target == target_key))
    }

    /// Targets on which `function` succeeded on the evidence.
    pub fn succeeded_targets(
        &self,
        function: &str,
        evidence_id: i64,
    ) -> anyhow::Result<Vec<String>> {
        Ok(self
            .job_states(function, evidence_id)?
            .into_iter()
            .filter(|(status, _)| *status == SchedTaskStatus::Success)
            .map(|(_, target)| target)
            .collect())
    }

    /// 状态迁移:`running` 首次写开始时间;终态写结束时间(耗时由两者结算);
    /// `error` 记失败/离线原因。返回更新后的行。
    pub fn update_status(
        &self,
        task_id: i64,
        status: SchedTaskStatus,
        error: Option<&str>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        self.write_if(task_id, |_| true, status, error, None)
    }

    /// [`Self::update_status`],但只在任务仍处于 `expected` 之一时生效
    /// (读取与写入在同一个写事务里)。任务已被别处改到其他状态时返回
    /// `None`,不覆盖较新的状态。
    pub fn transition(
        &self,
        task_id: i64,
        expected: &[SchedTaskStatus],
        status: SchedTaskStatus,
        error: Option<&str>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        self.write_if(
            task_id,
            |current| expected.contains(&current.status),
            status,
            error,
            None,
        )
    }

    /// 执行提交:仍为 `running` 的任务以一条 UPDATE 同时写入终态与
    /// `result`。已被取消/暂停/判离线的任务返回 `None`,结果丢弃。
    pub fn complete(
        &self,
        task_id: i64,
        status: SchedTaskStatus,
        result: Option<&TaskResult>,
        error: Option<&str>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        self.write_if(
            task_id,
            |current| current.status == SchedTaskStatus::Running,
            status,
            error,
            result,
        )
    }

    fn write_if(
        &self,
        task_id: i64,
        allowed: impl FnOnce(&SchedTaskRecord) -> bool,
        status: SchedTaskStatus,
        error: Option<&str>,
        result: Option<&TaskResult>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        let conn = self.conn();
        let tx = case_ledger::write_transaction(&conn)?;
        let Some(current) = query_task(&tx, task_id)? else {
            return Ok(None);
        };
        if !allowed(&current) {
            return Ok(None);
        }
        let written = write_status(&tx, &current, status, error, result)?;
        tx.commit()?;
        Ok(written)
    }

    /// 运行中任务的进度。只改仍为 `running` 的行:迟到的进度落库不会把
    /// 已结束的任务改回旧百分比。未改动时返回 `None`。
    pub fn update_progress(
        &self,
        task_id: i64,
        progress: i64,
        remain_time_secs: Option<i64>,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        let conn = self.conn();
        let progress = progress.clamp(0, 100);
        let updated = conn
            .execute(
                "UPDATE task SET progress = ?2, remain_secs = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = ?5",
                params![
                    task_id,
                    progress,
                    remain_time_secs,
                    now(),
                    SchedTaskStatus::Running.as_str(),
                ],
            )
            .context("update task progress")?;
        if updated == 0 {
            return Ok(None);
        }
        query_task(&conn, task_id)
    }

    pub fn set_result(
        &self,
        task_id: i64,
        result: &TaskResult,
    ) -> anyhow::Result<Option<SchedTaskRecord>> {
        let conn = self.conn();
        conn.execute(
            "UPDATE task SET result = ?2, updated_at = ?3 WHERE id = ?1",
            params![task_id, serde_json::to_string(result)?, now()],
        )
        .context("set task result")?;
        query_task(&conn, task_id)
    }

    /// 删除终态任务;非终态拒绝。
    pub fn remove(&self, task_id: i64) -> anyhow::Result<Option<SchedTaskRecord>> {
        let conn = self.conn();
        let tx = case_ledger::write_transaction(&conn)?;
        let Some(current) = query_task(&tx, task_id)? else {
            return Ok(None);
        };
        if !current.status.is_terminal() {
            anyhow::bail!(
                "task {task_id} is {} and cannot be removed",
                current.status.as_str()
            );
        }
        tx.execute("DELETE FROM task WHERE id = ?1", params![task_id])
            .context("delete task")?;
        tx.commit()?;
        Ok(Some(current))
    }

    /// 启动恢复:上一进程遗留的 `queued`/`running` 任务全部置 `offline`,
    /// 交由离线处理器分流。
    pub fn reset_active_to_offline(&self, reason: &str) -> anyhow::Result<Vec<SchedTaskRecord>> {
        let stale = {
            let mut stale = self.list_by_status(SchedTaskStatus::Queued)?;
            stale.extend(self.list_by_status(SchedTaskStatus::Running)?);
            stale
        };
        let mut updated = Vec::with_capacity(stale.len());
        for task in stale {
            if let Some(record) = self.transition(
                task.id,
                &[SchedTaskStatus::Queued, SchedTaskStatus::Running],
                SchedTaskStatus::Offline,
                Some(reason),
            )? {
                updated.push(record);
            }
        }
        Ok(updated)
    }
}

/// Writes a status change of `current` (see [`TaskStore::update_status`]);
/// `result` is stored in the same statement when given.
fn write_status(
    conn: &Connection,
    current: &SchedTaskRecord,
    status: SchedTaskStatus,
    error: Option<&str>,
    result: Option<&TaskResult>,
) -> anyhow::Result<Option<SchedTaskRecord>> {
    let now = now();
    let error = match error {
        Some(error) => Some(error.to_owned()),
        None if matches!(status, SchedTaskStatus::Created | SchedTaskStatus::Queued) => None,
        None => current.info.error.clone(),
    };
    let started_at = match (status, &current.start_at) {
        (SchedTaskStatus::Running, None) => Some(now.clone()),
        (SchedTaskStatus::Created, _) => None,
        (_, existing) => existing.clone(),
    };
    let finished_at = status.is_terminal().then(|| now.clone());
    let remain_secs = if status.is_terminal() {
        Some(0)
    } else {
        current.remain_time_secs
    };
    let progress = match status {
        SchedTaskStatus::Success => 100,
        SchedTaskStatus::Created => 0,
        _ => current.progress,
    };
    let result = result.map(serde_json::to_string).transpose()?;
    conn.execute(
        "UPDATE task SET status = ?2, error = ?3, started_at = ?4, finished_at = ?5,
                         remain_secs = ?6, progress = ?7, updated_at = ?8,
                         result = COALESCE(?9, result)
         WHERE id = ?1",
        params![
            current.id,
            status.as_str(),
            error,
            started_at,
            finished_at,
            remain_secs,
            progress,
            now,
            result,
        ],
    )
    .context("update task status")?;
    query_task(conn, current.id)
}

fn query_task(conn: &Connection, task_id: i64) -> anyhow::Result<Option<SchedTaskRecord>> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM {TASK_SOURCE} WHERE t.id = ?1"),
        params![task_id],
        task_from_row,
    )
    .optional()
    .context("query task")
}

fn text_column_error(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, message.into())
}

fn json_column<T: serde::de::DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    let Some(text) = row.get::<_, Option<String>>(index)? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<SchedTaskRecord> {
    let trigger_text: String = row.get(8)?;
    let trigger = parse_trigger(&trigger_text)
        .ok_or_else(|| text_column_error(8, format!("unknown task trigger {trigger_text:?}")))?;
    let status_text: String = row.get(15)?;
    let status = SchedTaskStatus::parse(&status_text)
        .ok_or_else(|| text_column_error(15, format!("unknown task status {status_text:?}")))?;
    let params: TaskParams = json_column(row, 12)?.unwrap_or_default();
    let result: Option<TaskResult> = json_column(row, 13)?;
    let started_at: Option<String> = row.get(21)?;
    let finished_at: Option<String> = row.get(22)?;
    let spend_time_secs = started_at
        .as_deref()
        .zip(finished_at.as_deref())
        .and_then(|(started, finished)| seconds_between(started, finished));
    Ok(SchedTaskRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        type_name: row.get(3)?,
        evidence_id: row.get(4)?,
        info: TaskInfo {
            image_id: row.get(5)?,
            plugin_id: row.get(6)?,
            analyzer: row.get(7)?,
            trigger,
            parent_tid: row.get(9)?,
            error: row.get(10)?,
        },
        progress: row.get(11)?,
        params,
        result,
        dependence: row.get(14)?,
        status,
        can_pause: row.get(16)?,
        single_instance: row.get(17)?,
        internal: row.get(18)?,
        spend_time_secs,
        remain_time_secs: row.get(19)?,
        start_at: started_at,
        create_at: row.get(20)?,
        update_at: row.get(23)?,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::core::{casework::case_ledger, casework::case_ledger_evidence};

    pub(in crate::core::extensions::task_scheduler) fn temp_store() -> (tempfile::TempDir, TaskStore)
    {
        let dir = tempfile::tempdir().expect("tempdir");
        drop(case_ledger::test_support::ledger(dir.path()));
        let store = TaskStore::open(&dir.path().join("case.db")).expect("open store");
        (dir, store)
    }

    pub(in crate::core::extensions::task_scheduler) fn sample_task(
        name: &str,
        evidence_id: i64,
    ) -> NewTask {
        NewTask {
            name: name.to_owned(),
            display_name: format!("Display {name}"),
            type_name: "analysis".to_owned(),
            evidence_id,
            plugin_id: "org.test.plugin".to_owned(),
            analyzer: "demo".to_owned(),
            trigger: TaskTrigger::Manually,
            parent_tid: None,
            params: TaskParams::default(),
            single_instance: true,
            internal: false,
        }
    }

    /// Registers `image_id` as evidence of the store's case; returns its id.
    pub(in crate::core::extensions::task_scheduler) fn insert_evidence(
        store: &TaskStore,
        image_id: &str,
    ) -> i64 {
        case_ledger_evidence::test_support::add(&store.conn(), image_id).id
    }

    #[test]
    fn insert_reads_back_created_task_with_its_evidence() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let outcome = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert");
        let InsertOutcome::Created(record) = outcome else {
            panic!("first insert must create");
        };
        assert_eq!(record.status, SchedTaskStatus::Created);
        assert_eq!(record.name, "org.test.plugin/demo");
        assert_eq!(record.evidence_id, evidence_id);
        assert_eq!(record.progress, 0);
        assert_eq!(record.params, TaskParams::default());
        assert_eq!(
            record.info,
            TaskInfo {
                plugin_id: "org.test.plugin".to_owned(),
                analyzer: "demo".to_owned(),
                image_id: "image-1".to_owned(),
                trigger: TaskTrigger::Manually,
                parent_tid: None,
                error: None,
            }
        );
        assert!(record.single_instance);
        assert!(record.create_at.ends_with('Z'));
        assert_eq!(store.get(record.id).expect("get"), Some(record));
    }

    #[test]
    fn tasks_need_an_evidence_of_the_case() {
        let (_dir, store) = temp_store();
        assert!(store
            .insert_task(&sample_task("org.test.plugin/demo", 999))
            .is_err());
    }

    #[test]
    fn single_instance_returns_existing_active_task() {
        let (_dir, store) = temp_store();
        let seven = insert_evidence(&store, "image-7");
        let eight = insert_evidence(&store, "image-8");
        let first = store
            .insert_task(&sample_task("org.test.plugin/demo", seven))
            .expect("insert")
            .into_record();
        let second = store
            .insert_task(&sample_task("org.test.plugin/demo", seven))
            .expect("insert");
        assert_eq!(second, InsertOutcome::Existing(first.clone()));

        let other_evidence = store
            .insert_task(&sample_task("org.test.plugin/demo", eight))
            .expect("insert");
        assert!(matches!(other_evidence, InsertOutcome::Created(_)));

        // Paused and offline tasks still hold the slot.
        store
            .update_status(first.id, SchedTaskStatus::Pause, None)
            .expect("pause");
        assert!(matches!(
            store
                .insert_task(&sample_task("org.test.plugin/demo", seven))
                .expect("insert"),
            InsertOutcome::Existing(_)
        ));
        store
            .update_status(first.id, SchedTaskStatus::Success, None)
            .expect("finish");
        let after_terminal = store
            .insert_task(&sample_task("org.test.plugin/demo", seven))
            .expect("insert");
        assert!(matches!(after_terminal, InsertOutcome::Created(_)));
    }

    #[test]
    fn single_instance_counts_per_target() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let on = |system: &str| {
            let mut task = sample_task("org.test.plugin/system", evidence_id);
            task.params = TaskParams {
                system_id: Some(system.to_owned()),
                ..TaskParams::default()
            };
            store.insert_task(&task).expect("insert")
        };
        let linux = on("linux@part:2");
        assert!(matches!(linux, InsertOutcome::Created(_)));
        assert!(matches!(on("linux@part:3"), InsertOutcome::Created(_)));
        assert_eq!(
            on("linux@part:2"),
            InsertOutcome::Existing(linux.record().clone())
        );
        assert!(store
            .has_task("org.test.plugin/system", evidence_id, "linux@part:3")
            .expect("has task"));
        assert!(!store
            .has_task("org.test.plugin/system", evidence_id, "windows@part:1")
            .expect("has task"));
        store
            .update_status(linux.record().id, SchedTaskStatus::Success, None)
            .expect("success");
        assert_eq!(
            store
                .succeeded_targets("org.test.plugin/system", evidence_id)
                .expect("targets"),
            vec!["linux@part:2".to_owned()]
        );
    }

    #[test]
    fn the_ledger_refuses_a_second_active_single_instance_task() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert");
        let conn = store.conn();
        let duplicate = conn.execute(
            "INSERT INTO task (evidence_id, function, plugin_id, analyzer, display_name, kind,
                               trigger, params, status, can_pause, single_instance, internal,
                               created_at, updated_at)
             VALUES (?1, 'org.test.plugin/demo', 'p', 'a', 'd', 'k', 'auto', '{}', 'queued',
                     0, 1, 0, 'now', 'now')",
            params![evidence_id],
        );
        assert!(duplicate.is_err());
    }

    #[test]
    fn status_transitions_stamp_start_and_spend_time() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let task = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert")
            .into_record();
        let queued = store
            .update_status(task.id, SchedTaskStatus::Queued, None)
            .expect("queue")
            .expect("row");
        assert_eq!(queued.status, SchedTaskStatus::Queued);
        assert_eq!(queued.start_at, None);

        let running = store
            .update_status(task.id, SchedTaskStatus::Running, None)
            .expect("run")
            .expect("row");
        assert!(running.start_at.is_some());
        assert_eq!(running.spend_time_secs, None);

        let progressed = store
            .update_progress(task.id, 40, Some(90))
            .expect("progress")
            .expect("row");
        assert_eq!(progressed.progress, 40);
        assert_eq!(progressed.remain_time_secs, Some(90));

        let failed = store
            .update_status(task.id, SchedTaskStatus::Failed, Some("boom"))
            .expect("fail")
            .expect("row");
        assert_eq!(failed.status, SchedTaskStatus::Failed);
        assert_eq!(failed.info.error.as_deref(), Some("boom"));
        assert_eq!(failed.spend_time_secs, Some(0));
        assert_eq!(failed.remain_time_secs, Some(0));
        assert_eq!(failed.progress, 40);

        let restarted = store
            .update_status(task.id, SchedTaskStatus::Created, None)
            .expect("requeue")
            .expect("row");
        assert_eq!(restarted.start_at, None);
        assert_eq!(restarted.spend_time_secs, None);
        assert_eq!(restarted.info.error, None);
        assert_eq!(restarted.progress, 0);
    }

    #[test]
    fn success_forces_full_progress_and_stores_result() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let task = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert")
            .into_record();
        store
            .update_status(task.id, SchedTaskStatus::Running, None)
            .expect("run");
        let result = TaskResult {
            summary: Some("ok".to_owned()),
            record_count: 2,
            timeline_count: 1,
            warning_count: 0,
            synthetic_bytes: 0,
            run_id: Some("run-1".to_owned()),
        };
        store.set_result(task.id, &result).expect("result");
        let done = store
            .update_status(task.id, SchedTaskStatus::Success, None)
            .expect("success")
            .expect("row");
        assert_eq!(done.progress, 100);
        assert_eq!(done.result, Some(result));
    }

    #[test]
    fn complete_writes_result_and_terminal_status_of_a_running_task() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let task = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert")
            .into_record();
        store
            .update_status(task.id, SchedTaskStatus::Running, None)
            .expect("run");
        let result = TaskResult {
            summary: Some("ok".to_owned()),
            record_count: 3,
            ..TaskResult::default()
        };
        let done = store
            .complete(task.id, SchedTaskStatus::Success, Some(&result), None)
            .expect("complete")
            .expect("row");
        assert_eq!(done.status, SchedTaskStatus::Success);
        assert_eq!(done.progress, 100);
        assert_eq!(done.result, Some(result));
        assert_eq!(
            store
                .complete(task.id, SchedTaskStatus::Failed, None, Some("late"))
                .expect("second commit"),
            None,
            "a finished task takes no second commit"
        );
    }

    #[test]
    fn transitions_never_overwrite_a_state_the_task_already_left() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let task = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert")
            .into_record();
        let queued = store
            .transition(
                task.id,
                &[SchedTaskStatus::Created],
                SchedTaskStatus::Queued,
                None,
            )
            .expect("queue")
            .expect("row");
        assert_eq!(queued.status, SchedTaskStatus::Queued);

        // Cancelled between dispatch and start: the execution must not
        // revive it, commit into it or move its progress.
        store
            .update_status(task.id, SchedTaskStatus::Cancel, None)
            .expect("cancel");
        assert_eq!(
            store
                .transition(
                    task.id,
                    &[SchedTaskStatus::Queued],
                    SchedTaskStatus::Running,
                    None
                )
                .expect("start"),
            None
        );
        assert_eq!(
            store
                .complete(
                    task.id,
                    SchedTaskStatus::Success,
                    Some(&TaskResult::default()),
                    None
                )
                .expect("commit"),
            None
        );
        assert_eq!(
            store
                .update_progress(task.id, 50, Some(10))
                .expect("progress"),
            None
        );
        let cancelled = store.get(task.id).expect("get").expect("row");
        assert_eq!(cancelled.status, SchedTaskStatus::Cancel);
        assert_eq!(cancelled.progress, 0);
        assert_eq!(cancelled.result, None);
    }

    #[test]
    fn list_filters_internal_and_evidence() {
        let (_dir, store) = temp_store();
        let seven = insert_evidence(&store, "image-7");
        let eight = insert_evidence(&store, "image-8");
        let mut internal = sample_task("org.test.plugin/internal", seven);
        internal.internal = true;
        store.insert_task(&internal).expect("insert internal");
        store
            .insert_task(&sample_task("org.test.plugin/visible", seven))
            .expect("insert visible");
        store
            .insert_task(&sample_task("org.test.plugin/other", eight))
            .expect("insert other");

        let visible = store
            .list(&TaskFilter {
                evidence_id: Some(seven),
                include_internal: false,
            })
            .expect("list");
        assert_eq!(
            visible
                .iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.plugin/visible"]
        );
        let all = store
            .list(&TaskFilter {
                evidence_id: None,
                include_internal: true,
            })
            .expect("list all");
        assert_eq!(all.len(), 3);
        assert_eq!(
            store.evidence_ids_with_tasks().expect("ids"),
            vec![seven, eight]
        );
    }

    #[test]
    fn startup_recovery_marks_active_tasks_offline() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let queued = store
            .insert_task(&sample_task("org.test.plugin/a", evidence_id))
            .expect("insert")
            .into_record();
        store
            .update_status(queued.id, SchedTaskStatus::Queued, None)
            .expect("queue");
        let running = store
            .insert_task(&sample_task("org.test.plugin/b", evidence_id))
            .expect("insert")
            .into_record();
        store
            .update_status(running.id, SchedTaskStatus::Running, None)
            .expect("run");
        let created = store
            .insert_task(&sample_task("org.test.plugin/c", evidence_id))
            .expect("insert")
            .into_record();

        let offline = store
            .reset_active_to_offline("worker lost at restart")
            .expect("reset");
        assert_eq!(
            offline.iter().map(|task| task.id).collect::<Vec<_>>(),
            vec![queued.id, running.id]
        );
        assert!(offline
            .iter()
            .all(|task| task.status == SchedTaskStatus::Offline
                && task.info.error.as_deref() == Some("worker lost at restart")));
        assert_eq!(
            store.get(created.id).expect("get").map(|task| task.status),
            Some(SchedTaskStatus::Created)
        );
    }

    #[test]
    fn remove_only_accepts_terminal_tasks() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let task = store
            .insert_task(&sample_task("org.test.plugin/demo", evidence_id))
            .expect("insert")
            .into_record();
        assert!(store.remove(task.id).is_err());
        store
            .update_status(task.id, SchedTaskStatus::Cancel, None)
            .expect("cancel");
        assert_eq!(
            store.remove(task.id).expect("remove").map(|task| task.id),
            Some(task.id)
        );
        assert_eq!(store.get(task.id).expect("get"), None);
        assert_eq!(store.remove(task.id).expect("remove missing"), None);
    }

    #[test]
    fn evidence_task_status_and_followups_round_trip() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let first = store
            .insert_task(&sample_task("org.test.plugin/a", evidence_id))
            .expect("insert")
            .into_record();
        store
            .insert_task(&sample_task("org.test.plugin/b", evidence_id))
            .expect("insert");
        store
            .update_status(first.id, SchedTaskStatus::Success, None)
            .expect("success");

        let aggregate = store.evidence_task_status(evidence_id).expect("aggregate");
        assert_eq!(aggregate.total, 2);
        assert_eq!(aggregate.success, 1);
        assert_eq!(aggregate.created, 1);

        assert_eq!(
            store.evidence_post_actions(evidence_id).expect("actions"),
            Some((None, false))
        );
        store
            .set_evidence_post_actions(evidence_id, Some("[{\"action\":\"notify\"}]"))
            .expect("set actions");
        store.mark_post_action_executed(evidence_id).expect("mark");
        assert_eq!(
            store.evidence_post_actions(evidence_id).expect("actions"),
            Some((Some("[{\"action\":\"notify\"}]".to_owned()), true))
        );
        // Setting the actions again re-arms them; clearing removes them.
        store
            .set_evidence_post_actions(evidence_id, Some("[]"))
            .expect("re-arm");
        assert_eq!(
            store.evidence_post_actions(evidence_id).expect("actions"),
            Some((Some("[]".to_owned()), false))
        );
        store
            .set_evidence_post_actions(evidence_id, None)
            .expect("clear");
        assert_eq!(
            store.evidence_post_actions(evidence_id).expect("actions"),
            Some((None, false))
        );
        assert_eq!(store.evidence_post_actions(999).expect("missing"), None);
        assert_eq!(
            store.evidence_id_for_image("image-1").expect("lookup"),
            Some(evidence_id)
        );
    }
}
