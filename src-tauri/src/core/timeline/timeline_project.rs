//! 时间线投影引擎。
//!
//! 两条投影路径:
//!
//! - **创建重建任务**(不是开关):遍历存量源数据(各卷已发布的文件系统索引 +
//!   检材 `records.db` 里已发布运行的时间点 + 旧案件 `analysis.db` 的插件
//!   时间线事件)整体重投影——先清空旧投影再分批插入,幂等([`run_rebuild`]);
//! - **写路径自动投影**:插件运行发布时按 tid(run_id)删掉它与被它替换的
//!   运行的投影,再投影它的时间点([`project_published_run`],调度器发布时
//!   调用)——插件只需在 schema 里给时间戳字段标时间线角色。
//!
//! 状态机:`build_required`(未创建)/ `running`(正在跑)/ `stale`(有新数据
//! 未处理)/ `ready`(就绪)。staleness 以源水位判定:文件源是检材已索引卷的
//! 「卷键@来源快照时间」签名,事件源是插件运行发布时间(旧版运行为完成时间)
//! 的最大值。水位在重建**开始前**取:重建期间新到的数据会让结果判为过期,
//! 而不是被悄悄漏掉。
//!
//! 投影规则:
//!
//! | datatype | time 字段 | tl_flag | from_app | content |
//! |---|---|---|---|---|
//! | file | **Mtime**(Ctime/Atime 不进时间线) | 修改 | volume_key | full_path |
//! | plugin_event | 带时间线角色的时间戳字段 | 角色(`login`…) | plugin_id | 记录标题 |
//! | plugin_event(旧版) | timestamp | event_type | plugin_id | description |
//! | ntfs_usn | journal FILETIME | reason flags | NTFS USN + volume | historical name / MFT references |
//!
//! 缺失/无法解析时间戳的源记录跳过(保持 `min/max(tl_time)` 与桶聚合不被
//! 零值污染);未带时区的本地时间不知道对应哪个 UTC 时刻,同样不进时间线,
//! 只在记录表格里按墙上时间排序展示。

use crate::core::filesystem::evidence_session::EvidenceSession;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::casework::case_ledger_volumes::{indexed_volumes, IndexedVolume};
use crate::core::timeline::timeline_store::{
    self, TimelineInsertRow, TimelineMetaRow, DATA_TYPE_FILE, DATA_TYPE_PLUGIN_EVENT,
};
use crate::core::{
    analysis::results::{
        legacy_store,
        record_query::{self, TimePointSource},
    },
    catalog::filedb::filedb_file,
};

/// 重建批次大小:流式落库大检材,批间检查取消/回报进度。
const INSERT_BATCH: usize = 2000;

/// 投影规则版本,写进文件源水位;规则变化后已建的时间线判为过期。
const PROJECTION_VERSION: &str = "v2";

/// 时间线重建结果摘要。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TimelineBuildSummary {
    /// `success` | `cancelled`。
    pub status: String,
    pub files_total: u64,
    pub file_rows: u64,
    pub events_total: u64,
    pub event_rows: u64,
    /// 因时间戳缺失/无法解析被跳过的源记录数。
    pub skipped_invalid_time: u64,
}

/// 时间线投影的构建状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TimelineStatus {
    /// 未创建。
    BuildRequired,
    /// 正在跑。
    Running,
    /// 有新数据未处理。
    Stale,
    /// 就绪。
    Ready,
}

/// 时间线状态响应:状态、时间范围与计数。
#[derive(Debug, Clone, Serialize)]
pub struct TimelineStatusInfo {
    pub status: TimelineStatus,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub record_count: u64,
    pub meta: Option<TimelineMetaRow>,
}

/// 进度回调:`(done, total, rows)`——已投影源记录数 / 源总数 / 已落行数。
pub type ProgressSink<'a> = &'a dyn Fn(u64, u64, u64);

