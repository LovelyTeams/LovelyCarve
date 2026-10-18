//! 全局搜索的持久化存储。
//!
//! 每次搜索 = 一行任务记录（`global_search_history`，含运行状态），命中逐条落
//! `global_search_hits`（`hit_seq` 递增序号，支持增量刷新与服务端去重）：
//!
//! - **任务即历史**：任务行在搜索启动时以 `running` 落库，结束时改写终态；
//!   删除任务 = 删除整次搜索（含命中）。
//! - **服务端分页**：命中经 `LIMIT/OFFSET` 分页读取，`total` 用同条件
//!   `COUNT` 另查；关键词过滤（命中内容/文件名/路径/模式，小写列 LIKE）
//!   与去重（按文件 `node_id` / 按 `sha256`，窗口函数取每组首条）都在
//!   SQL 侧完成，前端不整表拉取。
//! - **增量**：`after_seq` 只取序号大于该值的新命中——扫描进行中前端轮询
//!   追加。
//!
//! 两张表住在检材目录的 `search.db`:每个检材一个库,只有运行中的搜索在写
//! (启动、命中分批、收尾)。不同检材的搜索互不排队;检材移除即删除整个
//! 库文件。搜索对文件系统索引只读——索引是发布后不再修改的独立文件,导入
//! 期间搜索照常启动并完整落库。
//!
//! 读路径(任务列表、命中分页、导出)不建库、不跑建表 DDL、不做无条件写:
//! 库还不存在就按空结果返回,孤儿 `running` 行的修复只短等写锁。

mod lifecycle;
pub use lifecycle::{
    append_hits, create_running, delete, finish, history_get, history_list, set_source_hashes,
};
mod query;
pub use query::{export_rows, page};

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::core::casework::case_paths::EvidenceDb;
use crate::core::casework::evidence_db::{self, StoreSchema};

/// 每镜像保留的搜索任务数上限，超出时删除旧任务。
pub const HISTORY_MAX_PER_IMAGE: usize = 20;

/// 孤儿 `running` 行改判 `failed` 时记录的原因。
const ORPHANED_ERROR: &str = "应用重启导致搜索中断";

/// `search.db` 结构。
const SCHEMA: StoreSchema = StoreSchema {
    db: EvidenceDb::Search,
    // ASCII "LCSR".
    application_id: 0x4C43_5352,
    version: 1,
    ddl: r#"
        CREATE TABLE global_search_history (
            search_id TEXT PRIMARY KEY,
            keywords_json TEXT NOT NULL DEFAULT '[]',
            regexes_json TEXT NOT NULL DEFAULT '[]',
            codepages_json TEXT NOT NULL DEFAULT '[]',
            match_case INTEGER NOT NULL DEFAULT 0,
            search_deleted INTEGER NOT NULL DEFAULT 1,
            search_empty_space INTEGER NOT NULL DEFAULT 0,
            calc_hash INTEGER NOT NULL DEFAULT 0,
            scope_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'running',
            error TEXT,
            started_ms INTEGER NOT NULL,
            finished_ms INTEGER,
            files_total INTEGER NOT NULL DEFAULT 0,
            files_scanned INTEGER NOT NULL DEFAULT 0,
            matched_file_count INTEGER NOT NULL DEFAULT 0,
            total_matches INTEGER NOT NULL DEFAULT 0,
            record_count INTEGER NOT NULL DEFAULT 0,
            read_errors INTEGER NOT NULL DEFAULT 0,
            truncated INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX global_search_history_by_start ON global_search_history (started_ms DESC);

        CREATE TABLE global_search_hits (
            search_id TEXT NOT NULL,
            hit_seq INTEGER NOT NULL,
            node_id TEXT NOT NULL DEFAULT '',
            volume_key TEXT NOT NULL DEFAULT '',
            file_path TEXT NOT NULL DEFAULT '',
            file_path_low TEXT NOT NULL DEFAULT '',
            file_name TEXT NOT NULL DEFAULT '',
            file_name_low TEXT NOT NULL DEFAULT '',
            file_size INTEGER NOT NULL DEFAULT 0,
            file_offset INTEGER NOT NULL DEFAULT 0,
            match_len INTEGER NOT NULL DEFAULT 0,
            encoding TEXT NOT NULL DEFAULT '',
            pattern TEXT NOT NULL DEFAULT '',
            pattern_low TEXT NOT NULL DEFAULT '',
            pattern_kind TEXT NOT NULL DEFAULT 'keyword',
            match_count INTEGER NOT NULL DEFAULT 0,
            context_preview TEXT NOT NULL DEFAULT '',
            context_full TEXT NOT NULL DEFAULT '',
            context_low TEXT NOT NULL DEFAULT '',
            md5 TEXT NOT NULL DEFAULT '',
            sha256 TEXT NOT NULL DEFAULT '',
            source_kind TEXT NOT NULL DEFAULT 'file',
            PRIMARY KEY (search_id, hit_seq)
        );
        CREATE INDEX global_search_hits_by_node ON global_search_hits (search_id, node_id);
        CREATE INDEX global_search_hits_by_sha ON global_search_hits (search_id, sha256);
    "#,
    // 只有搜索自己的短事务在写,正常情况下等不满。
    busy_timeout: Duration::from_secs(60),
};

/// 孤儿修复等写锁的上限。修复只是顺手落库：拿不到就只在本次列表结果里
/// 按中断呈现，下次列表再修，列表轮询不排在长写事务后面。
const ORPHAN_FIX_BUSY_WAIT: Duration = Duration::from_millis(250);

// ── Wire 类型（与前端 `src/lib/ipc/search.ts` 镜像，snake_case 原样透传）──

/// 一条命中记录。`hit_seq` 是该搜索内的递增序号（增量轮询游标）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GlobalSearchHit {
    pub hit_seq: u64,
    pub node_id: String,
    pub volume_key: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: u64,
    /// 命中在源（文件 / 未分配节点）内的字节偏移。
    pub file_offset: u64,
    pub match_len: u64,
    pub encoding: String,
    pub pattern: String,
    /// `keyword` | `regex`。
    pub pattern_kind: String,
    /// 该源内命中总数（跨 per-file 截断仍准确）。
    pub match_count: u64,
    pub context_preview: String,
    pub context_full: String,
    pub md5: String,
    pub sha256: String,
    /// `file` | `unallocated` | `diskgap`。
    pub source_kind: String,
}

