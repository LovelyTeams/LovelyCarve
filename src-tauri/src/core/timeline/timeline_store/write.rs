//! write implementation.
use super::*;

// ── 投影写入（重建任务 / 插件收尾钩子）──────────────────────────────────

/// 批量插入投影行(单事务)。调用方按批切片以流式落库大检材,批间释放写锁,
/// 插件收尾钩子可以插进来。
pub fn insert_rows(conn: &Connection, rows: &[TimelineInsertRow]) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = evidence_db::write_transaction(conn)?;
    write_rows(&tx, rows)?;
    tx.commit()?;
    Ok(())
}

/// 在调用方的事务里插入投影行。
pub(super) fn write_rows(conn: &Connection, rows: &[TimelineInsertRow]) -> anyhow::Result<()> {
    let mut statement = conn.prepare_cached(
        "INSERT INTO timeline (
            oid, tid, \"delete\", tl_time, from_app, type, tl_flag, content, extra
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
    )?;
    for row in rows {
        let params: [&dyn ToSql; 9] = [
            &row.oid,
            &row.tid,
            &row.deleted,
            &row.tl_time,
            &row.from_app,
            &row.data_type,
            &row.tl_flag,
            &row.content,
            &row.extra,
        ];
        statement.execute(params.as_slice())?;
    }
    Ok(())
}

/// 清空全部投影(重建的幂等基线;无 WHERE 的 DELETE 走整表截断)。
pub fn clear(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("DELETE FROM timeline", [])?;
    Ok(())
}

/// 删除一个来源任务(run)的投影,返回删除行数。
pub(super) fn delete_run(conn: &Connection, tid: &str) -> anyhow::Result<u64> {
    let removed = conn.execute("DELETE FROM timeline WHERE tid = ?1", [tid])?;
    Ok(removed as u64)
}

/// 插件收尾钩子的整体写入:一个 IMMEDIATE 事务内确认时间线已建且不在重建中
/// → 按 tid 删掉本运行与被它替换的运行(`replaced`)的旧投影 → 写入新投影 →
/// 事件源水位推进到 `max(现值, completed_at)`。
///
/// 与重建的 `meta_begin` 由写锁排序:重建先开始则本钩子跳过,本钩子先提交
/// 则重建删掉并重投影这些行,不会重复。多条任务的钩子乱序提交时水位只前进;
/// 某个钩子失败时水位停在它之前,状态判为待重建。返回新投影行数,跳过时
/// 返回 `None`。
pub fn project_run_rows(
    conn: &Connection,
    tid: &str,
    replaced: &[String],
    rows: &[TimelineInsertRow],
    completed_at: &str,
) -> anyhow::Result<Option<u64>> {
    let tx = evidence_db::write_transaction(conn)?;
    let Some(meta) = meta_get(&tx)? else {
        return Ok(None);
    };
    if meta.status == "running" {
        return Ok(None);
    }
    delete_run(&tx, tid)?;
    for old in replaced {
        delete_run(&tx, old)?;
    }
    write_rows(&tx, rows)?;
    tx.execute(
        "UPDATE timeline_build SET source_event_watermark = MAX(source_event_watermark, ?1)
         WHERE singleton = 1",
        [completed_at],
    )?;
    tx.commit()?;
    Ok(Some(rows.len() as u64))
}

/// 删除若干 run(tid)的投影(任务删除级联),单个事务。时间线未建过时不动,
/// 交给重建。返回删除行数。
pub fn delete_runs(conn: &Connection, tids: &[String]) -> anyhow::Result<u64> {
    let tx = evidence_db::write_transaction(conn)?;
    if meta_get(&tx)?.is_none() {
        return Ok(0);
    }
    let mut removed = 0;
    for tid in tids {
        removed += delete_run(&tx, tid)?;
    }
    tx.commit()?;
    Ok(removed)
}

// ── 构建元数据 ──────────────────────────────────────────────────────────

pub fn meta_get(conn: &Connection) -> anyhow::Result<Option<TimelineMetaRow>> {
    conn.query_row(
        "SELECT status, error, started_ms, finished_ms, file_rows, event_rows,
                source_file_watermark, source_event_watermark
         FROM timeline_build WHERE singleton = 1",
        [],
        |row| {
            Ok(TimelineMetaRow {
                status: row.get(0)?,
                error: row.get(1)?,
                started_ms: row.get(2)?,
                finished_ms: row.get(3)?,
                file_rows: row.get::<_, i64>(4)? as u64,
                event_rows: row.get::<_, i64>(5)? as u64,
                source_file_watermark: row.get(6)?,
                source_event_watermark: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// 重建开始:状态先行落库(`running`),启动即可见。
pub fn meta_begin(conn: &Connection, started_ms: i64) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO timeline_build (singleton, status, started_ms)
         VALUES (1, 'running', ?1)
         ON CONFLICT (singleton) DO UPDATE SET
            status = 'running', error = NULL, started_ms = excluded.started_ms,
            finished_ms = NULL, file_rows = 0, event_rows = 0",
        rusqlite::params![started_ms],
    )
    .with_context(|| "begin timeline build")?;
    Ok(())
}

/// 重建收尾:终态 + 计数 + 源水位(staleness 判定基准)。
pub fn meta_finish(
    conn: &Connection,
    meta: &TimelineMetaRow,
    finished_ms: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE timeline_build SET
            status = ?1, error = ?2, finished_ms = ?3,
            file_rows = ?4, event_rows = ?5,
            source_file_watermark = ?6, source_event_watermark = ?7
         WHERE singleton = 1",
        rusqlite::params![
            meta.status,
            meta.error,
            finished_ms,
            meta.file_rows as i64,
            meta.event_rows as i64,
            meta.source_file_watermark,
            meta.source_event_watermark,
        ],
    )
    .with_context(|| "finish timeline build")?;
    Ok(())
}
