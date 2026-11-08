//! 案件台账 `case.db`:连接策略、结构与案件档案 / 设置。
//!
//! 台账是案件里唯一需要备份的库,只放与检材数量或人工操作次数同阶的行:
//! 案件档案、检材登记(`core::casework::case_ledger_evidence`)、卷发布记录
//! (`core::casework::case_ledger_volumes`)、插件任务(`core::extensions::task_scheduler`)、人工标签
//! (`core::casework::tag_store`)、检材级事实(盘符、读故障)、设置与哈希链审计
//! (`core::casework::case_journal`)。与文件数量同阶的数据都在 `evidence/<uid>/` 下。
//!
//! 写入纪律:台账上的每个写事务都是毫秒级的;读-改-写一律 `BEGIN IMMEDIATE`
//! 开局即取写锁,不会在读锁升级时撞上 `SQLITE_BUSY`;事务里不做文件 I/O、
//! 不等其它锁。于是台账写者之间只有短暂排队,忙等上限 [`BUSY_TIMEOUT`] 足够。
//!
//! 版本:`application_id` 标识这是 LovelyCarve 案件台账,`user_version` 是结构
//! 版本。不是当前版本的台账拒绝打开——不迁移、不兼容。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};

/// `PRAGMA application_id`:ASCII "LCCD"(LovelyCarve Case Database)。
pub const APPLICATION_ID: i32 = 0x4C43_4344;
/// 台账结构版本。
pub const SCHEMA_VERSION: i32 = 1;
/// 台账连接的忙等上限。台账只有短事务,正常情况下等不满。
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// 台账结构。外键一律 `ON DELETE CASCADE` 到检材:删除检材行即带走它的
/// 卷发布、标签、盘符、读故障与任务;审计链只记检材 uid,不随检材删除。
const SCHEMA: &str = r#"
CREATE TABLE case_profile (
    singleton    INTEGER PRIMARY KEY CHECK (singleton = 1),
    case_uid     TEXT NOT NULL,
    case_number  TEXT NOT NULL,
    name         TEXT NOT NULL,
    examiner     TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    locked       INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE setting (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE evidence (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    uid         TEXT NOT NULL UNIQUE,
    path        TEXT NOT NULL,
    path_key    TEXT NOT NULL UNIQUE,
    format      TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
    platform    TEXT,
    kind        TEXT,
    detection   TEXT,
    added_at    TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE volume (
    evidence_id     INTEGER NOT NULL REFERENCES evidence (id) ON DELETE CASCADE,
    volume_key      TEXT NOT NULL,
    vol_id          INTEGER NOT NULL,
    source_size     INTEGER NOT NULL CHECK (source_size >= 0),
    fingerprint     TEXT NOT NULL,
    snapshot_file   TEXT NOT NULL,
    built_at        TEXT NOT NULL,
    entry_count     INTEGER NOT NULL CHECK (entry_count >= 0),
    truncated       INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    index_file      TEXT,
    index_built_at  TEXT,
    PRIMARY KEY (evidence_id, volume_key)
) WITHOUT ROWID;

CREATE TABLE tag (
    evidence_id  INTEGER NOT NULL REFERENCES evidence (id) ON DELETE CASCADE,
    node_id      TEXT NOT NULL,
    label        TEXT NOT NULL,
    tagged_at    TEXT NOT NULL,
    PRIMARY KEY (evidence_id, node_id)
) WITHOUT ROWID;
CREATE INDEX tag_by_label ON tag (evidence_id, label);

CREATE TABLE drive_letter (
    evidence_id   INTEGER NOT NULL REFERENCES evidence (id) ON DELETE CASCADE,
    partition_id  INTEGER NOT NULL,
    letter        TEXT NOT NULL,
    ambiguous     INTEGER NOT NULL CHECK (ambiguous IN (0, 1)),
    PRIMARY KEY (evidence_id, partition_id, letter)
) WITHOUT ROWID;

CREATE TABLE read_fault (
    evidence_id  INTEGER NOT NULL REFERENCES evidence (id) ON DELETE CASCADE,
    offset       INTEGER NOT NULL CHECK (offset >= 0),
    length       INTEGER NOT NULL CHECK (length > 0),
    PRIMARY KEY (evidence_id, offset)
) WITHOUT ROWID;

CREATE TABLE task (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    evidence_id      INTEGER NOT NULL REFERENCES evidence (id) ON DELETE CASCADE,
    function         TEXT NOT NULL,
    plugin_id        TEXT NOT NULL,
    analyzer         TEXT NOT NULL,
    display_name     TEXT NOT NULL,
    kind             TEXT NOT NULL,
    depends_on       TEXT NOT NULL DEFAULT '',
    trigger          TEXT NOT NULL CHECK (trigger IN ('auto', 'manually')),
    parent_id        INTEGER,
    params           TEXT NOT NULL,
    status           TEXT NOT NULL CHECK (status IN (
                         'created', 'queued', 'running', 'success',
                         'failed', 'cancel', 'pause', 'offline')),
    progress         INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    remain_secs      INTEGER,
    error            TEXT,
    result           TEXT,
    can_pause        INTEGER NOT NULL CHECK (can_pause IN (0, 1)),
    single_instance  INTEGER NOT NULL CHECK (single_instance IN (0, 1)),
    internal         INTEGER NOT NULL CHECK (internal IN (0, 1)),
    created_at       TEXT NOT NULL,
    started_at       TEXT,
    finished_at      TEXT,
    updated_at       TEXT NOT NULL
);
CREATE INDEX task_by_status ON task (status, id);
CREATE INDEX task_by_evidence ON task (evidence_id, id);
CREATE UNIQUE INDEX task_single_active ON task (evidence_id, function)
    WHERE single_instance = 1
      AND status IN ('created', 'queued', 'running', 'pause', 'offline');

CREATE TABLE task_followup (
    evidence_id  INTEGER PRIMARY KEY REFERENCES evidence (id) ON DELETE CASCADE,
    actions      TEXT NOT NULL,
    done_at      TEXT
);

CREATE TABLE journal (
    seq       INTEGER PRIMARY KEY,
    at        TEXT NOT NULL,
    actor     TEXT NOT NULL,
    action    TEXT NOT NULL,
    evidence  TEXT,
    detail    TEXT NOT NULL,
    hash      TEXT NOT NULL
);
"#;

/// 台账里的 UTC 时间戳:RFC 3339、毫秒精度、`Z` 结尾。字典序即时间序。
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 两个台账时间戳之间的整秒数;任一无法解析时为 `None`。
pub fn seconds_between(start: &str, end: &str) -> Option<i64> {
    let start = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    Some((end - start).num_seconds().max(0))
}

/// 案件档案(`case_profile` 唯一一行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseProfile {
    pub case_uid: String,
    pub case_number: String,
    pub name: String,
    pub examiner: String,
    pub description: String,
    pub locked: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// 在台账上开一个写事务(`BEGIN IMMEDIATE`)。连接只有共享借用时用它代替
/// `Connection::transaction`。
pub fn write_transaction(conn: &Connection) -> rusqlite::Result<Transaction<'_>> {
    Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
}

fn configure(conn: &Connection, busy_timeout: Duration) -> anyhow::Result<()> {
    conn.busy_timeout(busy_timeout)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn validate(conn: &Connection, path: &Path) -> anyhow::Result<()> {
    let application_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id != APPLICATION_ID {
        anyhow::bail!("{} is not a LovelyCarve case database", path.display());
    }
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        anyhow::bail!(
            "case database {} has schema {version}, this build supports {SCHEMA_VERSION}; \
             create a new case",
            path.display()
        );
    }
    Ok(())
}

/// 建一个新台账并写入案件档案。文件已存在时失败,绝不覆盖已有案件。
pub fn create(path: &Path, profile: &CaseProfile) -> anyhow::Result<Connection> {
    if path.exists() {
        anyhow::bail!("case database {} already exists", path.display());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("create case database {}", path.display()))?;
    configure(&conn, BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    let tx = write_transaction(&conn)?;
    tx.execute_batch(SCHEMA)?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.execute(
        "INSERT INTO case_profile (singleton, case_uid, case_number, name, examiner, description,
                                   locked, created_at, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            profile.case_uid,
            profile.case_number,
            profile.name,
            profile.examiner,
            profile.description,
            profile.locked,
            profile.created_at,
            profile.updated_at,
        ],
    )?;
    tx.commit()?;
    Ok(conn)
}

/// 打开已有台账。不存在、不是 LovelyCarve 台账或结构版本不符时失败。
pub fn open(path: &Path) -> anyhow::Result<Connection> {
    open_with_timeout(path, BUSY_TIMEOUT)
}

/// [`open`],指定忙等上限。
pub fn open_with_timeout(path: &Path, busy_timeout: Duration) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open case database {}", path.display()))?;
    configure(&conn, busy_timeout)?;
    validate(&conn, path)?;
    Ok(conn)
}

pub fn load_profile(conn: &Connection) -> anyhow::Result<CaseProfile> {
    conn.query_row(
        "SELECT case_uid, case_number, name, examiner, description, locked, created_at, updated_at
         FROM case_profile WHERE singleton = 1",
        [],
        |row| {
            Ok(CaseProfile {
                case_uid: row.get(0)?,
                case_number: row.get(1)?,
                name: row.get(2)?,
                examiner: row.get(3)?,
                description: row.get(4)?,
                locked: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    )
    .context("read case profile")
}

/// 改案件名称。
pub fn set_name(tx: &Transaction<'_>, name: &str) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE case_profile SET name = ?1, updated_at = ?2 WHERE singleton = 1",
        params![name, now()],
    )?;
    Ok(())
}

/// 翻转逻辑锁(锁定后拒绝改名、移除检材与重建快照)。
pub fn set_locked(tx: &Transaction<'_>, locked: bool) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE case_profile SET locked = ?1, updated_at = ?2 WHERE singleton = 1",
        params![locked, now()],
    )?;
    Ok(())
}

/// 刷新档案的更新时间(检材增删等案件内容变化)。
pub fn touch(tx: &Transaction<'_>) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE case_profile SET updated_at = ?1 WHERE singleton = 1",
        params![now()],
    )?;
    Ok(())
}

/// 案件级设置;`prefix` 为空返回全部,按键排序。
pub fn settings_query(conn: &Connection, prefix: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut upper = prefix.to_string();
    upper.push(char::MAX);
    let mut statement =
        conn.prepare("SELECT key, value FROM setting WHERE key >= ?1 AND key < ?2 ORDER BY key")?;
    let rows = statement.query_map(params![prefix, upper], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 写入案件级设置(upsert);空键忽略。
pub fn settings_save(conn: &Connection, entries: &[(String, String)]) -> anyhow::Result<()> {
    let tx = write_transaction(conn)?;
    {
        let mut statement = tx.prepare(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in entries {
            if key.is_empty() {
                continue;
            }
            statement.execute(params![key, value])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 读取单个设置。
pub fn setting(conn: &Connection, key: &str) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM setting WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// 检材整数主键;检材不在案件中时为 `None`。
pub fn evidence_id(conn: &Connection, image_id: &str) -> anyhow::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM evidence WHERE uid = ?1",
        params![image_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    pub fn profile(name: &str) -> CaseProfile {
        let now = now();
        CaseProfile {
            case_uid: uuid::Uuid::new_v4().to_string(),
            case_number: "LC-TEST".to_string(),
            name: name.to_string(),
            examiner: "examiner".to_string(),
            description: String::new(),
            locked: false,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// A fresh ledger in `directory` with one test profile.
    pub fn ledger(directory: &Path) -> Connection {
        create(&directory.join("case.db"), &profile("test")).expect("create ledger")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn create_stamps_identity_and_round_trips_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("case.db");
        let expected = profile("案件甲");
        drop(create(&path, &expected).expect("create"));
        assert!(create(&path, &expected).is_err(), "never clobbers a case");

        let conn = open(&path).expect("open");
        assert_eq!(load_profile(&conn).expect("profile"), expected);
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal_mode, "wal");

        let tx = write_transaction(&conn).expect("tx");
        set_name(&tx, "案件乙").expect("rename");
        set_locked(&tx, true).expect("lock");
        tx.commit().expect("commit");
        let renamed = load_profile(&conn).expect("profile");
        assert_eq!((renamed.name.as_str(), renamed.locked), ("案件乙", true));
    }

    #[test]
    fn foreign_and_future_databases_are_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let foreign = directory.path().join("foreign.db");
        Connection::open(&foreign)
            .expect("foreign")
            .execute_batch("CREATE TABLE x (y);")
            .expect("seed");
        let error = open(&foreign).expect_err("foreign file");
        assert!(format!("{error:#}").contains("not a LovelyCarve case database"));

        let path = directory.path().join("case.db");
        drop(create(&path, &profile("x")).expect("create"));
        Connection::open(&path)
            .expect("raw")
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .expect("stamp");
        let error = open(&path).expect_err("other schema");
        assert!(format!("{error:#}").contains("create a new case"));
    }

    #[test]
    fn settings_upsert_and_prefix_query() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = ledger(directory.path());
        settings_save(
            &conn,
            &[
                ("ui.theme".into(), "dark".into()),
                ("export.timeout".into(), "30".into()),
                (String::new(), "ignored".into()),
            ],
        )
        .expect("save");
        settings_save(&conn, &[("ui.theme".into(), "pink".into())]).expect("update");
        assert_eq!(
            setting(&conn, "ui.theme").expect("get"),
            Some("pink".into())
        );
        assert_eq!(settings_query(&conn, "").expect("all").len(), 2);
        assert_eq!(
            settings_query(&conn, "ui.").expect("prefix"),
            vec![("ui.theme".to_string(), "pink".to_string())]
        );
    }

    #[test]
    fn timestamps_sort_and_measure() {
        let first = now();
        assert!(first.ends_with('Z'));
        assert_eq!(
            seconds_between("2026-10-09T00:00:00.000Z", "2026-10-09T00:01:05.500Z"),
            Some(65)
        );
        assert_eq!(seconds_between("bad", "2026-10-09T00:00:00Z"), None);
    }
}