/// RFC3339 字符串 → unix 秒(无法解析返回 `None`)。
fn rfc3339_to_epoch(value: Option<&str>) -> Option<i64> {
    let value = value?;
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.timestamp())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

// ── 源读取 ─────────────────────────────────────────────────────────────

/// 文件源水位:投影规则版本 + 每个已索引卷的「卷键@来源快照时间」。卷的
/// 增删、重建都会改变它。
fn file_watermark(volumes: &[IndexedVolume]) -> String {
    let parts: Vec<String> = volumes
        .iter()
        .map(|volume| format!("{}@{}", volume.volume_key, volume.built_at))
        .collect();
    format!("{PROJECTION_VERSION}:{}", parts.join(","))
}

fn file_rows_total(volumes: &[IndexedVolume]) -> anyhow::Result<u64> {
    let mut total = 0u64;
    for volume in volumes {
        let conn = filedb_file::open_reader(&volume.index)?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM filedb", [], |row| row.get(0))?;
        total += u64::try_from(count).context("filedb row count is negative")?;
    }
    Ok(total)
}

/// 流式遍历各卷索引的文件行。`cancel` 置位即停止迭代(返回 true)。
fn visit_file_rows(
    volumes: &[IndexedVolume],
    cancel: &CancellationToken,
    mut visit: impl FnMut(&str, &str, &str, Option<&str>, bool) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    for volume in volumes {
        let conn = filedb_file::open_reader(&volume.index)?;
        let mut statement =
            conn.prepare("SELECT nid, full_path, mtime, is_delete FROM filedb ORDER BY ordinal")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if cancel.is_cancelled() {
                return Ok(true);
            }
            let nid = row.get::<_, String>(0)?;
            let full_path = row.get::<_, String>(1)?;
            let mtime = row.get::<_, Option<String>>(2)?;
            // filedb is_delete:2=在世,1=已删除 → timeline delete 镜像。
            let deleted = row.get::<_, i64>(3)? == 1;
            visit(
                &volume.volume_key,
                &nid,
                &full_path,
                mtime.as_deref(),
                deleted,
            )?;
        }
    }
    Ok(false)
}

fn event_insert_row(event: &legacy_store::StoredTimelineEventSource) -> Option<TimelineInsertRow> {
    let tl_time = rfc3339_to_epoch(Some(event.timestamp.as_str()))?;
    Some(TimelineInsertRow {
        oid: event.event_id.clone(),
        tid: event.run_id.clone(),
        deleted: false,
        tl_time,
        from_app: event.plugin_id.clone(),
        data_type: DATA_TYPE_PLUGIN_EVENT,
        tl_flag: event.event_type.clone(),
        content: event.description.clone(),
        extra: String::new(),
    })
}

/// One timeline point of a plugin record. `extra` names the record, so the
/// timeline can open it.
fn point_insert_row(point: &TimePointSource) -> TimelineInsertRow {
    TimelineInsertRow {
        oid: format!("{}:{}", point.record_id, point.field),
        tid: point.run_id.clone(),
        deleted: false,
        tl_time: point.epoch_us.div_euclid(1_000_000),
        from_app: point.plugin_id.clone(),
        data_type: DATA_TYPE_PLUGIN_EVENT,
        tl_flag: point.role.clone(),
        content: point.title.clone(),
        extra: serde_json::json!({
            "recordId": point.record_id.to_string(),
            "recordType": point.record_type,
            "field": point.field,
        })
        .to_string(),
    }
}

/// The plugin source watermark: the latest publication of either store.
fn event_watermark(case_dir: &Path, image_id: &str) -> anyhow::Result<String> {
    let records = record_query::max_published_at(case_dir, image_id)?;
    let legacy = legacy_store::max_completed_at(case_dir, image_id)?;
    Ok(records.max(legacy))
}

// ── 重建任务 ─────────────────────────────────────────

