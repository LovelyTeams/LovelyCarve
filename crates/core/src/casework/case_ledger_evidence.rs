//! 台账中的检材登记,以及挂在检材上的小型事实:原始盘符、源读故障。
//!
//! 检材以 `uid`(LovelyCarve image id)为外部身份,整数 `id` 只在台账内做外键;
//! `path_key`(`core::casework::case::evidence_path_key`)唯一,同一来源在案件里只有一条
//! 记录。删除检材行由外键级联带走它的卷发布、标签、盘符、读故障与任务。
//!
//! 检验人手动指定的平台存在 `setting` 表的 `evidence.platform.<uid>` 键下(台账结构
//! 不变),读检材时一并带出,删除检材时一并删除;`evidence.platform` 列只放自动识别结论。

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::analysis::identify::win_drive_letters::{DriveLetterAssignment, DriveLetterReport};
use crate::casework::case_ledger::{evidence_id, now};
use crate::evidence::image::BadRegion;

/// 一条检材登记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRecord {
    pub id: i64,
    pub uid: String,
    pub path: String,
    pub format: String,
    pub size_bytes: u64,
    /// 自动识别出的平台(未识别为 `None`)。
    pub platform: Option<String>,
    /// 自动识别出的检材类型(`disk` / `compressfile` / 备份枚举词)。
    pub kind: Option<String>,
    pub added_at: String,
    pub updated_at: String,
    /// 检验人手动指定的平台,优先于自动识别,重新识别不覆盖。
    pub manual_platform: Option<String>,
}

/// 手动平台在 `setting` 表中的键前缀,后接检材 uid。
pub const MANUAL_PLATFORM_KEY_PREFIX: &str = "evidence.platform.";

fn manual_platform_key(uid: &str) -> String {
    format!("{MANUAL_PLATFORM_KEY_PREFIX}{uid}")
}

/// 登记输入。
#[derive(Debug, Clone, Copy)]
pub struct NewEvidence<'a> {
    pub uid: &'a str,
    pub path: &'a str,
    pub path_key: &'a str,
    pub format: &'a str,
    pub size_bytes: u64,
    pub added_at: &'a str,
}

/// [`register`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// 新登记。
    Added(EvidenceRecord),
    /// 已登记,来源属性有变化并已刷新。
    Refreshed(EvidenceRecord),
    /// 已登记且无变化。
    Unchanged(EvidenceRecord),
    /// 同一来源已作为另一个检材登记,本次登记被拒绝。
    SourceTaken { existing_uid: String },
}

/// 检材列与它的手动平台;`manual` 是按 `?1 || uid` 连接的 `setting` 行。
const COLUMNS: &str = "e.id, e.uid, e.path, e.format, e.size_bytes, e.platform, e.kind, \
                       e.added_at, e.updated_at, manual.value";

fn record_from_row(row: &Row<'_>) -> rusqlite::Result<EvidenceRecord> {
    let size: i64 = row.get(4)?;
    Ok(EvidenceRecord {
        id: row.get(0)?,
        uid: row.get(1)?,
        path: row.get(2)?,
        format: row.get(3)?,
        size_bytes: u64::try_from(size).unwrap_or(0),
        platform: row.get(5)?,
        kind: row.get(6)?,
        added_at: row.get(7)?,
        updated_at: row.get(8)?,
        manual_platform: row.get(9)?,
    })
}

/// 全部检材,按登记顺序。
pub fn list(conn: &Connection) -> anyhow::Result<Vec<EvidenceRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM evidence e
         LEFT JOIN setting manual ON manual.key = ?1 || e.uid
         ORDER BY e.id"
    ))?;
    let rows = statement.query_map(params![MANUAL_PLATFORM_KEY_PREFIX], record_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get(conn: &Connection, uid: &str) -> anyhow::Result<Option<EvidenceRecord>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM evidence e
             LEFT JOIN setting manual ON manual.key = ?1 || e.uid
             WHERE e.uid = ?2"
        ),
        params![MANUAL_PLATFORM_KEY_PREFIX, uid],
        record_from_row,
    )
    .optional()
    .map_err(Into::into)
}

