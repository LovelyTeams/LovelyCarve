//! lifecycle implementation.
use super::*;

// ── 任务元数据 CRUD ────────────────────────────────────────────────────

/// 搜索启动即落任务行(`running`)——任务即历史。
pub fn create_running(case_dir: &Path, meta: &GlobalSearchMeta) -> anyhow::Result<()> {
    let conn = open_store(case_dir, &meta.image_id)?;
    conn.execute(
        "INSERT OR REPLACE INTO global_search_history (
            search_id, keywords_json, regexes_json, codepages_json,
            match_case, search_deleted, search_empty_space, calc_hash, scope_json,
            status, error, started_ms, finished_ms,
            files_total, files_scanned, matched_file_count, total_matches,
            record_count, read_errors, truncated
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        rusqlite::params![
            meta.search_id,
            serde_json::to_string(&meta.keywords)?,
            serde_json::to_string(&meta.regexes)?,
            serde_json::to_string(&meta.codepages)?,
            meta.match_case,
            meta.search_deleted,
            meta.search_empty_space,
            meta.calc_hash,
            meta.scope_json,
            meta.status,
            meta.error,
            meta.started_ms,
            meta.finished_ms,
            to_sqlite(meta.files_total),
            to_sqlite(meta.files_scanned),
            to_sqlite(meta.matched_file_count),
            to_sqlite(meta.total_matches),
            to_sqlite(meta.record_count),
            to_sqlite(meta.read_errors),
            meta.truncated,
        ],
    )
    .with_context(|| "insert global search history row")?;
    Ok(())
}

