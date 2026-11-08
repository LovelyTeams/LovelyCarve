//! 人工文件标签(重点 / 可疑 / 已排除),存在案件台账的 `tag` 表。
//!
//! 标签是检验人的工作成果,随台账备份;外键级联到检材,移除检材即带走它的
//! 标签。界面按节点增量写入(当前页的增删),整体替换只作为兼容入口保留。
//! 没有案件时标签只留在前端本机存储里。标签值是自由文本,前端按自己的
//! 标签集校验,增加新的标签种类不需要改结构。

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use rusqlite::{params, params_from_iter, Connection};

use crate::casework::case_ledger::{self, evidence_id};
use crate::casework::case_paths;

fn open_ledger(case_dir: &Path) -> anyhow::Result<Connection> {
    case_ledger::open(&case_paths::ledger_path(case_dir))
}

fn collect(
    conn: &Connection,
    sql: &str,
    binds: impl IntoIterator<Item = String>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params_from_iter(binds), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(Into::into)
}

fn total(conn: &Connection, evidence: i64) -> anyhow::Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tag WHERE evidence_id = ?1",
        params![evidence],
        |row| row.get(0),
    )?;
    usize::try_from(count).context("tag count")
}

/// 检材的全部标签,按节点 id。
pub fn load(case_dir: &Path, image_id: &str) -> anyhow::Result<HashMap<String, String>> {
    let conn = open_ledger(case_dir)?;
    collect(
        &conn,
        "SELECT t.node_id, t.label FROM tag t JOIN evidence e ON e.id = t.evidence_id
         WHERE e.uid = ?",
        [image_id.to_string()],
    )
}

/// 一组节点(调用方限制规模)的标签。
pub fn load_nodes(
    case_dir: &Path,
    image_id: &str,
    node_ids: &[String],
) -> anyhow::Result<HashMap<String, String>> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open_ledger(case_dir)?;
    let placeholders = std::iter::repeat_n("?", node_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    collect(
        &conn,
        &format!(
            "SELECT t.node_id, t.label FROM tag t JOIN evidence e ON e.id = t.evidence_id
             WHERE e.uid = ? AND t.node_id IN ({placeholders})"
        ),
        std::iter::once(image_id.to_string()).chain(node_ids.iter().cloned()),
    )
}

/// 在一个事务里删除 `delete_node_ids` 并写入 `upserts`。返回检材的标签总数;
/// 检材不在案件中时返回 `None`。
pub fn apply_changes(
    case_dir: &Path,
    image_id: &str,
    upserts: HashMap<String, String>,
    delete_node_ids: Vec<String>,
) -> anyhow::Result<Option<usize>> {
    let conn = open_ledger(case_dir)?;
    let tx = case_ledger::write_transaction(&conn)?;
    let Some(evidence) = evidence_id(&tx, image_id)? else {
        return Ok(None);
    };
    {
        let mut delete = tx.prepare("DELETE FROM tag WHERE evidence_id = ?1 AND node_id = ?2")?;
        for node_id in delete_node_ids {
            delete.execute(params![evidence, node_id])?;
        }
        let now = case_ledger::now();
        let mut upsert = tx.prepare(
            "INSERT INTO tag (evidence_id, node_id, label, tagged_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (evidence_id, node_id) DO UPDATE SET
                label = excluded.label, tagged_at = excluded.tagged_at",
        )?;
        for (node_id, label) in upserts {
            upsert.execute(params![evidence, node_id, label, now])?;
        }
    }
    let count = total(&tx, evidence)?;
    tx.commit()?;
    Ok(Some(count))
}

/// 用 `entries` 整体替换检材的标签(空表即清空)。检材不在案件中时返回
/// `None`。
pub fn save(
    case_dir: &Path,
    image_id: &str,
    entries: HashMap<String, String>,
) -> anyhow::Result<Option<usize>> {
    let conn = open_ledger(case_dir)?;
    let tx = case_ledger::write_transaction(&conn)?;
    let Some(evidence) = evidence_id(&tx, image_id)? else {
        return Ok(None);
    };
    tx.execute("DELETE FROM tag WHERE evidence_id = ?1", params![evidence])?;
    {
        let now = case_ledger::now();
        let mut insert = tx.prepare(
            "INSERT INTO tag (evidence_id, node_id, label, tagged_at) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (node_id, label) in entries {
            insert.execute(params![evidence, node_id, label, now])?;
        }
    }
    let count = total(&tx, evidence)?;
    tx.commit()?;
    Ok(Some(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casework::case_ledger::test_support;
    use crate::casework::case_ledger_evidence::test_support::add;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(node, tag)| (node.to_string(), tag.to_string()))
            .collect()
    }

    fn case_with(image_id: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(dir.path());
        add(&conn, image_id);
        dir
    }

    #[test]
    fn save_replaces_and_apply_changes_merges() {
        let dir = case_with("img");
        assert_eq!(
            save(dir.path(), "img", tags(&[("a", "重点"), ("b", "可疑")])).expect("save"),
            Some(2)
        );
        assert_eq!(
            save(dir.path(), "img", tags(&[("a", "已排除")])).expect("replace"),
            Some(1)
        );
        assert_eq!(
            apply_changes(
                dir.path(),
                "img",
                tags(&[("new", "重点"), ("a", "可疑")]),
                vec!["missing".to_string()],
            )
            .expect("apply"),
            Some(2)
        );
        let loaded = load(dir.path(), "img").expect("load");
        assert_eq!(loaded, tags(&[("a", "可疑"), ("new", "重点")]));
        assert_eq!(
            load_nodes(dir.path(), "img", &["new".into(), "nope".into()]).expect("nodes"),
            tags(&[("new", "重点")])
        );
        assert_eq!(
            apply_changes(
                dir.path(),
                "img",
                HashMap::new(),
                vec!["a".into(), "new".into()]
            )
            .expect("delete all"),
            Some(0)
        );
        assert!(load(dir.path(), "img").expect("load").is_empty());
    }

    #[test]
    fn evidence_outside_the_case_is_not_written() {
        let dir = case_with("img");
        assert_eq!(
            apply_changes(dir.path(), "other", tags(&[("a", "重点")]), Vec::new()).expect("apply"),
            None
        );
        assert!(load(dir.path(), "other").expect("load").is_empty());
    }
}