/// 全量重建:清空旧投影 → 流式重投影文件与插件事件 → 终态落元数据(含源
/// 水位)。取消时保留已落库的部分投影(`status=cancelled`,与全局搜索取消
/// 语义一致)。失败时元数据落 `failed` 并向上传播。
pub fn run_rebuild(
    case_dir: &Path,
    image_id: &str,
    cancel_flag: &CancellationToken,
    on_progress: ProgressSink<'_>,
) -> anyhow::Result<TimelineBuildSummary> {
    run_rebuild_sources(case_dir, image_id, None, cancel_flag, on_progress)
}

/// Rebuild using the loaded image as well as persisted file/plugin sources.
pub fn run_rebuild_with_image(
    case_dir: &Path,
    image: &Arc<EvidenceSession>,
    cancel_flag: &CancellationToken,
    on_progress: ProgressSink<'_>,
) -> anyhow::Result<TimelineBuildSummary> {
    run_rebuild_sources(case_dir, &image.id, Some(image), cancel_flag, on_progress)
}

fn run_rebuild_sources(
    case_dir: &Path,
    image_id: &str,
    image: Option<&Arc<EvidenceSession>>,
    cancel_flag: &CancellationToken,
    on_progress: ProgressSink<'_>,
) -> anyhow::Result<TimelineBuildSummary> {
    let started_ms = now_ms();
    let conn = timeline_store::open_writer(case_dir, image_id)?;
    timeline_store::meta_begin(&conn, started_ms).context("timeline rebuild: begin meta")?;
    // 先取水位再读源:重建期间到达的新数据让结果判为过期。
    let sources = indexed_volumes(case_dir, image_id).and_then(|volumes| {
        let event_watermark = event_watermark(case_dir, image_id)?;
        Ok((volumes, event_watermark))
    });
    let outcome = sources.and_then(|(volumes, event_watermark)| {
        let summary = rebuild_inner(
            case_dir,
            image_id,
            image,
            &volumes,
            &conn,
            cancel_flag,
            on_progress,
        )?;
        Ok((summary, file_watermark(&volumes), event_watermark))
    });
    match outcome {
        Ok((summary, file_watermark, event_watermark)) => {
            let meta = TimelineMetaRow {
                status: summary.status.clone(),
                error: None,
                started_ms,
                finished_ms: None,
                file_rows: summary.file_rows,
                event_rows: summary.event_rows,
                source_file_watermark: file_watermark,
                source_event_watermark: event_watermark,
            };
            timeline_store::meta_finish(&conn, &meta, now_ms())
                .context("timeline rebuild: finish meta")?;
            Ok(summary)
        }
        Err(error) => {
            let meta = TimelineMetaRow {
                status: "failed".to_string(),
                error: Some(format!("{error:#}")),
                started_ms,
                finished_ms: None,
                file_rows: 0,
                event_rows: 0,
                source_file_watermark: String::new(),
                source_event_watermark: String::new(),
            };
            if let Err(finish_error) = timeline_store::meta_finish(&conn, &meta, now_ms()) {
                tracing::warn!("timeline rebuild: mark failed meta: {finish_error:#}");
            }
            Err(error)
        }
    }
}

