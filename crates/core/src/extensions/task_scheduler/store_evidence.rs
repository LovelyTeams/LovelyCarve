//! `TaskStore` 的检材侧操作(从 `store.rs` 拆出):image id ↔ 台账检材主键、
//! 任务状态即时统计,以及收尾动作(台账 `task_followup` 表,PostActionWatcher
//! 的载荷)。

use anyhow::Context;
use rusqlite::{params, OptionalExtension};

use super::{
    model::{EvidenceTaskStatus, SchedTaskStatus},
    store::TaskStore,
};
use crate::casework::case_ledger::{self, now};

impl TaskStore {
    /// 台账检材主键(任务的 `evidence_id`)← LovelyCarve image id。
    pub fn evidence_id_for_image(&self, image_id: &str) -> anyhow::Result<Option<i64>> {
        case_ledger::evidence_id(&self.conn(), image_id).context("query evidence id")
    }

    /// 检材全部任务的状态统计。
    pub fn evidence_task_status(&self, evidence_id: i64) -> anyhow::Result<EvidenceTaskStatus> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT status FROM task WHERE evidence_id = ?1")?;
        let statuses = statement
            .query_map(params![evidence_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EvidenceTaskStatus::from_statuses(
            statuses
                .iter()
                .filter_map(|status| SchedTaskStatus::parse(status)),
        ))
    }

    pub fn evidence_ids_with_tasks(&self) -> anyhow::Result<Vec<i64>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT DISTINCT evidence_id FROM task ORDER BY evidence_id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// `(收尾动作 JSON, 是否已执行)`;检材不在案件中时返回 `None`。
    pub fn evidence_post_actions(
        &self,
        evidence_id: i64,
    ) -> anyhow::Result<Option<(Option<String>, bool)>> {
        self.conn()
            .query_row(
                "SELECT f.actions, f.done_at IS NOT NULL
                 FROM evidence e LEFT JOIN task_followup f ON f.evidence_id = e.id
                 WHERE e.id = ?1",
                params![evidence_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<bool>>(1)?.unwrap_or(false),
                    ))
                },
            )
            .optional()
            .context("query evidence post actions")
    }

    /// 设置检材的收尾动作并重置执行标记;`None` 清除。
    pub fn set_evidence_post_actions(
        &self,
        evidence_id: i64,
        actions_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        match actions_json {
            Some(actions) => conn
                .execute(
                    "INSERT INTO task_followup (evidence_id, actions, done_at) VALUES (?1, ?2, NULL)
                     ON CONFLICT (evidence_id) DO UPDATE SET
                        actions = excluded.actions, done_at = NULL",
                    params![evidence_id, actions],
                )
                .context("set evidence post actions")?,
            None => conn
                .execute(
                    "DELETE FROM task_followup WHERE evidence_id = ?1",
                    params![evidence_id],
                )
                .context("clear evidence post actions")?,
        };
        Ok(())
    }

    pub fn mark_post_action_executed(&self, evidence_id: i64) -> anyhow::Result<()> {
        self.conn()
            .execute(
                "UPDATE task_followup SET done_at = ?2 WHERE evidence_id = ?1",
                params![evidence_id, now()],
            )
            .context("mark post action executed")?;
        Ok(())
    }
}