/// 登记或刷新一个检材(以 `uid` 幂等)。来源已被另一个检材占用时不写入。
pub fn register(tx: &Transaction<'_>, evidence: NewEvidence<'_>) -> anyhow::Result<Registration> {
    let size = i64::try_from(evidence.size_bytes).context("evidence size exceeds i64")?;
    if let Some(existing_uid) = tx
        .query_row(
            "SELECT uid FROM evidence WHERE path_key = ?1 AND uid <> ?2",
            params![evidence.path_key, evidence.uid],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Registration::SourceTaken { existing_uid });
    }
    match get(tx, evidence.uid)? {
        Some(existing)
            if existing.path == evidence.path
                && existing.format == evidence.format
                && existing.size_bytes == evidence.size_bytes =>
        {
            Ok(Registration::Unchanged(existing))
        }
        Some(_) => {
            tx.execute(
                "UPDATE evidence SET path = ?2, path_key = ?3, format = ?4, size_bytes = ?5,
                                     updated_at = ?6
                 WHERE uid = ?1",
                params![
                    evidence.uid,
                    evidence.path,
                    evidence.path_key,
                    evidence.format,
                    size,
                    now()
                ],
            )?;
            let refreshed = get(tx, evidence.uid)?.context("refreshed evidence row vanished")?;
            Ok(Registration::Refreshed(refreshed))
        }
        None => {
            tx.execute(
                "INSERT INTO evidence (uid, path, path_key, format, size_bytes, added_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    evidence.uid,
                    evidence.path,
                    evidence.path_key,
                    evidence.format,
                    size,
                    evidence.added_at,
                    now()
                ],
            )?;
            let added = get(tx, evidence.uid)?.context("inserted evidence row vanished")?;
            Ok(Registration::Added(added))
        }
    }
}

/// 删除检材行(级联带走卷发布、标签、盘符、读故障与任务)与它的手动平台。
/// 返回被删的行。
pub fn delete(tx: &Transaction<'_>, uid: &str) -> anyhow::Result<Option<EvidenceRecord>> {
    let Some(existing) = get(tx, uid)? else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM setting WHERE key = ?1",
        params![manual_platform_key(uid)],
    )?;
    tx.execute("DELETE FROM evidence WHERE id = ?1", params![existing.id])?;
    Ok(Some(existing))
}

/// 回写自动识别结论。`platform` 为 `None` 时保留原值(未命中不清空)。
/// 检材不在案件中时返回 `None`。
pub fn record_detection(
    tx: &Transaction<'_>,
    uid: &str,
    platform: Option<&str>,
    kind: &str,
    detection_json: &str,
) -> anyhow::Result<Option<EvidenceRecord>> {
    let changed = tx.execute(
        "UPDATE evidence SET platform = COALESCE(?2, platform), kind = ?3, detection = ?4,
                             updated_at = ?5
         WHERE uid = ?1",
        params![uid, platform, kind, detection_json, now()],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get(tx, uid)
}

/// 写入检验人手动指定的平台;`None` 清除手动选择,恢复自动识别结论。检材不在
/// 案件中时返回 `None`。
pub fn set_manual_platform(
    tx: &Transaction<'_>,
    uid: &str,
    platform: Option<&str>,
) -> anyhow::Result<Option<EvidenceRecord>> {
    if evidence_id(tx, uid)?.is_none() {
        return Ok(None);
    }
    let key = manual_platform_key(uid);
    match platform {
        Some(platform) => tx.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, platform],
        )?,
        None => tx.execute("DELETE FROM setting WHERE key = ?1", params![key])?,
    };
    tx.execute(
        "UPDATE evidence SET updated_at = ?2 WHERE uid = ?1",
        params![uid, now()],
    )?;
    get(tx, uid)
}

/// 自动识别的完整报告 JSON。
pub fn detection(conn: &Connection, uid: &str) -> anyhow::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT detection FROM evidence WHERE uid = ?1",
            params![uid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

// ── 原始盘符 ────────────────────────────────────────────────────────────

/// 已缓存的盘符报告;没有缓存(或检材不在案件中)时为 `None`。
pub fn drive_letters(conn: &Connection, uid: &str) -> anyhow::Result<Option<DriveLetterReport>> {
    let mut statement = conn.prepare(
        "SELECT d.partition_id, d.letter, d.ambiguous
         FROM drive_letter d JOIN evidence e ON e.id = d.evidence_id
         WHERE e.uid = ?1
         ORDER BY d.partition_id, d.letter",
    )?;
    let rows = statement.query_map(params![uid], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })?;
    let mut assignments = Vec::new();
    let mut ambiguous = false;
    for row in rows {
        let (partition_id, drive_letter, row_ambiguous) = row?;
        let partition_id =
            u8::try_from(partition_id).context("stored drive-letter partition id")?;
        assignments.push(DriveLetterAssignment {
            partition_id,
            drive_letter,
        });
        ambiguous |= row_ambiguous;
    }
    Ok((!assignments.is_empty()).then_some(DriveLetterReport {
        assignments,
        ambiguous,
    }))
}

