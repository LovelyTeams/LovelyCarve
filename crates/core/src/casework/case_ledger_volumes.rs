//! 台账中的卷发布记录:每个 `(检材, 卷)` 当前生效的快照与索引文件。
//!
//! 发布分两步,各是一条短事务:
//!
//! 1. [`publish_snapshot`] —— 走树完成、`.dirz` 移入卷目录后登记。昂贵的走树
//!    结果因此立刻持久,后续索引失败或案件关闭都不会丢。快照内容(源大小、
//!    证据指纹)未变时保留旧索引继续服务,直到新索引发布;内容变了则旧索引
//!    立即下线。
//! 2. [`publish_index`] —— 由当前快照导入的索引文件移入卷目录后登记。登记以
//!    「索引来自的快照仍是当前快照」为条件,过期的构建不会覆盖新结果。
//!
//! 卷文件按记录里的文件名寻址;记录之外的文件都是可回收的旧代
//! (`core::casework::case` 打开时清理)。

use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::casework::case_ledger::{self, evidence_id};
use crate::casework::case_paths;

/// 一个卷当前的发布状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRecord {
    pub image_id: String,
    pub volume_key: String,
    /// 镜像文件系统数组下标(`fs_all`),MetaAddr 寻址用。
    pub vol_id: i64,
    /// 构建时的源卷大小,与已加载镜像比对判断是否过期。
    pub source_size: u64,
    pub fingerprint: String,
    /// 卷目录下的快照文件名。
    pub snapshot_file: String,
    /// 快照构建(走树)时间,RFC 3339。
    pub built_at: String,
    pub entry_count: u64,
    pub truncated: bool,
    /// 当前服务的索引文件名;`None` 表示没有可用索引。
    pub index_file: Option<String>,
    /// 当前索引所依据快照的 `built_at`。
    pub index_built_at: Option<String>,
}

impl VolumeRecord {
    /// 索引存在且来自当前快照。
    pub fn index_is_current(&self) -> bool {
        self.index_file.is_some() && self.index_built_at.as_deref() == Some(self.built_at.as_str())
    }
}

/// [`publish_snapshot`] 的输入。
#[derive(Debug, Clone)]
pub struct SnapshotPublication {
    pub image_id: String,
    pub volume_key: String,
    pub vol_id: i64,
    pub source_size: u64,
    pub fingerprint: String,
    pub snapshot_file: String,
    pub built_at: String,
    pub entry_count: u64,
    pub truncated: bool,
}

/// 快照发布结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// 已发布;`retired` 是不再被记录引用、可回收的旧文件名。
    Published {
        record: Box<VolumeRecord>,
        retired: Vec<String>,
    },
    /// 记录里已有更新的快照,本次结果作废。
    Superseded,
    /// 检材已不在案件中。
    EvidenceGone,
}

/// 索引发布结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOutcome {
    Published {
        record: Box<VolumeRecord>,
        retired: Option<String>,
    },
    /// 当前快照已不是索引所依据的那份(或卷已被删除),本次索引作废。
    Stale,
}

const COLUMNS: &str = "e.uid, v.volume_key, v.vol_id, v.source_size, v.fingerprint, \
v.snapshot_file, v.built_at, v.entry_count, v.truncated, v.index_file, v.index_built_at";

fn record_from_row(row: &Row<'_>) -> rusqlite::Result<VolumeRecord> {
    let source_size: i64 = row.get(3)?;
    let entry_count: i64 = row.get(7)?;
    Ok(VolumeRecord {
        image_id: row.get(0)?,
        volume_key: row.get(1)?,
        vol_id: row.get(2)?,
        source_size: u64::try_from(source_size).unwrap_or(0),
        fingerprint: row.get(4)?,
        snapshot_file: row.get(5)?,
        built_at: row.get(6)?,
        entry_count: u64::try_from(entry_count).unwrap_or(0),
        truncated: row.get(8)?,
        index_file: row.get(9)?,
        index_built_at: row.get(10)?,
    })
}