/// 一次搜索任务的元数据行（历史列表 / 任务树节点）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalSearchMeta {
    pub search_id: String,
    pub image_id: String,
    pub keywords: Vec<String>,
    pub regexes: Vec<String>,
    pub codepages: Vec<String>,
    pub match_case: bool,
    pub search_deleted: bool,
    pub search_empty_space: bool,
    pub calc_hash: bool,
    /// 原始范围 JSON（分区/标签/树节点），前端回显用。
    pub scope_json: String,
    /// `running` | `success` | `cancelled` | `failed`。
    pub status: String,
    pub error: Option<String>,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub files_total: u64,
    pub files_scanned: u64,
    pub matched_file_count: u64,
    pub total_matches: u64,
    pub record_count: u64,
    pub read_errors: u64,
    pub truncated: bool,
}

/// 命中分页的去重维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlobalSearchDistinct {
    #[default]
    None,
    /// 每个文件（`node_id`）只保留首条命中。
    File,
    /// 每个哈希只保留首条（未算哈希的行按文件去重兜底）。
    Sha256,
}

impl GlobalSearchDistinct {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::File => "file",
            Self::Sha256 => "sha256",
        }
    }

    pub fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value.unwrap_or("none") {
            "none" | "" => Ok(Self::None),
            "file" => Ok(Self::File),
            "sha256" => Ok(Self::Sha256),
            other => anyhow::bail!("未知去重模式：{other}"),
        }
    }
}

/// 命中分页查询。`after_seq` 与 `skip` 可同时给（增量轮询场景 `skip=0`）。
#[derive(Debug, Clone)]
pub struct GlobalSearchPageQuery {
    pub search_id: String,
    pub skip: u64,
    pub limit: u64,
    pub keyword: Option<String>,
    pub distinct: GlobalSearchDistinct,
    pub after_seq: Option<u64>,
}

/// 分页结果：`total` 是同条件（含去重）下的总数，供前端显示与判定加载完。
#[derive(Debug, Clone, Serialize)]
pub struct GlobalSearchPage {
    pub search_id: String,
    pub items: Vec<GlobalSearchHit>,
    pub total: u64,
}

const HIT_COLUMNS: &str = "hit_seq, node_id, volume_key, file_path, file_name, file_size,
       file_offset, match_len, encoding, pattern, pattern_kind, match_count,
       context_preview, context_full, md5, sha256, source_kind";