/// 搜索收尾：改写终态与全部计数；`record_count` 以命中表实数为准，并滚动
/// 删除超出 [`HISTORY_MAX_PER_IMAGE`] 的旧搜索（含命中行）。
///
/// 任务行已经不在(搜索在跑时被删除——删除只等扫描停下 10 s——或被滚动删除)
/// 时,之后落库的命中一并删掉,不能留下没有任务行的命中。
pub fn finish(case_dir: &Path, meta: &GlobalSearchMeta) -> anyhow::Result<()> {
    let conn = open_store(case_dir, &meta.image_id)?;
    let tx = evidence_db::write_transaction(&conn)?;
    let record_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM global_search_hits WHERE search_id = ?1",
        rusqlite::params![meta.search_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE global_search_history SET
            status = ?2, error = ?3, finished_ms = ?4, files_total = ?5,
            files_scanned = ?6, matched_file_count = ?7, total_matches = ?8,
            record_count = ?9, read_errors = ?10, truncated = ?11
         WHERE search_id = ?1",
        rusqlite::params![
            meta.search_id,
            meta.status,
            meta.error,
            meta.finished_ms,
            to_sqlite(meta.files_total),
            to_sqlite(meta.files_scanned),
            to_sqlite(meta.matched_file_count),
            to_sqlite(meta.total_matches),
            record_count,
            to_sqlite(meta.read_errors),
            meta.truncated,
        ],
    )?;
    tx.execute(
        "DELETE FROM global_search_hits
         WHERE search_id = ?1
           AND NOT EXISTS (SELECT 1 FROM global_search_history WHERE search_id = ?1)",
        rusqlite::params![meta.search_id],
    )?;
    // 滚动删除:每个检材只留最新 N 条(含运行中),命中行随任务级联删除。
    let stale: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT search_id FROM global_search_history
             ORDER BY started_ms DESC, search_id DESC
             LIMIT -1 OFFSET ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![HISTORY_MAX_PER_IMAGE as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for search_id in &stale {
        tx.execute(
            "DELETE FROM global_search_hits WHERE search_id = ?1",
            rusqlite::params![search_id],
        )?;
        tx.execute(
            "DELETE FROM global_search_history WHERE search_id = ?1",
            rusqlite::params![search_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 任务列表（新→旧，运行中的排最前）。
///
/// `live_search_ids` 返回本进程在跑的搜索，读行之后才调用：启动先登记在跑
/// 再落 `running` 行、收尾先落终态再移除登记，所以读到的 `running` 行不在
/// 存活集里，就是应用重启或收尾落库失败遗留的孤儿，改判 `failed`。
/// 修复之后重读：挑出
/// 孤儿之后才正常收尾的搜索显示真实终态，没能落库的孤儿就地按中断呈现。
pub fn history_list(
    case_dir: &Path,
    image_id: &str,
    limit: u32,
    live_search_ids: impl FnOnce() -> HashSet<String>,
) -> anyhow::Result<Vec<GlobalSearchMeta>> {
    let rows = read_history(case_dir, image_id, limit)?;
    let live = live_search_ids();
    let orphaned: Vec<String> = rows
        .iter()
        .filter(|meta| meta.status == "running" && !live.contains(&meta.search_id))
        .map(|meta| meta.search_id.clone())
        .collect();
    if orphaned.is_empty() {
        return Ok(rows);
    }
    let finished_ms = now_ms();
    match fail_orphaned_running(case_dir, image_id, &orphaned, finished_ms) {
        Ok(()) => {}
        Err(error)
            if error
                .downcast_ref::<rusqlite::Error>()
                .and_then(rusqlite::Error::sqlite_error_code)
                == Some(rusqlite::ErrorCode::DatabaseBusy) =>
        {
            tracing::debug!("global search orphan fixup deferred, store busy image={image_id}");
        }
        Err(error) => {
            tracing::warn!("global search orphan fixup failed image={image_id}: {error:#}");
        }
    }
    let mut rows = read_history(case_dir, image_id, limit)?;
    for meta in &mut rows {
        if meta.status == "running" && orphaned.contains(&meta.search_id) {
            meta.status = "failed".to_string();
            meta.error = Some(ORPHANED_ERROR.to_string());
            meta.finished_ms = Some(finished_ms);
        }
    }
    Ok(rows)
}

pub(super) fn read_history(
    case_dir: &Path,
    image_id: &str,
    limit: u32,
) -> anyhow::Result<Vec<GlobalSearchMeta>> {
    let Some(conn) = open_reader(case_dir, image_id)? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {META_COLUMNS} FROM global_search_history
         ORDER BY (status = 'running') DESC, started_ms DESC
         LIMIT ?1"
    ))?;
    let rows = stmt.query_map(rusqlite::params![i64::from(limit)], |row| {
        meta_from_row(row, image_id)
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// 取单个任务元数据。
pub fn history_get(
    case_dir: &Path,
    image_id: &str,
    search_id: &str,
) -> anyhow::Result<Option<GlobalSearchMeta>> {
    let Some(conn) = open_reader(case_dir, image_id)? else {
        return Ok(None);
    };
    conn.query_row(
        &format!("SELECT {META_COLUMNS} FROM global_search_history WHERE search_id = ?1"),
        rusqlite::params![search_id],
        |row| meta_from_row(row, image_id),
    )
    .optional()
    .map_err(Into::into)
}

/// 删除整次搜索(任务 + 命中)。
pub fn delete(case_dir: &Path, image_id: &str, search_id: &str) -> anyhow::Result<bool> {
    let Some(conn) = open_existing(case_dir, image_id, SCHEMA.busy_timeout)? else {
        return Ok(false);
    };
    let tx = evidence_db::write_transaction(&conn)?;
    tx.execute(
        "DELETE FROM global_search_hits WHERE search_id = ?1",
        rusqlite::params![search_id],
    )?;
    let changed = tx.execute(
        "DELETE FROM global_search_history WHERE search_id = ?1",
        rusqlite::params![search_id],
    )?;
    tx.commit()?;
    Ok(changed > 0)
}

/// 把 [`history_list`] 挑出的孤儿 `running` 行改判 `failed`。仍以
/// `status = 'running'` 为条件,挑出之后才收尾的搜索保留真实终态。
pub(super) fn fail_orphaned_running(
    case_dir: &Path,
    image_id: &str,
    orphaned: &[String],
    finished_ms: i64,
) -> anyhow::Result<()> {
    let Some(conn) = open_existing(case_dir, image_id, ORPHAN_FIX_BUSY_WAIT)? else {
        return Ok(());
    };
    let placeholders = vec!["?"; orphaned.len()].join(",");
    let sql = format!(
        "UPDATE global_search_history SET status = 'failed', error = ?1, finished_ms = ?2
         WHERE status = 'running' AND search_id IN ({placeholders})"
    );
    let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&ORPHANED_ERROR, &finished_ms];
    binds.extend(orphaned.iter().map(|id| id as &dyn rusqlite::ToSql));
    conn.execute(&sql, binds.as_slice())?;
    Ok(())
}

/// 批量落命中(单批便捷入口,扫描路径用 [`HitWriter`])。
pub fn append_hits(
    case_dir: &Path,
    image_id: &str,
    search_id: &str,
    hits: &[GlobalSearchHit],
) -> anyhow::Result<()> {
    if hits.is_empty() {
        return Ok(());
    }
    HitWriter::open(case_dir, image_id, search_id)?.append(hits)
}

/// 回填某源全部命中的 md5/sha256(单源便捷入口,扫描路径用 [`HitWriter`])。
pub fn set_source_hashes(
    case_dir: &Path,
    image_id: &str,
    search_id: &str,
    node_id: &str,
    md5: &str,
    sha256: &str,
) -> anyhow::Result<()> {
    HitWriter::open(case_dir, image_id, search_id)?.set_source_hashes(&[(
        node_id.to_string(),
        md5.to_string(),
        sha256.to_string(),
    )])
}
