//! 文件级派生事实:内容哈希(MD5/SHA1/SHA256/SM3)与内容类型识别(Magika),
//! 存在检材目录的 `facts.db`。
//!
//! 两者都由用户在文件列表里触发(单个、批量),写入是成批的
//! 短 upsert;重新计算代价高,而且结论可能已被引用,所以随案件持久化。没有
//! 案件时结果只留在会话里。
//!
//! - 哈希按算法逐字段合并:后算的 SM3 不会冲掉先前的 MD5;
//! - 类型识别是一个节点的完整结论,重识别整体替换该节点,其它节点保留。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};

use crate::analysis::identify::magika_ident::TypeIdentReport;
use crate::casework::case_paths::EvidenceDb;
use crate::casework::evidence_db::{self, StoreSchema};

const SCHEMA: StoreSchema = StoreSchema {
    db: EvidenceDb::Facts,
    // ASCII "LCFX".
    application_id: 0x4C43_4658,
    version: 1,
    ddl: r#"
        CREATE TABLE file_digest (
            node_id     TEXT PRIMARY KEY,
            md5         TEXT,
            sha1        TEXT,
            sha256      TEXT,
            sm3         TEXT,
            updated_at  TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE INDEX file_digest_by_sha256 ON file_digest (sha256);

        CREATE TABLE file_type (
            node_id      TEXT PRIMARY KEY,
            label        TEXT,
            mime_type    TEXT,
            group_name   TEXT,
            report_json  TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE INDEX file_type_by_label ON file_type (label);
    "#,
    busy_timeout: Duration::from_secs(10),
};

/// 一个文件已持久化的哈希。各算法独立可选——界面按需计算。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDigest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sm3: Option<String>,
}

fn node_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn digest_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, StoredDigest)> {
    Ok((
        row.get(0)?,
        StoredDigest {
            md5: row.get(1)?,
            sha1: row.get(2)?,
            sha256: row.get(3)?,
            sm3: row.get(4)?,
        },
    ))
}

fn type_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, TypeIdentReport)> {
    let node_id: String = row.get(0)?;
    let report_json: String = row.get(1)?;
    let report = serde_json::from_str::<TypeIdentReport>(&report_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok((node_id, report))
}

fn collect<T>(
    rows: impl Iterator<Item = rusqlite::Result<(String, T)>>,
) -> anyhow::Result<HashMap<String, T>> {
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn count(conn: &Connection, table: &str) -> anyhow::Result<usize> {
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })?;
    usize::try_from(count).context("facts row count")
}

// ── 哈希 ────────────────────────────────────────────────────────────────

/// 检材的全部已持久化哈希。没有 `facts.db` 时为空。
pub fn digests_load(
    case_dir: &Path,
    image_id: &str,
) -> anyhow::Result<HashMap<String, StoredDigest>> {
    let Some(conn) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(HashMap::new());
    };
    let mut statement = conn.prepare("SELECT node_id, md5, sha1, sha256, sm3 FROM file_digest")?;
    let rows = statement.query_map([], digest_from_row)?;
    collect(rows)
}

/// 一组节点(调用方限制规模)的已持久化哈希。
pub fn digests_load_nodes(
    case_dir: &Path,
    image_id: &str,
    node_ids: &[String],
) -> anyhow::Result<HashMap<String, StoredDigest>> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(conn) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(HashMap::new());
    };
    let mut statement = conn.prepare(&format!(
        "SELECT node_id, md5, sha1, sha256, sm3 FROM file_digest WHERE node_id IN ({})",
        node_placeholders(node_ids.len())
    ))?;
    let rows = statement.query_map(params_from_iter(node_ids.iter()), digest_from_row)?;
    collect(rows)
}

/// 按算法逐字段合并新哈希,返回合并后的总条数。
pub fn digests_merge(
    case_dir: &Path,
    image_id: &str,
    entries: HashMap<String, StoredDigest>,
) -> anyhow::Result<usize> {
    let conn = evidence_db::open_writer(case_dir, image_id, &SCHEMA)?;
    let tx = evidence_db::write_transaction(&conn)?;
    {
        let now = crate::casework::case_ledger::now();
        let mut statement = tx.prepare(
            "INSERT INTO file_digest (node_id, md5, sha1, sha256, sm3, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (node_id) DO UPDATE SET
                md5 = COALESCE(excluded.md5, file_digest.md5),
                sha1 = COALESCE(excluded.sha1, file_digest.sha1),
                sha256 = COALESCE(excluded.sha256, file_digest.sha256),
                sm3 = COALESCE(excluded.sm3, file_digest.sm3),
                updated_at = excluded.updated_at",
        )?;
        for (node_id, digest) in entries {
            statement.execute(params![
                node_id,
                digest.md5,
                digest.sha1,
                digest.sha256,
                digest.sm3,
                now
            ])?;
        }
    }
    tx.commit()?;
    count(&conn, "file_digest")
}

// ── 类型识别 ────────────────────────────────────────────────────────────