// ── 连接与建表 ─────────────────────────────────────────────────────────

/// 写路径连接:打开(必要时创建)检材的 `search.db`。
fn open_store(case_dir: &Path, image_id: &str) -> anyhow::Result<Connection> {
    evidence_db::open_writer(case_dir, image_id, &SCHEMA)
}

/// 已有库的连接:不建库、不跑 DDL;检材从未搜索过时返回 `None`。
fn open_existing(
    case_dir: &Path,
    image_id: &str,
    busy_timeout: Duration,
) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_existing(case_dir, image_id, &SCHEMA, busy_timeout)
}

/// 读路径连接(任务列表、命中分页、导出)。
fn open_reader(case_dir: &Path, image_id: &str) -> anyhow::Result<Option<Connection>> {
    evidence_db::open_reader(case_dir, image_id, &SCHEMA)
}

const META_COLUMNS: &str = "search_id, keywords_json, regexes_json, codepages_json,
    match_case, search_deleted, search_empty_space, calc_hash, scope_json,
    status, error, started_ms, finished_ms,
    files_total, files_scanned, matched_file_count, total_matches,
    record_count, read_errors, truncated";

// ── 命中写入与查询 ─────────────────────────────────────────────────────

/// 一次搜索的命中写入器：扫描全程复用同一条连接。drain 线程每批落库时不再
/// 重开连接、重跑建表语句——命中密集的搜索要落几百批。
pub struct HitWriter {
    conn: Connection,
    search_id: String,
}

impl HitWriter {
    pub fn open(case_dir: &Path, image_id: &str, search_id: &str) -> anyhow::Result<Self> {
        Ok(Self {
            conn: open_store(case_dir, image_id)?,
            search_id: search_id.to_string(),
        })
    }