fn rebuild_inner(
    case_dir: &Path,
    image_id: &str,
    image: Option<&Arc<EvidenceSession>>,
    volumes: &[IndexedVolume],
    conn: &rusqlite::Connection,
    cancel_flag: &CancellationToken,
    on_progress: ProgressSink<'_>,
) -> anyhow::Result<TimelineBuildSummary> {
    let files_total = file_rows_total(volumes)?;
    let events_total = record_query::count_time_points(case_dir, image_id)?
        + legacy_store::count_timeline_events(case_dir, image_id)?;

    let mut summary = TimelineBuildSummary {
        status: "success".to_string(),
        files_total,
        file_rows: 0,
        events_total,
        event_rows: 0,
        skipped_invalid_time: 0,
    };

    // 幂等基线:先清空旧投影。
    timeline_store::clear(conn)?;

    if let Some(image) = image {
        crate::core::timeline::timeline_usn::project_journals(
            image,
            conn,
            cancel_flag,
            &mut summary,
            on_progress,
        )?;
    }
    let total = files_total + summary.events_total;
    let mut done = summary.events_total - events_total;
    if cancel_flag.is_cancelled() {
        summary.status = "cancelled".into();
        return Ok(summary);
    }

    // 插件记录的时间点与旧版事件(量小,先投影)。
    {
        let mut batch: Vec<TimelineInsertRow> = Vec::with_capacity(INSERT_BATCH);
        record_query::visit_time_points(case_dir, image_id, |point| {
            done += 1;
            batch.push(point_insert_row(point));
            summary.event_rows += 1;
            if batch.len() >= INSERT_BATCH {
                timeline_store::insert_rows(conn, &batch)?;
                batch.clear();
            }
            Ok(())
        })?;
        legacy_store::visit_timeline_events(case_dir, image_id, |event| {
            done += 1;
            match event_insert_row(event) {
                Some(row) => {
                    batch.push(row);
                    summary.event_rows += 1;
                }
                None => summary.skipped_invalid_time += 1,
            }
            if batch.len() >= INSERT_BATCH {
                timeline_store::insert_rows(conn, &batch)?;
                batch.clear();
            }
            Ok(())
        })?;
        timeline_store::insert_rows(conn, &batch)?;
    }
    on_progress(done.min(total), total, summary.event_rows);

    // 文件记录(量最大,流式分批)。
    let cancelled = {
        let mut batch: Vec<TimelineInsertRow> = Vec::with_capacity(INSERT_BATCH);
        let stopped = visit_file_rows(
            volumes,
            cancel_flag,
            |volume_key, nid, full_path, mtime, deleted| {
                done += 1;
                match rfc3339_to_epoch(mtime) {
                    Some(tl_time) => {
                        batch.push(TimelineInsertRow {
                            oid: nid.to_string(),
                            tid: String::new(),
                            deleted,
                            tl_time,
                            from_app: volume_key.to_string(),
                            data_type: DATA_TYPE_FILE,
                            tl_flag: "修改".to_string(),
                            content: full_path.to_string(),
                            extra: String::new(),
                        });
                        summary.file_rows += 1;
                    }
                    None => summary.skipped_invalid_time += 1,
                }
                if batch.len() >= INSERT_BATCH {
                    timeline_store::insert_rows(conn, &batch)?;
                    on_progress(
                        done.min(total),
                        total,
                        summary.event_rows + summary.file_rows,
                    );
                    batch.clear();
                }
                Ok(())
            },
        )?;
        timeline_store::insert_rows(conn, &batch)?;
        stopped
    };
    on_progress(
        done.min(total),
        total,
        summary.event_rows + summary.file_rows,
    );

    if cancelled {
        summary.status = "cancelled".to_string();
    }
    Ok(summary)
}

// ── 写路径增量投影(插件运行发布钩子)──────────────────────────────────

/// 删掉 `run_id` 与它替换的运行(`superseded`)的投影,再投影 `run_id` 的
/// 时间点。仅在检材时间线已初始化且不在重建中时生效(否则交给全量重建
/// 覆盖)。检查与写入在同一事务里([`timeline_store::project_run_rows`]),
/// 同一检材的多条插件任务同时发布时依次排队提交。返回新投影行数。
pub fn project_published_run(
    case_dir: &Path,
    image_id: &str,
    run_id: &str,
    superseded: &[String],
) -> anyhow::Result<u64> {
    let Some((published_at, points)) = record_query::run_time_points(case_dir, image_id, run_id)?
    else {
        return Ok(0);
    };
    let Some(conn) =
        timeline_store::open_existing(case_dir, image_id, std::time::Duration::from_secs(30))?
    else {
        // 时间线从未建过:不产生半初始化状态,交给全量重建。
        return Ok(0);
    };
    let rows: Vec<TimelineInsertRow> = points.iter().map(point_insert_row).collect();
    let projected =
        timeline_store::project_run_rows(&conn, run_id, superseded, &rows, &published_at)?;
    Ok(projected.unwrap_or(0))
}