/// 全部卷记录,按检材登记顺序、卷键排序。
pub fn list(conn: &Connection) -> anyhow::Result<Vec<VolumeRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM volume v JOIN evidence e ON e.id = v.evidence_id
         ORDER BY e.id, v.volume_key"
    ))?;
    let rows = statement.query_map([], record_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 一个检材的全部卷记录。
pub fn list_for(conn: &Connection, image_id: &str) -> anyhow::Result<Vec<VolumeRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM volume v JOIN evidence e ON e.id = v.evidence_id
         WHERE e.uid = ?1 ORDER BY v.volume_key"
    ))?;
    let rows = statement.query_map(params![image_id], record_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get(
    conn: &Connection,
    image_id: &str,
    volume_key: &str,
) -> anyhow::Result<Option<VolumeRecord>> {
    conn.query_row(
        &format!(
            "SELECT {COLUMNS} FROM volume v JOIN evidence e ON e.id = v.evidence_id
             WHERE e.uid = ?1 AND v.volume_key = ?2"
        ),
        params![image_id, volume_key],
        record_from_row,
    )
    .optional()
    .map_err(Into::into)
}

/// 登记一份新快照。
pub fn publish_snapshot(
    tx: &Transaction<'_>,
    publication: &SnapshotPublication,
) -> anyhow::Result<SnapshotOutcome> {
    let Some(evidence) = evidence_id(tx, &publication.image_id)? else {
        return Ok(SnapshotOutcome::EvidenceGone);
    };
    let previous = get(tx, &publication.image_id, &publication.volume_key)?;
    if previous
        .as_ref()
        .is_some_and(|previous| previous.built_at > publication.built_at)
    {
        return Ok(SnapshotOutcome::Superseded);
    }
    let same_content = previous.as_ref().is_some_and(|previous| {
        previous.fingerprint == publication.fingerprint
            && previous.source_size == publication.source_size
            && previous.vol_id == publication.vol_id
    });
    let (index_file, index_built_at) = match &previous {
        Some(previous) if same_content => {
            (previous.index_file.clone(), previous.index_built_at.clone())
        }
        Some(_) | None => (None, None),
    };
    let mut retired = Vec::new();
    if let Some(previous) = &previous {
        if previous.snapshot_file != publication.snapshot_file {
            retired.push(previous.snapshot_file.clone());
        }
        if !same_content {
            retired.extend(previous.index_file.clone());
        }
    }
    tx.execute(
        "INSERT INTO volume (evidence_id, volume_key, vol_id, source_size, fingerprint,
                             snapshot_file, built_at, entry_count, truncated,
                             index_file, index_built_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (evidence_id, volume_key) DO UPDATE SET
            vol_id = excluded.vol_id,
            source_size = excluded.source_size,
            fingerprint = excluded.fingerprint,
            snapshot_file = excluded.snapshot_file,
            built_at = excluded.built_at,
            entry_count = excluded.entry_count,
            truncated = excluded.truncated,
            index_file = excluded.index_file,
            index_built_at = excluded.index_built_at",
        params![
            evidence,
            publication.volume_key,
            publication.vol_id,
            i64::try_from(publication.source_size).context("source volume size exceeds i64")?,
            publication.fingerprint,
            publication.snapshot_file,
            publication.built_at,
            i64::try_from(publication.entry_count).context("snapshot entry count exceeds i64")?,
            publication.truncated,
            index_file,
            index_built_at,
        ],
    )?;
    let record = get(tx, &publication.image_id, &publication.volume_key)?
        .context("published volume row vanished")?;
    Ok(SnapshotOutcome::Published {
        record: Box::new(record),
        retired,
    })
}

/// 登记由 `built_at` 那份快照导入的索引文件。
pub fn publish_index(
    tx: &Transaction<'_>,
    image_id: &str,
    volume_key: &str,
    built_at: &str,
    index_file: &str,
) -> anyhow::Result<IndexOutcome> {
    let Some(previous) = get(tx, image_id, volume_key)? else {
        return Ok(IndexOutcome::Stale);
    };
    if previous.built_at != built_at {
        return Ok(IndexOutcome::Stale);
    }
    tx.execute(
        "UPDATE volume SET index_file = ?3, index_built_at = ?4
         WHERE evidence_id = (SELECT id FROM evidence WHERE uid = ?1) AND volume_key = ?2",
        params![image_id, volume_key, index_file, built_at],
    )?;
    let record = get(tx, image_id, volume_key)?.context("indexed volume row vanished")?;
    let retired = previous
        .index_file
        .filter(|previous_file| previous_file != index_file);
    Ok(IndexOutcome::Published {
        record: Box::new(record),
        retired,
    })
}

/// 让卷不再服务索引(索引文件缺失时自愈用)。
pub fn clear_index(tx: &Transaction<'_>, image_id: &str, volume_key: &str) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE volume SET index_file = NULL, index_built_at = NULL
         WHERE evidence_id = (SELECT id FROM evidence WHERE uid = ?1) AND volume_key = ?2",
        params![image_id, volume_key],
    )?;
    Ok(())
}

/// 一个卷当前服务的索引文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedVolume {
    pub volume_key: String,
    pub vol_id: i64,
    /// 索引文件的完整路径。
    pub index: PathBuf,
    /// 索引所依据快照的 `built_at`。同内容重建后、新索引发布前,它早于卷记录
    /// 的 `built_at`——内容相同,旧索引照常服务。
    pub built_at: String,
}