    /// 批量落命中(`hit_seq` 已由调用方编排)。单事务,`INSERT OR REPLACE`
    /// 使重跑同一 search_id 的残留行被覆盖。失败时整批回滚,调用方留着这批
    /// 命中随下一批重试即可(同序号覆盖,幂等)。
    pub fn append(&mut self, hits: &[GlobalSearchHit]) -> anyhow::Result<()> {
        if hits.is_empty() {
            return Ok(());
        }
        let tx = evidence_db::write_transaction(&self.conn)?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO global_search_hits (
                    search_id, hit_seq, node_id, volume_key,
                    file_path, file_path_low, file_name, file_name_low,
                    file_size, file_offset, match_len, encoding,
                    pattern, pattern_low, pattern_kind, match_count,
                    context_preview, context_full, context_low,
                    md5, sha256, source_kind
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            )?;
            for hit in hits {
                stmt.execute(rusqlite::params![
                    self.search_id,
                    hit.hit_seq as i64,
                    hit.node_id,
                    hit.volume_key,
                    hit.file_path,
                    hit.file_path.to_lowercase(),
                    hit.file_name,
                    hit.file_name.to_lowercase(),
                    to_sqlite(hit.file_size),
                    to_sqlite(hit.file_offset),
                    to_sqlite(hit.match_len),
                    hit.encoding,
                    hit.pattern,
                    hit.pattern.to_lowercase(),
                    hit.pattern_kind,
                    to_sqlite(hit.match_count),
                    hit.context_preview,
                    hit.context_full,
                    hit.context_preview.to_lowercase(),
                    hit.md5,
                    hit.sha256,
                    hit.source_kind,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 回填若干源全部命中的 md5/sha256（CalcHash 收尾阶段），单事务。每项为
    /// `(node_id, md5, sha256)`；失败时整批回滚，按源 UPDATE 重试幂等。
    pub fn set_source_hashes(&mut self, hashes: &[(String, String, String)]) -> anyhow::Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        let tx = evidence_db::write_transaction(&self.conn)?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE global_search_hits SET md5 = ?3, sha256 = ?4
                 WHERE search_id = ?1 AND node_id = ?2",
            )?;
            for (node_id, md5, sha256) in hashes {
                stmt.execute(rusqlite::params![self.search_id, node_id, md5, sha256])
                    .with_context(|| "update hit hashes")?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
impl HitWriter {
    /// 测试用：让本连接的写入立即失败（`query_only`），模拟落库故障。
    pub(crate) fn set_read_only(&self, read_only: bool) {
        self.conn
            .pragma_update(None, "query_only", read_only)
            .expect("toggle query_only");
    }
}

// ── 行映射 ─────────────────────────────────────────────────────────────

fn meta_from_row(row: &rusqlite::Row<'_>, image_id: &str) -> rusqlite::Result<GlobalSearchMeta> {
    Ok(GlobalSearchMeta {
        search_id: row.get(0)?,
        image_id: image_id.to_string(),
        keywords: decode_json_column(row.get::<_, String>(1)?, 1, "keywords")?,
        regexes: decode_json_column(row.get::<_, String>(2)?, 2, "regexes")?,
        codepages: decode_json_column(row.get::<_, String>(3)?, 3, "codepages")?,
        match_case: row.get(4)?,
        search_deleted: row.get(5)?,
        search_empty_space: row.get(6)?,
        calc_hash: row.get(7)?,
        scope_json: row.get(8)?,
        status: row.get(9)?,
        error: row.get(10)?,
        started_ms: row.get(11)?,
        finished_ms: row.get(12)?,
        files_total: from_sqlite(row.get(13)?),
        files_scanned: from_sqlite(row.get(14)?),
        matched_file_count: from_sqlite(row.get(15)?),
        total_matches: from_sqlite(row.get(16)?),
        record_count: from_sqlite(row.get(17)?),
        read_errors: from_sqlite(row.get(18)?),
        truncated: row.get(19)?,
    })
}

fn hit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GlobalSearchHit> {
    Ok(GlobalSearchHit {
        hit_seq: from_sqlite(row.get(0)?),
        node_id: row.get(1)?,
        volume_key: row.get(2)?,
        file_path: row.get(3)?,
        file_name: row.get(4)?,
        file_size: from_sqlite(row.get(5)?),
        file_offset: from_sqlite(row.get(6)?),
        match_len: from_sqlite(row.get(7)?),
        encoding: row.get(8)?,
        pattern: row.get(9)?,
        pattern_kind: row.get(10)?,
        match_count: from_sqlite(row.get(11)?),
        context_preview: row.get(12)?,
        context_full: row.get(13)?,
        md5: row.get(14)?,
        sha256: row.get(15)?,
        source_kind: row.get(16)?,
    })
}

fn decode_json_column<T: serde::de::DeserializeOwned>(
    value: String,
    index: usize,
    label: &str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("decode global search {label}: {error}"),
            )),
        )
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn to_sqlite(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn from_sqlite(value: i64) -> u64 {
    value.max(0) as u64
}

/// 搜索存储测试的公共样本（本模块与引擎的测试共用）。
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn sample_meta(search_id: &str, status: &str) -> GlobalSearchMeta {
        GlobalSearchMeta {
            search_id: search_id.to_string(),
            image_id: "image-1".to_string(),
            keywords: vec!["password".to_string()],
            regexes: vec![r"\d{4}".to_string()],
            codepages: vec!["UTF-8".to_string(), "GBK".to_string()],
            match_case: false,
            search_deleted: true,
            search_empty_space: false,
            calc_hash: false,
            scope_json: "{}".to_string(),
            status: status.to_string(),
            error: None,
            started_ms: 1000,
            finished_ms: None,
            files_total: 10,
            files_scanned: 0,
            matched_file_count: 0,
            total_matches: 0,
            record_count: 0,
            read_errors: 0,
            truncated: false,
        }
    }

    pub(crate) fn sample_hit(seq: u64, node: &str, context: &str) -> GlobalSearchHit {
        GlobalSearchHit {
            hit_seq: seq,
            node_id: node.to_string(),
            volume_key: "part:0".to_string(),
            file_path: format!("part:0:/etc/{node}"),
            file_name: node.to_string(),
            file_size: 128,
            file_offset: seq * 10,
            match_len: 8,
            encoding: "ASCII".to_string(),
            pattern: "password".to_string(),
            pattern_kind: "keyword".to_string(),
            match_count: 3,
            context_preview: context.to_string(),
            context_full: context.to_string(),
            md5: String::new(),
            sha256: String::new(),
            source_kind: "file".to_string(),
        }
    }

    pub(crate) fn page_query(search_id: &str) -> GlobalSearchPageQuery {
        GlobalSearchPageQuery {
            search_id: search_id.to_string(),
            skip: 0,
            limit: 100,
            keyword: None,
            distinct: GlobalSearchDistinct::None,
            after_seq: None,
        }
    }
}

#[cfg(test)]
mod tests;