/// 检材的全部已持久化类型识别结论。
pub fn types_load(
    case_dir: &Path,
    image_id: &str,
) -> anyhow::Result<HashMap<String, TypeIdentReport>> {
    let Some(conn) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(HashMap::new());
    };
    let mut statement = conn.prepare("SELECT node_id, report_json FROM file_type")?;
    let rows = statement.query_map([], type_from_row)?;
    collect(rows)
}

/// 一组节点的已持久化类型识别结论。
pub fn types_load_nodes(
    case_dir: &Path,
    image_id: &str,
    node_ids: &[String],
) -> anyhow::Result<HashMap<String, TypeIdentReport>> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(conn) = evidence_db::open_reader(case_dir, image_id, &SCHEMA)? else {
        return Ok(HashMap::new());
    };
    let mut statement = conn.prepare(&format!(
        "SELECT node_id, report_json FROM file_type WHERE node_id IN ({})",
        node_placeholders(node_ids.len())
    ))?;
    let rows = statement.query_map(params_from_iter(node_ids.iter()), type_from_row)?;
    collect(rows)
}

/// 写入(按节点整体替换)类型识别结论,返回总条数。
pub fn types_merge(
    case_dir: &Path,
    image_id: &str,
    entries: HashMap<String, TypeIdentReport>,
) -> anyhow::Result<usize> {
    let conn = evidence_db::open_writer(case_dir, image_id, &SCHEMA)?;
    let tx = evidence_db::write_transaction(&conn)?;
    {
        let now = crate::casework::case_ledger::now();
        let mut statement = tx.prepare(
            "INSERT INTO file_type (node_id, label, mime_type, group_name, report_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (node_id) DO UPDATE SET
                label = excluded.label,
                mime_type = excluded.mime_type,
                group_name = excluded.group_name,
                report_json = excluded.report_json,
                updated_at = excluded.updated_at",
        )?;
        for (node_id, report) in entries {
            let report_json = serde_json::to_string(&report)?;
            statement.execute(params![
                node_id,
                report.label,
                report.mime_type,
                report.group,
                report_json,
                now
            ])?;
        }
    }
    tx.commit()?;
    count(&conn, "file_type")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(tag: &str) -> StoredDigest {
        StoredDigest {
            md5: Some(format!("{tag}-md5")),
            sha1: Some(format!("{tag}-sha1")),
            sha256: Some(format!("{tag}-sha256")),
            sm3: Some(format!("{tag}-sm3")),
        }
    }

    #[test]
    fn missing_store_loads_empty_without_creating_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(digests_load(dir.path(), "img").expect("load").is_empty());
        assert!(types_load_nodes(dir.path(), "img", &["n".into()])
            .expect("load")
            .is_empty());
        assert!(!dir.path().join("evidence").exists());
    }

    #[test]
    fn digests_merge_field_wise_and_load_by_node() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            digests_merge(
                dir.path(),
                "img",
                HashMap::from([("a".to_string(), digest("a"))])
            )
            .expect("merge"),
            1
        );
        let md5_only = StoredDigest {
            md5: Some("m".into()),
            ..StoredDigest::default()
        };
        let sm3_only = StoredDigest {
            sm3: Some("s".into()),
            ..StoredDigest::default()
        };
        digests_merge(
            dir.path(),
            "img",
            HashMap::from([("b".to_string(), md5_only)]),
        )
        .expect("md5");
        assert_eq!(
            digests_merge(
                dir.path(),
                "img",
                HashMap::from([("b".to_string(), sm3_only)])
            )
            .expect("sm3"),
            2
        );
        let loaded = digests_load(dir.path(), "img").expect("load");
        assert_eq!(
            loaded["b"],
            StoredDigest {
                md5: Some("m".into()),
                sm3: Some("s".into()),
                ..StoredDigest::default()
            }
        );
        let filtered =
            digests_load_nodes(dir.path(), "img", &["a".into(), "missing".into()]).expect("nodes");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered["a"], digest("a"));
        assert!(dir
            .path()
            .join("evidence")
            .join("img")
            .join("facts.db")
            .is_file());
    }

    #[test]
    fn type_reports_replace_per_node() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = |label: &str| TypeIdentReport {
            label: label.to_string(),
            mime_type: "application/octet-stream".to_string(),
            group: "document".to_string(),
            description: label.to_string(),
            extensions: vec![label.to_string()],
            is_text: false,
            score: 0.5,
            inferred: true,
            file_len: 10,
        };
        types_merge(
            dir.path(),
            "img",
            HashMap::from([
                ("a".to_string(), report("pdf")),
                ("b".to_string(), report("pdf")),
            ]),
        )
        .expect("merge");
        assert_eq!(
            types_merge(
                dir.path(),
                "img",
                HashMap::from([("a".to_string(), report("zip"))])
            )
            .expect("replace"),
            2
        );
        let loaded = types_load(dir.path(), "img").expect("load");
        assert_eq!(loaded["a"].label, "zip");
        assert_eq!(loaded["b"].label, "pdf");
    }
}