/// 检材当前有索引可用的卷,按卷键排序。只读台账;案件目录没有台账(案件外
/// 的临时目录)时为空。
pub fn indexed_volumes(case_dir: &Path, image_id: &str) -> anyhow::Result<Vec<IndexedVolume>> {
    let ledger_path = case_paths::ledger_path(case_dir);
    if !ledger_path.is_file() {
        return Ok(Vec::new());
    }
    let conn = case_ledger::open(&ledger_path)?;
    Ok(list_for(&conn, image_id)?
        .into_iter()
        .filter_map(|record| {
            let index_file = record.index_file?;
            let built_at = record.index_built_at?;
            Some(IndexedVolume {
                index: case_paths::volume_dir(case_dir, image_id, &record.volume_key)
                    .join(index_file),
                volume_key: record.volume_key,
                vol_id: record.vol_id,
                built_at,
            })
        })
        .collect())
}

/// 删除卷记录,返回被删的记录。
pub fn delete(
    tx: &Transaction<'_>,
    image_id: &str,
    volume_key: &str,
) -> anyhow::Result<Option<VolumeRecord>> {
    let Some(existing) = get(tx, image_id, volume_key)? else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM volume
         WHERE evidence_id = (SELECT id FROM evidence WHERE uid = ?1) AND volume_key = ?2",
        params![image_id, volume_key],
    )?;
    Ok(Some(existing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casework::case_ledger::{test_support, write_transaction};
    use crate::casework::case_ledger_evidence::test_support::add;

    fn publication(built_at: &str, snapshot_file: &str, fingerprint: &str) -> SnapshotPublication {
        SnapshotPublication {
            image_id: "img".into(),
            volume_key: "part:1".into(),
            vol_id: 0,
            source_size: 4096,
            fingerprint: fingerprint.into(),
            snapshot_file: snapshot_file.into(),
            built_at: built_at.into(),
            entry_count: 10,
            truncated: false,
        }
    }

    fn snapshot(conn: &Connection, publication: &SnapshotPublication) -> SnapshotOutcome {
        let tx = write_transaction(conn).expect("tx");
        let outcome = publish_snapshot(&tx, publication).expect("publish");
        tx.commit().expect("commit");
        outcome
    }

    fn index(conn: &Connection, built_at: &str, file: &str) -> IndexOutcome {
        let tx = write_transaction(conn).expect("tx");
        let outcome = publish_index(&tx, "img", "part:1", built_at, file).expect("index");
        tx.commit().expect("commit");
        outcome
    }

    #[test]
    fn index_follows_its_snapshot_and_survives_a_same_content_rebuild() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        add(&conn, "img");

        let first = publication("2026-10-09T01:00:00.000Z", "tree-a.dirz", "fp");
        assert!(matches!(
            snapshot(&conn, &first),
            SnapshotOutcome::Published { ref retired, .. } if retired.is_empty()
        ));
        let IndexOutcome::Published { record, retired } =
            index(&conn, &first.built_at, "index-a.db")
        else {
            panic!("index of the current snapshot publishes");
        };
        assert!(record.index_is_current());
        assert_eq!(retired, None);

        // Same content rebuilt: the old index keeps serving until replaced.
        let second = publication("2026-10-09T02:00:00.000Z", "tree-b.dirz", "fp");
        let SnapshotOutcome::Published { record, retired } = snapshot(&conn, &second) else {
            panic!("newer snapshot publishes");
        };
        assert_eq!(retired, vec!["tree-a.dirz".to_string()]);
        assert_eq!(record.index_file.as_deref(), Some("index-a.db"));
        assert!(!record.index_is_current());

        // An index built from the retired snapshot is stale.
        assert_eq!(
            index(&conn, &first.built_at, "index-late.db"),
            IndexOutcome::Stale
        );
        let IndexOutcome::Published { retired, .. } = index(&conn, &second.built_at, "index-b.db")
        else {
            panic!("index of the current snapshot publishes");
        };
        assert_eq!(retired.as_deref(), Some("index-a.db"));

        // An older build never replaces a newer one.
        assert_eq!(snapshot(&conn, &first), SnapshotOutcome::Superseded);
    }

    #[test]
    fn changed_content_retires_the_index_and_deleted_evidence_refuses() {
        let directory = tempfile::tempdir().expect("tempdir");
        let conn = test_support::ledger(directory.path());
        add(&conn, "img");
        let first = publication("2026-10-09T01:00:00.000Z", "tree-a.dirz", "fp-1");
        snapshot(&conn, &first);
        index(&conn, &first.built_at, "index-a.db");

        let changed = publication("2026-10-09T02:00:00.000Z", "tree-b.dirz", "fp-2");
        let SnapshotOutcome::Published { record, retired } = snapshot(&conn, &changed) else {
            panic!("publishes");
        };
        assert_eq!(record.index_file, None);
        assert_eq!(
            retired,
            vec!["tree-a.dirz".to_string(), "index-a.db".to_string()]
        );

        let tx = write_transaction(&conn).expect("tx");
        crate::casework::case_ledger_evidence::delete(&tx, "img").expect("delete evidence");
        tx.commit().expect("commit");
        assert!(list(&conn).expect("list").is_empty(), "rows cascade");
        assert_eq!(snapshot(&conn, &changed), SnapshotOutcome::EvidenceGone);
    }
}
