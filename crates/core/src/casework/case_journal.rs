//! 案件审计链:台账 `journal` 表上的哈希链。
//!
//! 每条记录的 `hash` 覆盖上一条的 `hash` 与本条全部字段(逐字段带长度前缀,
//! 字段边界无法挪动);首条的「上一条」是案件 uid,链与案件身份绑定。改写、
//! 删除或插入任何一条历史记录,其后的校验都会失败,[`verify`] 报出第一处
//! 断点。链只能证明内部一致:截掉末尾若干条不会留下断点,需要时应把最新
//! `hash` 抄录到案件之外。
//!
//! 记录与它描述的台账变更在同一个事务里写入,两者要么都在、要么都不在。

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

/// 一条要写入的审计事件。`evidence` 记检材 uid(不做外键,检材移除后记录
/// 仍然保留)。
#[derive(Debug, Clone, Copy)]
pub struct Event<'a> {
    pub action: &'a str,
    pub evidence: Option<&'a str>,
    pub detail: &'a str,
}

/// 一条已落库的审计记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub seq: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub evidence: Option<String>,
    pub detail: String,
    pub hash: String,
}

/// 链校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verification {
    pub entries: u64,
    /// 第一条哈希对不上(或序号不连续)的记录;`None` 表示整条链一致。
    pub first_broken_seq: Option<i64>,
}

/// 当前操作系统用户,作为审计记录的操作人。不读写进程环境以外的任何东西。
pub fn current_actor() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "examiner".to_string())
}

fn chain_hash(previous: &str, entry: &JournalEntry) -> String {
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    field(previous.as_bytes());
    field(entry.seq.to_string().as_bytes());
    field(entry.at.as_bytes());
    field(entry.actor.as_bytes());
    field(entry.action.as_bytes());
    match entry.evidence.as_deref() {
        Some(evidence) => {
            field(b"1");
            field(evidence.as_bytes());
        }
        None => field(b"0"),
    }
    field(entry.detail.as_bytes());
    hex::encode(hasher.finalize())
}

/// 在调用方的写事务里追加一条记录。
pub fn append(tx: &Transaction<'_>, case_uid: &str, event: Event<'_>) -> anyhow::Result<()> {
    let last: Option<(i64, String)> = tx
        .query_row(
            "SELECT seq, hash FROM journal ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (previous_seq, previous_hash) = last.unwrap_or_else(|| (0, case_uid.to_string()));
    let mut entry = JournalEntry {
        seq: previous_seq + 1,
        at: crate::casework::case_ledger::now(),
        actor: current_actor(),
        action: event.action.to_string(),
        evidence: event.evidence.map(str::to_string),
        detail: event.detail.to_string(),
        hash: String::new(),
    };
    entry.hash = chain_hash(&previous_hash, &entry);
    tx.execute(
        "INSERT INTO journal (seq, at, actor, action, evidence, detail, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            entry.seq,
            entry.at,
            entry.actor,
            entry.action,
            entry.evidence,
            entry.detail,
            entry.hash
        ],
    )
    .context("append case journal")?;
    Ok(())
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalEntry> {
    Ok(JournalEntry {
        seq: row.get(0)?,
        at: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        evidence: row.get(4)?,
        detail: row.get(5)?,
        hash: row.get(6)?,
    })
}

/// 最近的 `limit` 条记录,新的在前。
pub fn recent(conn: &Connection, limit: usize) -> anyhow::Result<Vec<JournalEntry>> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT seq, at, actor, action, evidence, detail, hash
         FROM journal ORDER BY seq DESC LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit], entry_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 从头校验整条链。
pub fn verify(conn: &Connection, case_uid: &str) -> anyhow::Result<Verification> {
    let mut statement = conn.prepare(
        "SELECT seq, at, actor, action, evidence, detail, hash FROM journal ORDER BY seq",
    )?;
    let mut rows = statement.query([])?;
    let mut previous = case_uid.to_string();
    let mut expected_seq = 1;
    let mut entries = 0u64;
    while let Some(row) = rows.next()? {
        let entry = entry_from_row(row)?;
        entries += 1;
        if entry.seq != expected_seq || chain_hash(&previous, &entry) != entry.hash {
            return Ok(Verification {
                entries,
                first_broken_seq: Some(entry.seq),
            });
        }
        previous = entry.hash;
        expected_seq += 1;
    }
    Ok(Verification {
        entries,
        first_broken_seq: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casework::case_ledger::{self, test_support};

    fn record(conn: &Connection, case_uid: &str, action: &str, evidence: Option<&str>) {
        let tx = case_ledger::write_transaction(conn).expect("tx");
        append(
            &tx,
            case_uid,
            Event {
                action,
                evidence,
                detail: "detail",
            },
        )
        .expect("append");
        tx.commit().expect("commit");
    }

    #[test]
    fn chain_verifies_and_reports_the_first_tampered_entry() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        let case_uid = case_ledger::load_profile(&conn).expect("profile").case_uid;
        record(&conn, &case_uid, "case.create", None);
        record(&conn, &case_uid, "evidence.add", Some("img-1"));
        record(&conn, &case_uid, "evidence.remove", Some("img-1"));
        assert_eq!(
            verify(&conn, &case_uid).expect("verify"),
            Verification {
                entries: 3,
                first_broken_seq: None
            }
        );
        let recent = recent(&conn, 2).expect("recent");
        assert_eq!(
            recent
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec!["evidence.remove", "evidence.add"]
        );

        conn.execute("UPDATE journal SET detail = 'forged' WHERE seq = 2", [])
            .expect("tamper");
        assert_eq!(
            verify(&conn, &case_uid).expect("verify").first_broken_seq,
            Some(2)
        );
    }

    #[test]
    fn a_removed_middle_entry_breaks_the_chain() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        let case_uid = case_ledger::load_profile(&conn).expect("profile").case_uid;
        for action in ["a", "b", "c"] {
            record(&conn, &case_uid, action, None);
        }
        conn.execute("DELETE FROM journal WHERE seq = 2", [])
            .expect("delete");
        assert_eq!(
            verify(&conn, &case_uid).expect("verify").first_broken_seq,
            Some(3)
        );
        // The genesis is bound to the case identity.
        assert_eq!(
            verify(&conn, "another-case")
                .expect("verify")
                .first_broken_seq,
            Some(1)
        );
    }
}
