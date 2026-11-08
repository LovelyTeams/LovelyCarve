//! Project retained NTFS change-journal events into the case timeline.
//! Historical MFT numbers are not current node identities: records may have
//! been reused. Keep journal names/references without fabricating a full path.
use crate::{
    filesystem::evidence_session::EvidenceSession,
    filesystem::fs_all::FsAll,
    filesystem::tree::fs_tree::open_ntfs_volume_for_key,
    filesystem::volumes::ntfs_usn::UsnRecord,
    timeline::timeline_project::{ProgressSink, TimelineBuildSummary},
    timeline::timeline_store::{self, TimelineInsertRow, DATA_TYPE_NTFS_USN},
};
use anyhow::Context;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn journal_row(volume_key: &str, record: &UsnRecord) -> Option<TimelineInsertRow> {
    let time = chrono::DateTime::parse_from_rfc3339(record.timestamp.as_deref()?).ok()?;
    let labels: Vec<&str> = record
        .reasons
        .iter()
        .map(|reason| match reason.as_str() {
            "FILE_CREATE" => "创建",
            "FILE_DELETE" => "删除",
            "RENAME_OLD_NAME" => "重命名（旧名称）",
            "RENAME_NEW_NAME" => "重命名（新名称）",
            "DATA_OVERWRITE" => "内容写入",
            "DATA_EXTEND" => "内容扩展",
            "DATA_TRUNCATION" => "内容截断",
            "BASIC_INFO_CHANGE" => "属性修改",
            "SECURITY_CHANGE" => "权限修改",
            "CLOSE" => "关闭",
            other => other,
        })
        .collect();
    Some(TimelineInsertRow {
        oid: format!("usn:{volume_key}:{}", record.usn),
        tid: format!("ntfs-usn:{volume_key}"),
        // FILE_DELETE is an event, not a reason to hide the journal row when
        // the UI excludes currently deleted filesystem metadata.
        deleted: false,
        tl_time: time.timestamp(),
        from_app: format!("NTFS USN · {volume_key}"),
        data_type: DATA_TYPE_NTFS_USN,
        tl_flag: labels.join(" / "),
        content: format!(
            "{} · MFT #{} · 父目录 MFT #{}",
            record.name, record.file_record, record.parent_record
        ),
        extra: serde_json::to_string(record).ok()?,
    })
}

/// Stream journal records directly into SQLite, including logs outside the
/// indexed file tree. Cancellation preserves only the batches already read.
pub fn project_journals(
    image: &Arc<EvidenceSession>,
    conn: &rusqlite::Connection,
    cancel: &CancellationToken,
    summary: &mut TimelineBuildSummary,
    progress: ProgressSink<'_>,
) -> anyhow::Result<()> {
    let mut batch = Vec::with_capacity(2000);
    progress(0, 0, 0); // Unknown journal length in records: indeterminate.
    for entry in FsAll::for_image(image).entries() {
        if cancel.is_cancelled() {
            break;
        }
        let Some(volume) = open_ntfs_volume_for_key(image, &entry.volume_key) else {
            continue;
        };
        let volume =
            volume.with_context(|| format!("打开 {} 的 NTFS 日志失败", entry.volume_key))?;
        volume
            .visit_usn_journal(cancel, |record| {
                summary.events_total += 1;
                if let Some(row) = journal_row(&entry.volume_key, &record) {
                    batch.push(row);
                    summary.event_rows += 1;
                } else {
                    summary.skipped_invalid_time += 1;
                }
                if batch.len() >= 2000 {
                    timeline_store::insert_rows(conn, &batch)?;
                    batch.clear();
                    progress(summary.event_rows, 0, summary.event_rows);
                }
                Ok(())
            })
            .with_context(|| format!("读取 {} 的 $UsnJrnl:$J 失败", entry.volume_key))?;
    }
    timeline_store::insert_rows(conn, &batch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_is_visible_and_retains_historical_identity() {
        let record = UsnRecord {
            usn: 4096,
            file_record: 42,
            parent_record: 5,
            timestamp: Some("2025-06-01T01:02:03Z".into()),
            reasons: vec!["FILE_DELETE".into(), "CLOSE".into()],
            name: "旧文件.txt".into(),
            file_attributes: 32,
        };
        let row = journal_row("part:2", &record).expect("row");
        assert_eq!(
            row,
            TimelineInsertRow {
                oid: "usn:part:2:4096".into(),
                tid: "ntfs-usn:part:2".into(),
                deleted: false,
                tl_time: 1748739723,
                from_app: "NTFS USN · part:2".into(),
                data_type: DATA_TYPE_NTFS_USN,
                tl_flag: "删除 / 关闭".into(),
                content: "旧文件.txt · MFT #42 · 父目录 MFT #5".into(),
                extra: serde_json::to_string(&record).expect("json"),
            }
        );
        let dir = tempfile::tempdir().expect("temp");
        let conn = timeline_store::open_writer(dir.path(), "disk").expect("open");
        timeline_store::insert_rows(&conn, &[row.clone()]).expect("insert");
        let result = timeline_store::page(
            &conn,
            "disk",
            &timeline_store::TimelinePageQuery {
                query: timeline_store::TimelineQuery {
                    data_types: vec![DATA_TYPE_NTFS_USN],
                    ..Default::default()
                },
                skip: 0,
                limit: 10,
                ascending: false,
            },
        )
        .expect("query");
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].extra, row.extra);
        assert_eq!(result.items[0].tl_flag, "删除 / 关闭");
        let invalid = UsnRecord {
            timestamp: None,
            ..record
        };
        assert!(journal_row("part:2", &invalid).is_none());
    }
}