// ── 状态机 ──────────────────────────────────────────

/// 计算检材时间线状态。`running_in_process` 是命令层的在跑登记
/// (`state.timeline_builds`);元数据 `running` 但进程内不在跑 = 孤儿(应用
/// 重启中断),短等写锁改判 `failed`(拿不到锁时只在本次结果里按中断呈现)。
/// 时间线还不存在时不建库。
pub fn compute_status(
    case_dir: &Path,
    image_id: &str,
    running_in_process: bool,
) -> anyhow::Result<TimelineStatusInfo> {
    let Some(conn) = timeline_store::open_reader(case_dir, image_id)? else {
        return Ok(TimelineStatusInfo {
            status: TimelineStatus::BuildRequired,
            start_time: None,
            end_time: None,
            record_count: 0,
            meta: None,
        });
    };
    let mut meta = timeline_store::meta_get(&conn)?;
    let status = match &meta {
        None => TimelineStatus::BuildRequired,
        Some(meta_row) if meta_row.status == "running" && running_in_process => {
            TimelineStatus::Running
        }
        Some(meta_row) if meta_row.status == "running" => {
            // 孤儿 running:应用重启导致构建中断,改判 failed。
            let repaired = TimelineMetaRow {
                status: "failed".to_string(),
                error: Some("应用重启导致时间线构建中断".to_string()),
                finished_ms: Some(now_ms()),
                ..meta_row.clone()
            };
            let fixed = timeline_store::open_existing(
                case_dir,
                image_id,
                std::time::Duration::from_millis(250),
            )
            .and_then(|writer| match writer {
                Some(writer) => timeline_store::meta_finish(&writer, &repaired, now_ms()),
                None => Ok(()),
            });
            if let Err(error) = fixed {
                tracing::debug!("timeline orphan fixup deferred for {image_id}: {error:#}");
            }
            meta = Some(repaired);
            TimelineStatus::Stale
        }
        Some(meta_row) if meta_row.status != "success" => TimelineStatus::Stale,
        Some(meta_row) => {
            let file_current = indexed_volumes(case_dir, image_id)
                .map(|volumes| file_watermark(&volumes))
                .unwrap_or_default();
            let event_current = event_watermark(case_dir, image_id).unwrap_or_default();
            if file_current == meta_row.source_file_watermark
                && event_current == meta_row.source_event_watermark
            {
                TimelineStatus::Ready
            } else {
                TimelineStatus::Stale
            }
        }
    };
    let (start_time, end_time) = timeline_store::time_range(&conn)?
        .map(|(min, max)| (Some(min), Some(max)))
        .unwrap_or((None, None));
    let record_count = conn.query_row("SELECT COUNT(*) FROM timeline", [], |row| {
        row.get::<_, i64>(0).map(|count| count as u64)
    })?;
    Ok(TimelineStatusInfo {
        status,
        start_time,
        end_time,
        record_count,
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::analysis::results::legacy_store::test_support::{persist, LegacyRun};
    use crate::core::analysis::results::record_store::{
        self, NewRun, RecordWriter, RunCompletion, RunTarget,
    };
    use crate::core::casework::case::test_support::{case_with_evidence, publish_volume};
    use crate::core::casework::case::CaseState;
    use crate::core::catalog::filedb::filedb_store::test_support::entry;
    use crate::core::evidence::image::ReadFaultReport;
    use crate::core::filesystem::tree::fs_tree::NodeKind;
    use crate::core::timeline::timeline_store::TimelineQuery;
    use lovelycarve_plugin::{protocol::WireRecord, FieldKind, FieldSchema, RecordSchema, Value};

    const EID: &str = "img-1";

    /// A case with one evidence whose `part:0` volume has a published index
    /// of three file rows (one without mtime, one deleted).
    fn seeded_case() -> (tempfile::TempDir, CaseState) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut case = case_with_evidence(dir.path(), EID);
        publish(&mut case, "2026-01-01T00:00:00.000Z");
        (dir, case)
    }

    fn publish(case: &mut CaseState, built_at: &str) {
        let mut entries = vec![
            entry("part:0", "n-1", "a.txt", NodeKind::File, "/a.txt"),
            entry("part:0", "n-2", "b.txt", NodeKind::File, "/dir/b.txt"),
            entry("part:0", "n-3", "c.txt", NodeKind::File, "/c.txt"),
        ];
        entries[0].node.timestamps.modified = Some("2024-01-01T00:00:00Z".into());
        entries[1].node.timestamps.modified = Some("2024-06-15T12:30:00Z".into());
        entries[1].node.deleted = true;
        publish_volume(case, EID, "part:0", built_at, &entries);
    }

    /// A legacy run with one valid and one unparseable event.
    fn seed_plugin_events(case_dir: &Path) -> String {
        persist(
            case_dir,
            EID,
            &LegacyRun {
                plugin_id: "org.demo",
                analyzer: "demo.analyzer",
                display_name: "演示功能",
                type_name: "baseinfo",
                node_id: "n-1",
                file_name: "a.txt",
                completed_at: "2026-01-02T00:00:01Z",
                nodes: Vec::new(),
                artifacts: Vec::new(),
                events: vec![
                    (
                        "2024-03-01T08:00:00Z",
                        "browser.visit",
                        "访问 https://example.test",
                    ),
                    ("无法解析的时间", "bad.event", "坏时间戳应被跳过"),
                ],
            },
        )
    }

    /// Publishes a records.db run of the job `job` with two logins: one with
    /// a time zone (projected) and one local time (not projected).
    fn publish_record_run(case_dir: &Path, job: &str) -> (String, record_store::Published) {
        let schema = RecordSchema::new("test.login", "登录")
            .title("{user}")
            .field(FieldSchema::new("user", "用户", FieldKind::Text).key())
            .field(FieldSchema::new("time", "时间", FieldKind::Timestamp).timeline("login"));
        let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
        record_store::begin_run(
            case_dir,
            EID,
            &NewRun {
                run_id: &run_id,
                job_key: job,
                plugin_id: "org.demo",
                plugin_version: "1",
                analyzer: "logins",
                analyzer_name: "登录",
                category: "baseinfo",
                target: &RunTarget::default(),
                options: &serde_json::json!({}),
                profile: &serde_json::json!({}),
                inputs: &[],
                schema_hash: "fnv1a64:0",
                started_at: "2026-01-03T00:00:00Z",
            },
        )
        .expect("begin run");
        let mut writer = RecordWriter::open(case_dir, EID, &run_id, std::slice::from_ref(&schema))
            .expect("writer");
        let login = |user: &str, time: &str| WireRecord {
            record_type: "test.login".to_owned(),
            values: vec![Value::from(user), Value::from(time)],
            title: None,
            severity: None,
            sources: Vec::new(),
        };
        writer
            .insert_batch(&[
                login("root", "2024-04-01T10:00:00+08:00"),
                login("kali", "2024-04-02 09:00:00"),
            ])
            .expect("records");
        drop(writer);
        let published = record_store::publish(
            case_dir,
            EID,
            &run_id,
            &RunCompletion {
                summary: None,
                finished_at: "2026-01-03T00:00:01Z",
                read_faults: &ReadFaultReport::default(),
            },
        )
        .expect("publish")
        .expect("published");
        (run_id, published)
    }
    fn rebuild(case_dir: &Path) -> TimelineBuildSummary {
        let cancel = CancellationToken::new();
        run_rebuild(case_dir, EID, &cancel, &|_, _, _| {}).expect("rebuild")
    }

    fn count_rows(case_dir: &Path, query: &TimelineQuery) -> u64 {
        let Some(conn) = timeline_store::open_reader(case_dir, EID).expect("store") else {
            return 0;
        };
        timeline_store::page(
            &conn,
            EID,
            &timeline_store::TimelinePageQuery {
                query: query.clone(),
                skip: 0,
                limit: u64::MAX,
                ascending: true,
            },
        )
        .expect("page")
        .total
    }

    /// 全行计数(含已删)。
    fn count_all(case_dir: &Path) -> u64 {
        count_rows(
            case_dir,
            &TimelineQuery {
                show_delete: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn rebuild_projects_files_and_events() {
        let (dir, _case) = seeded_case();
        let run_id = seed_plugin_events(dir.path());

        let summary = rebuild(dir.path());
        assert_eq!(summary.status, "success");
        assert_eq!(summary.files_total, 3);
        assert_eq!(summary.file_rows, 2, "无 mtime 的文件被跳过");
        assert_eq!(summary.events_total, 2);
        assert_eq!(summary.event_rows, 1, "坏时间戳事件被跳过");
        assert_eq!(summary.skipped_invalid_time, 2);

        // 投影内容逐字段核对。
        let conn = timeline_store::open_reader(dir.path(), EID)
            .expect("store")
            .expect("built");
        let page = timeline_store::page(
            &conn,
            EID,
            &timeline_store::TimelinePageQuery {
                query: TimelineQuery {
                    show_delete: true,
                    ..Default::default()
                },
                skip: 0,
                limit: 10,
                ascending: true,
            },
        )
        .expect("page");
        assert_eq!(page.total, 3);
        let file_row = page
            .items
            .iter()
            .find(|row| row.data_type == DATA_TYPE_FILE && row.oid == "n-2")
            .expect("deleted file row");
        assert!(file_row.deleted, "filedb is_delete=1 镜像到 timeline");
        assert_eq!(file_row.from_app, "part:0");
        assert_eq!(file_row.tl_flag, "修改");
        assert_eq!(file_row.tl_time, 1718454600);
        assert_eq!(file_row.content, "/dir/b.txt");
        let event_row = page
            .items
            .iter()
            .find(|row| row.data_type == DATA_TYPE_PLUGIN_EVENT)
            .expect("event row");
        assert_eq!(event_row.tid, run_id);
        assert_eq!(event_row.from_app, "org.demo");
        assert_eq!(event_row.tl_flag, "browser.visit");
        assert_eq!(event_row.content, "访问 https://example.test");
    }

    #[test]
    fn rebuild_is_idempotent() {
        let (dir, _case) = seeded_case();
        seed_plugin_events(dir.path());
        let first = rebuild(dir.path());
        let second = rebuild(dir.path());
        assert_eq!(first, second);
        assert_eq!(count_all(dir.path()), 3);
    }

    #[test]
    fn rebuild_cancel_keeps_partial_projection() {
        let (dir, _case) = seeded_case();
        seed_plugin_events(dir.path());
        let cancel = CancellationToken::new();
        let summary = run_rebuild(dir.path(), EID, &cancel, &|_, _, rows| {
            if rows > 0 {
                cancel.cancel();
            }
        })
        .expect("rebuild");
        assert_eq!(summary.status, "cancelled");
        assert_eq!(summary.file_rows, 0, "文件阶段前即取消");
        assert_eq!(summary.event_rows, 1, "事件阶段已落库的部分保留");

        // cancelled ≠ success → 状态机给 stale(建议重建)。
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::Stale);
    }

    #[test]
    fn status_lifecycle_build_required_running_ready_stale() {
        let (dir, mut case) = seeded_case();
        seed_plugin_events(dir.path());

        // 未创建:不建库。
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::BuildRequired);
        assert_eq!(status.record_count, 0);
        assert!(timeline_store::open_reader(dir.path(), EID)
            .expect("reader")
            .is_none());

        // 就绪 + 时间范围。
        rebuild(dir.path());
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::Ready);
        assert_eq!(status.start_time, Some(1704067200)); // 2024-01-01 文件最早
        assert_eq!(status.end_time, Some(1718454600)); // 2024-06-15 文件最晚
        assert_eq!(status.record_count, 3);

        // 卷重建发布了新的索引 → stale。
        publish(&mut case, "2026-02-01T00:00:00.000Z");
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::Stale);

        // 元数据 running 且进程内在跑。
        let store = timeline_store::open_writer(dir.path(), EID).expect("store");
        timeline_store::meta_begin(&store, 1).expect("begin");
        let status = compute_status(dir.path(), EID, true).expect("status");
        assert_eq!(status.status, TimelineStatus::Running);

        // 元数据 running 但进程内不在跑 = 孤儿 → 改判 failed + stale。
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::Stale);
        let meta = timeline_store::meta_get(&store)
            .expect("meta")
            .expect("meta row");
        assert_eq!(meta.status, "failed");
    }

    #[test]
    fn published_runs_project_incrementally_and_replace_their_predecessor() {
        let (dir, _case) = seeded_case();
        rebuild(dir.path());
        assert_eq!(count_all(dir.path()), 2);

        // 插件运行发布:写路径增量投影。只有带时区的时间进时间线。
        let (first, published) = publish_record_run(dir.path(), "org.demo/logins#");
        assert!(published.superseded.is_empty());
        let inserted =
            project_published_run(dir.path(), EID, &first, &published.superseded).expect("hook");
        assert_eq!(inserted, 1);
        assert_eq!(count_all(dir.path()), 3);
        // 水位随钩子前移 → 状态保持就绪。
        let status = compute_status(dir.path(), EID, false).expect("status");
        assert_eq!(status.status, TimelineStatus::Ready);

        let conn = timeline_store::open_reader(dir.path(), EID)
            .expect("store")
            .expect("built");
        let page = timeline_store::page(
            &conn,
            EID,
            &timeline_store::TimelinePageQuery {
                query: TimelineQuery::default(),
                skip: 0,
                limit: 10,
                ascending: true,
            },
        )
        .expect("page");
        let login = page
            .items
            .iter()
            .find(|row| row.data_type == DATA_TYPE_PLUGIN_EVENT)
            .expect("login point");
        assert_eq!(
            (
                login.tl_flag.as_str(),
                login.content.as_str(),
                login.tid.as_str()
            ),
            ("login", "root", first.as_str())
        );
        assert_eq!(login.tl_time, 1_711_936_800, "2024-04-01T02:00:00Z");
        assert!(login.extra.contains("\"recordType\":\"test.login\""));

        // 同一作业再次发布:旧运行的投影被替换,行数不膨胀。
        let (second, published) = publish_record_run(dir.path(), "org.demo/logins#");
        assert_eq!(published.superseded, vec![first.clone()]);
        assert_eq!(
            project_published_run(dir.path(), EID, &second, &published.superseded).expect("again"),
            1
        );
        assert_eq!(count_all(dir.path()), 3);

        // 全量重建读到的是同样的投影。
        rebuild(dir.path());
        assert_eq!(count_all(dir.path()), 3);
    }

    #[test]
    fn publication_skips_uninitialized_timeline() {
        let (dir, _case) = seeded_case();
        let (run_id, published) = publish_record_run(dir.path(), "org.demo/logins#");
        // 时间线从未建过:钩子不产生半初始化状态,交给全量重建。
        assert_eq!(
            project_published_run(dir.path(), EID, &run_id, &published.superseded).expect("hook"),
            0
        );
        assert_eq!(count_rows(dir.path(), &TimelineQuery::default()), 0);
        assert!(timeline_store::open_reader(dir.path(), EID)
            .expect("reader")
            .is_none());
    }
}