/// 整体替换检材的盘符报告;空报告即清除缓存。检材不在案件中时返回 `false`。
pub fn replace_drive_letters(
    tx: &Transaction<'_>,
    uid: &str,
    report: &DriveLetterReport,
) -> anyhow::Result<bool> {
    let Some(evidence) = evidence_id(tx, uid)? else {
        return Ok(false);
    };
    tx.execute(
        "DELETE FROM drive_letter WHERE evidence_id = ?1",
        params![evidence],
    )?;
    let mut statement = tx.prepare(
        "INSERT OR REPLACE INTO drive_letter (evidence_id, partition_id, letter, ambiguous)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for assignment in &report.assignments {
        statement.execute(params![
            evidence,
            assignment.partition_id,
            assignment.drive_letter,
            report.ambiguous
        ])?;
    }
    Ok(true)
}

// ── 源读故障 ────────────────────────────────────────────────────────────

/// 检材持久化的读故障区间(按偏移排序)。
pub fn read_faults(conn: &Connection, uid: &str) -> anyhow::Result<Vec<BadRegion>> {
    let mut statement = conn.prepare(
        "SELECT r.offset, r.length
         FROM read_fault r JOIN evidence e ON e.id = r.evidence_id
         WHERE e.uid = ?1
         ORDER BY r.offset",
    )?;
    let rows = statement.query_map(params![uid], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut regions = Vec::new();
    for row in rows {
        let (offset, length) = row?;
        regions.push(BadRegion {
            offset: u64::try_from(offset).context("stored read-fault offset")?,
            len: u64::try_from(length).context("stored read-fault length")?,
        });
    }
    Ok(regions)
}

/// 以已合并的区间整体替换检材的读故障记录。检材不在案件中时返回 `false`。
pub fn replace_read_faults(
    tx: &Transaction<'_>,
    uid: &str,
    regions: &[BadRegion],
) -> anyhow::Result<bool> {
    let Some(evidence) = evidence_id(tx, uid)? else {
        return Ok(false);
    };
    tx.execute(
        "DELETE FROM read_fault WHERE evidence_id = ?1",
        params![evidence],
    )?;
    let mut statement = tx.prepare(
        "INSERT OR REPLACE INTO read_fault (evidence_id, offset, length) VALUES (?1, ?2, ?3)",
    )?;
    for region in regions.iter().filter(|region| region.len > 0) {
        statement.execute(params![
            evidence,
            i64::try_from(region.offset).context("read-fault offset exceeds i64")?,
            i64::try_from(region.len).context("read-fault length exceeds i64")?,
        ])?;
    }
    Ok(true)
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    /// Registers `uid` as evidence with a path derived from it.
    pub fn add(conn: &Connection, uid: &str) -> EvidenceRecord {
        let path = format!("/evidence/{uid}.E01");
        let tx = crate::casework::case_ledger::write_transaction(conn).expect("tx");
        let outcome = register(
            &tx,
            NewEvidence {
                uid,
                path: &path,
                path_key: &path,
                format: "ewf",
                size_bytes: 1,
                added_at: "2026-10-09T00:00:00.000Z",
            },
        )
        .expect("register");
        tx.commit().expect("commit");
        match outcome {
            Registration::Added(record)
            | Registration::Refreshed(record)
            | Registration::Unchanged(record) => record,
            Registration::SourceTaken { existing_uid } => {
                panic!("source already registered as {existing_uid}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::add;
    use super::*;
    use crate::casework::case_ledger::{test_support, write_transaction};

    fn new_evidence<'a>(uid: &'a str, path: &'a str, size_bytes: u64) -> NewEvidence<'a> {
        NewEvidence {
            uid,
            path,
            path_key: path,
            format: "raw",
            size_bytes,
            added_at: "2026-10-09T00:00:00.000Z",
        }
    }

    #[test]
    fn registration_is_idempotent_and_one_record_per_source() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        let tx = write_transaction(&conn).expect("tx");
        let added = register(&tx, new_evidence("a", "/a.dd", 10)).expect("add");
        assert!(matches!(added, Registration::Added(_)));
        assert!(matches!(
            register(&tx, new_evidence("a", "/a.dd", 10)).expect("same"),
            Registration::Unchanged(_)
        ));
        let Registration::Refreshed(refreshed) =
            register(&tx, new_evidence("a", "/a.dd", 20)).expect("refresh")
        else {
            panic!("size change refreshes");
        };
        assert_eq!(refreshed.size_bytes, 20);
        assert_eq!(
            register(&tx, new_evidence("b", "/a.dd", 20)).expect("conflict"),
            Registration::SourceTaken {
                existing_uid: "a".to_string()
            }
        );
        tx.commit().expect("commit");
        assert_eq!(list(&conn).expect("list").len(), 1);
    }

    #[test]
    fn detection_keeps_platform_when_not_detected() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        add(&conn, "img");
        let tx = write_transaction(&conn).expect("tx");
        let updated = record_detection(&tx, "img", Some("windows"), "disk", "{\"a\":1}")
            .expect("detect")
            .expect("row");
        assert_eq!(updated.platform.as_deref(), Some("windows"));
        let kept = record_detection(&tx, "img", None, "compressfile", "{}")
            .expect("detect")
            .expect("row");
        assert_eq!(
            (kept.platform.as_deref(), kept.kind.as_deref()),
            (Some("windows"), Some("compressfile"))
        );
        assert!(record_detection(&tx, "missing", None, "disk", "{}")
            .expect("detect")
            .is_none());
        tx.commit().expect("commit");
        assert_eq!(
            detection(&conn, "img").expect("json").as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn manual_platform_survives_detection_and_leaves_with_the_evidence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        add(&conn, "img");
        let tx = write_transaction(&conn).expect("tx");
        record_detection(&tx, "img", Some("linux"), "compressfile", "{}").expect("detect");
        let manual = set_manual_platform(&tx, "img", Some("windows"))
            .expect("set")
            .expect("row");
        assert_eq!(
            (
                manual.platform.as_deref(),
                manual.manual_platform.as_deref()
            ),
            (Some("linux"), Some("windows"))
        );
        // 重新识别只更新自动结论。
        let redetected = record_detection(&tx, "img", Some("android"), "backup", "{}")
            .expect("detect")
            .expect("row");
        assert_eq!(
            (
                redetected.platform.as_deref(),
                redetected.manual_platform.as_deref()
            ),
            (Some("android"), Some("windows"))
        );
        let cleared = set_manual_platform(&tx, "img", None)
            .expect("clear")
            .expect("row");
        assert_eq!(
            (cleared.platform.as_deref(), cleared.manual_platform),
            (Some("android"), None)
        );
        assert!(set_manual_platform(&tx, "missing", Some("ios"))
            .expect("unknown evidence")
            .is_none());
        set_manual_platform(&tx, "img", Some("drone")).expect("set again");
        let listed: Vec<Option<String>> = list(&tx)
            .expect("list")
            .into_iter()
            .map(|record| record.manual_platform)
            .collect();
        assert_eq!(listed, vec![Some("drone".to_owned())]);
        assert!(delete(&tx, "img").expect("delete").is_some());
        tx.commit().expect("commit");
        assert_eq!(
            crate::casework::case_ledger::settings_query(&conn, MANUAL_PLATFORM_KEY_PREFIX)
                .expect("settings"),
            Vec::new()
        );
    }

    #[test]
    fn drive_letters_and_read_faults_cascade_with_the_evidence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        add(&conn, "img");
        let tx = write_transaction(&conn).expect("tx");
        assert!(replace_drive_letters(
            &tx,
            "img",
            &DriveLetterReport {
                assignments: vec![DriveLetterAssignment {
                    partition_id: 1,
                    drive_letter: "C:".into()
                }],
                ambiguous: true,
            }
        )
        .expect("letters"));
        assert!(replace_read_faults(
            &tx,
            "img",
            &[BadRegion {
                offset: 512,
                len: 1024
            }]
        )
        .expect("faults"));
        assert!(!replace_read_faults(&tx, "missing", &[]).expect("unknown evidence"));
        tx.commit().expect("commit");

        let report = drive_letters(&conn, "img").expect("load").expect("cached");
        assert_eq!(report.assignments[0].drive_letter, "C:");
        assert!(report.ambiguous);
        assert_eq!(
            read_faults(&conn, "img").expect("faults"),
            vec![BadRegion {
                offset: 512,
                len: 1024
            }]
        );

        let tx = write_transaction(&conn).expect("tx");
        assert!(delete(&tx, "img").expect("delete").is_some());
        tx.commit().expect("commit");
        let leftovers: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM drive_letter) + (SELECT COUNT(*) FROM read_fault)",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(leftovers, 0);
    }
}
