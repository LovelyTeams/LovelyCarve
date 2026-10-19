//! 检材目录下按写者划分的数据库(`facts.db` / `records.db` / `analysis.db` /
//! `timeline.db` / `search.db`,见 [`crate::core::casework::case_paths::EvidenceDb`])
//! 的连接策略。
//!
//! - **写者** [`open_writer`]:必要时建检材目录与库文件,新库在一个
//!   `BEGIN IMMEDIATE` 事务里一次建全结构并盖上 `application_id` /
//!   `user_version`;已有库只校验版本,不跑 DDL。
//! - **读者** [`open_reader`]:库不存在(或尚未建好结构)返回 `None`;从不
//!   建库、不跑 DDL、不写,任务面板之类的轮询不会和写者抢写锁。
//! - 只改已有数据的写者(删除、孤儿修复)用 [`open_existing`]:不建库,忙等
//!   上限由调用方给出。
//!
//! 结构版本不符的库拒绝打开——不迁移。各库都是可重建的派生数据,出错信息
//! 指向重新生成。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::core::casework::case_paths::{self, EvidenceDb};

/// 一个检材库的结构声明。
#[derive(Debug, Clone, Copy)]
pub struct StoreSchema {
    pub db: EvidenceDb,
    /// `PRAGMA application_id`,区分各种库文件。
    pub application_id: i32,
    /// `PRAGMA user_version`。
    pub version: i32,
    /// 新库一次执行的建表语句。
    pub ddl: &'static str,
    /// 写者与读者连接的忙等上限。
    pub busy_timeout: Duration,
}

fn configure(conn: &Connection, busy_timeout: Duration) -> anyhow::Result<()> {
    conn.busy_timeout(busy_timeout)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn stamp(conn: &Connection) -> anyhow::Result<(i32, i32)> {
    let application_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok((application_id, version))
}

fn check_stamp(path: &Path, schema: &StoreSchema, stamp: (i32, i32)) -> anyhow::Result<()> {
    let (application_id, version) = stamp;
    if application_id != schema.application_id || version != schema.version {
        anyhow::bail!(
            "{} is not a {} of this build (application id {application_id:#x}, schema \
             {version}); remove the evidence from the case and add it again to rebuild it",
            path.display(),
            schema.db.file_name()
        );
    }
    Ok(())
}

/// 写者连接;库不存在时创建并建全结构。
pub fn open_writer(
    case_dir: &Path,
    image_id: &str,
    schema: &StoreSchema,
) -> anyhow::Result<Connection> {
    let path = case_paths::evidence_db_path(case_dir, image_id, schema.db);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create evidence directory {}", parent.display()))?;
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", path.display()))?;
    configure(&conn, schema.busy_timeout)?;
    let current = stamp(&conn)?;
    if current == (0, 0) {
        // 建库在进程内串行:切换到 WAL 需要排他锁,两个连接同时切换时一方
        // 可能拿不到忙等重试。案件进程锁保证没有别的进程在写这个案件。
        let _creating = CREATE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 另一个写者可能刚建完。
        if stamp(&conn)? == (0, 0) {
            // journal_mode 不能在事务里切换。
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
            tx.execute_batch(schema.ddl)
                .with_context(|| format!("create schema of {}", path.display()))?;
            tx.pragma_update(None, "application_id", schema.application_id)?;
            tx.pragma_update(None, "user_version", schema.version)?;
            tx.commit()?;
        }
        check_stamp(&path, schema, stamp(&conn)?)?;
    } else {
        check_stamp(&path, schema, current)?;
    }
    Ok(conn)
}

/// 串行化新库的建立(见 [`open_writer`])。
static CREATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 已有库的连接(不建库、不跑 DDL);库不存在或结构尚未建好时返回 `None`。
pub fn open_existing(
    case_dir: &Path,
    image_id: &str,
    schema: &StoreSchema,
    busy_timeout: Duration,
) -> anyhow::Result<Option<Connection>> {
    let path = case_paths::evidence_db_path(case_dir, image_id, schema.db);
    if !path.is_file() {
        return Ok(None);
    }
    // 读写方式打开:WAL 读者也要用共享内存索引;本函数自身从不写。
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open {}", path.display()))?;
    configure(&conn, busy_timeout)?;
    let current = stamp(&conn)?;
    if current == (0, 0) {
        return Ok(None);
    }
    check_stamp(&path, schema, current)?;
    Ok(Some(conn))
}

/// 读者连接(见模块说明)。
pub fn open_reader(
    case_dir: &Path,
    image_id: &str,
    schema: &StoreSchema,
) -> anyhow::Result<Option<Connection>> {
    open_existing(case_dir, image_id, schema, schema.busy_timeout)
}

/// 在写者连接上开 `BEGIN IMMEDIATE` 事务。
pub fn write_transaction(conn: &Connection) -> rusqlite::Result<Transaction<'_>> {
    Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SCHEMA: StoreSchema = StoreSchema {
        db: EvidenceDb::Facts,
        application_id: 0x7465_7374,
        version: 3,
        ddl: "CREATE TABLE item (id INTEGER PRIMARY KEY, value TEXT NOT NULL);",
        busy_timeout: Duration::from_secs(1),
    };

    #[test]
    fn readers_never_create_and_writers_create_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(open_reader(directory.path(), "img", &TEST_SCHEMA)
            .expect("reader")
            .is_none());
        assert!(
            !directory.path().join("evidence").exists(),
            "reader created nothing"
        );

        let writer = open_writer(directory.path(), "img", &TEST_SCHEMA).expect("writer");
        writer
            .execute("INSERT INTO item (value) VALUES ('a')", [])
            .expect("insert");
        drop(writer);
        // Reopening an existing database runs no DDL.
        drop(open_writer(directory.path(), "img", &TEST_SCHEMA).expect("reopen"));
        let reader = open_reader(directory.path(), "img", &TEST_SCHEMA)
            .expect("reader")
            .expect("exists");
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM item", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let journal: String = reader
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal, "wal");
    }

    #[test]
    fn other_versions_are_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        drop(open_writer(directory.path(), "img", &TEST_SCHEMA).expect("writer"));
        let newer = StoreSchema {
            version: 4,
            ..TEST_SCHEMA
        };
        let error = open_writer(directory.path(), "img", &newer).expect_err("other version");
        assert!(format!("{error:#}").contains("add it again to rebuild it"));
        assert!(open_reader(directory.path(), "img", &newer).is_err());
    }
}
