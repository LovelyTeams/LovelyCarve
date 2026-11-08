//! 卷索引构建:把一份 `.dirz` 快照导入一个全新的索引文件
//! ([`crate::catalog::filedb::filedb_file`])。
//!
//! 生产者线程解码快照并序列化成有界批次,构建线程逐行插入并流式累计目录
//! 覆盖与子项计数;全部行写完后一次写入计数缓存与卷身份行,再建全部二级
//! 索引,最后 fsync。整个构建只写一个还没有人读取的暂存文件:不占任何共享
//! 库的写锁,也不需要回滚——取消或失败时直接删除暂存文件,上一次发布的索引
//! 原样继续服务。发布(移入卷目录并登记台账)由案件层完成。
//!
//! 每行保存名称与路径的小写副本(`name_low`、`full_path_low`)与 MetaAddr
//! 列,查询侧从不在读时折叠大小写。

use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection};
use tokio_util::sync::CancellationToken;

use crate::catalog::filedb::filedb_bulk::DirectoryTally;
use crate::catalog::filedb::filedb_file::{self, IndexMeta};
use crate::catalog::snapshot::volume_snapshot::{self, SnapshotEntry, SnapshotHeader};
use crate::filesystem::meta_addr;
use crate::filesystem::tree::fs_tree::NodeKind;

/// Rows per producer batch. Sized so insert throughput stays high while the
/// bounded channel keeps producer memory flat.
const INDEX_BATCH_SIZE: usize = 4_096;

/// 被索引的卷与它来自的快照。
#[derive(Debug, Clone)]
pub struct IndexSource<'a> {
    pub image_id: &'a str,
    pub volume_key: &'a str,
    pub vol_id: i64,
    /// 来源快照的构建时间(发布时据此确认快照仍是当前快照)。
    pub built_at: &'a str,
    pub fingerprint: &'a str,
    pub truncated: bool,
}

/// 构建期间删除半成品:未调用 [`PartialIndex::keep`] 就离开作用域时删除
/// 输出文件。必须先于连接声明,连接先关闭(Windows 上关闭前删不掉)。
struct PartialIndex<'p> {
    path: &'p Path,
    keep: bool,
}

impl<'p> PartialIndex<'p> {
    fn new(path: &'p Path) -> Self {
        Self { path, keep: false }
    }

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PartialIndex<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Err(error) = crate::casework::case_paths::remove_database_files(self.path) {
            tracing::warn!(
                "filedb: removing partial index {} failed: {error}",
                self.path.display()
            );
        }
    }
}

/// 把 `snapshot_path` 导入新文件 `output`(必须不存在),返回行数。
/// `on_progress` 收到已导入行数;`cancel` 在批间与 SQLite 进度回调里轮询,
/// 二级索引建立期间同样响应取消。
pub fn build_index(
    snapshot_path: &Path,
    output: &Path,
    source: &IndexSource<'_>,
    mut on_progress: Option<&mut dyn FnMut(u64)>,
    cancel: Option<&CancellationToken>,
) -> anyhow::Result<u64> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        anyhow::bail!("filedb index build cancelled");
    }
    let partial = PartialIndex::new(output);
    let conn = filedb_file::create_build_connection(output)?;
    if let Some(cancel) = cancel.cloned() {
        conn.progress_handler(1_000, Some(move || cancel.is_cancelled()))?;
    }
    let tx = conn.unchecked_transaction()?;

    let (sender, receiver) = std::sync::mpsc::sync_channel::<anyhow::Result<IndexBatch>>(2);
    let producer_path = snapshot_path.to_path_buf();
    let expected_image = source.image_id.to_string();
    let expected_volume = source.volume_key.to_string();
    let producer = std::thread::spawn(move || {
        produce_index_batches(&producer_path, &expected_image, &expected_volume, &sender)
    });

    let consumer_result = {
        (|| -> anyhow::Result<(u64, DirectoryTally)> {
            let mut node_stmt = prepare_insert(&tx)?;
            let mut count = 0u64;
            let mut directories = DirectoryTally::new(source.volume_key);
            for batch in receiver.iter() {
                let batch =
                    batch.map_err(|_| anyhow::anyhow!("filedb index producer disconnected"))?;
                for row in batch.rows {
                    insert_row(&mut node_stmt, &row)?;
                    tally_row(&mut directories, row);
                    count += 1;
                }
                if let Some(progress) = on_progress.as_deref_mut() {
                    progress(count);
                }
                if cancel.is_some_and(CancellationToken::is_cancelled) {
                    anyhow::bail!("filedb index build cancelled");
                }
            }
            Ok((count, directories))
        })()
    };
    // The iterator only borrows the receiver. Disconnect it before joining:
    // on cancellation/error the producer may be blocked on the full channel.
    drop(receiver);
    let producer_result = producer.join();
    let (count, directories) = consumer_result?;
    let streamed_count = match producer_result {
        Ok(Ok(count)) => count,
        Ok(Err(err)) => return Err(err),
        Err(_) => anyhow::bail!("filedb index producer thread panicked"),
    };
    if streamed_count != count {
        anyhow::bail!(
            "snapshot stream count mismatch: visited {count}, reader reported {streamed_count}"
        );
    }
    finish_build(&tx, &directories, source, count)?;
    tx.commit()?;
    filedb_file::create_secondary_indexes(&conn)?;
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        anyhow::bail!("filedb index build cancelled");
    }
    close_and_sync(conn, output)?;
    partial.keep();
    Ok(count)
}

/// 由内存条目直接建索引(测试与基准用)。
pub fn build_index_from_entries(
    entries: &[SnapshotEntry],
    output: &Path,
    source: &IndexSource<'_>,
) -> anyhow::Result<u64> {
    let partial = PartialIndex::new(output);
    let conn = filedb_file::create_build_connection(output)?;
    let tx = conn.unchecked_transaction()?;
    let mut directories = DirectoryTally::new(source.volume_key);
    {
        let mut node_stmt = prepare_insert(&tx)?;
        for (ordinal, entry) in entries.iter().enumerate() {
            let row = index_row_from_entry(ordinal, entry)?;
            insert_row(&mut node_stmt, &row)?;
            tally_row(&mut directories, row);
        }
    }
    let count = u64::try_from(entries.len()).context("entry count exceeds u64")?;
    finish_build(&tx, &directories, source, count)?;
    tx.commit()?;
    filedb_file::create_secondary_indexes(&conn)?;
    close_and_sync(conn, output)?;
    partial.keep();
    Ok(count)
}

fn finish_build(
    conn: &Connection,
    directories: &DirectoryTally,
    source: &IndexSource<'_>,
    count: u64,
) -> anyhow::Result<()> {
    directories.publish(conn, source.truncated)?;
    filedb_file::write_meta(
        conn,
        &IndexMeta {
            image_id: source.image_id.to_string(),
            volume_key: source.volume_key.to_string(),
            vol_id: source.vol_id,
            built_at: source.built_at.to_string(),
            fingerprint: source.fingerprint.to_string(),
            entry_count: count,
            truncated: source.truncated,
            indexed_at: crate::casework::case_ledger::now(),
        },
    )
}

/// 关闭构建连接并把文件刷到磁盘,之后才能发布。
fn close_and_sync(conn: Connection, output: &Path) -> anyhow::Result<()> {
    conn.close()
        .map_err(|(_, error)| error)
        .context("close filesystem index")?;
    // FlushFileBuffers 需要写权限。
    std::fs::OpenOptions::new()
        .write(true)
        .open(output)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("sync filesystem index {}", output.display()))
}

/// Feed one inserted row to the directory tally, moving its ids.
fn tally_row(directories: &mut DirectoryTally, row: IndexRow) {
    if row.is_walked_dir {
        directories.record_walked_dir(row.nid);
    }
    directories.record_child(row.pid, row.is_delete);
}

fn prepare_insert<'conn>(conn: &'conn Connection) -> rusqlite::Result<rusqlite::Statement<'conn>> {
    conn.prepare(
        "INSERT INTO filedb (
            ordinal, nid, pid, name, name_low, full_path, full_path_low, kind, extension,
            size, mtime, atime, ctime, crtime, is_delete, meta_addr, meta_addr_high, node_json
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
    )
}

/// One fully owned, serialized filedb row, decoded on the producer thread so
/// the build thread only binds parameters and executes.
struct IndexRow {
    ordinal: i64,
    pid: String,
    nid: String,
    name: String,
    name_low: String,
    full_path: String,
    full_path_low: String,
    kind: &'static str,
    extension: Option<String>,
    size: i64,
    mtime: Option<String>,
    atime: Option<String>,
    ctime: Option<String>,
    crtime: Option<String>,
    is_delete: i64,
    meta_addr: i64,
    meta_addr_high: i64,
    node_json: String,
    is_walked_dir: bool,
}

struct IndexBatch {
    rows: Vec<IndexRow>,
}

fn index_row_from_entry(ordinal: usize, entry: &SnapshotEntry) -> anyhow::Result<IndexRow> {
    let ordinal = i64::try_from(ordinal).context("snapshot ordinal exceeds i64")?;
    let node_json = serde_json::to_string(&entry.node)?;
    let size = i64::try_from(entry.node.size).context("snapshot size exceeds SQLite range")?;
    let (meta_addr, meta_addr_high) = meta_addr::classify(&entry.node.id).columns();
    let timestamps = &entry.node.timestamps;
    Ok(IndexRow {
        ordinal,
        pid: entry.parent.clone(),
        nid: entry.node.id.clone(),
        name_low: entry.node.name.to_lowercase(),
        name: entry.node.name.clone(),
        full_path_low: entry.path.to_lowercase(),
        full_path: entry.path.clone(),
        kind: node_kind_as_str(entry.node.kind),
        extension: catalog_extension(&entry.node.name, entry.node.kind),
        size,
        mtime: timestamps.modified.clone(),
        atime: timestamps.accessed.clone(),
        ctime: timestamps.mft_changed.clone(),
        crtime: timestamps.created.clone(),
        is_delete: meta_addr::is_delete_value(entry.node.deleted),
        meta_addr,
        meta_addr_high,
        node_json,
        is_walked_dir: entry.node.kind == NodeKind::Dir && !entry.node.deleted,
    })
}

fn insert_row(node_stmt: &mut rusqlite::Statement<'_>, row: &IndexRow) -> anyhow::Result<()> {
    node_stmt.execute(params![
        row.ordinal,
        row.nid,
        row.pid,
        row.name,
        row.name_low,
        row.full_path,
        row.full_path_low,
        row.kind,
        row.extension,
        row.size,
        row.mtime,
        row.atime,
        row.ctime,
        row.crtime,
        row.is_delete,
        row.meta_addr,
        row.meta_addr_high,
        row.node_json,
    ])?;
    Ok(())
}

fn validate_snapshot_header(
    header: &SnapshotHeader,
    image_id: &str,
    volume_key: &str,
) -> anyhow::Result<()> {
    if header.image_id != image_id {
        anyhow::bail!(
            "snapshot image mismatch: file has {}, expected {image_id}",
            header.image_id
        );
    }
    if header.volume_key != volume_key {
        anyhow::bail!(
            "snapshot volume mismatch: file has {}, expected {volume_key}",
            header.volume_key
        );
    }
    Ok(())
}

/// Decode and serialize snapshot entries into bounded batches. Runs on the
/// producer thread; the sender failing means the consumer is gone (cancelled
/// or failed) and the walk aborts — its error already describes the outcome.
fn produce_index_batches(
    snapshot_path: &Path,
    image_id: &str,
    volume_key: &str,
    sender: &std::sync::mpsc::SyncSender<anyhow::Result<IndexBatch>>,
) -> anyhow::Result<u64> {
    let mut count = 0usize;
    let mut batch = IndexBatch {
        rows: Vec::with_capacity(INDEX_BATCH_SIZE),
    };
    let (_, streamed_count) = volume_snapshot::visit_snapshot_entries_checked(
        snapshot_path,
        |header| validate_snapshot_header(header, image_id, volume_key),
        |entry| {
            batch.rows.push(index_row_from_entry(count, &entry)?);
            count += 1;
            if batch.rows.len() >= INDEX_BATCH_SIZE {
                let next = IndexBatch {
                    rows: Vec::with_capacity(INDEX_BATCH_SIZE),
                };
                if sender
                    .send(Ok(std::mem::replace(&mut batch, next)))
                    .is_err()
                {
                    anyhow::bail!("filedb index consumer stopped");
                }
            }
            Ok(())
        },
    )
    .with_context(|| format!("stream snapshot entries {}", snapshot_path.display()))?;
    if streamed_count != count {
        anyhow::bail!(
            "snapshot stream count mismatch: visited {count}, reader reported {streamed_count}"
        );
    }
    if !batch.rows.is_empty() && sender.send(Ok(batch)).is_err() {
        anyhow::bail!("filedb index consumer stopped");
    }
    u64::try_from(count).context("snapshot entry count exceeds u64")
}

pub(crate) fn node_kind_as_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Symlink => "symlink",
        NodeKind::Unallocated => "unallocated",
    }
}

fn catalog_extension(name: &str, kind: NodeKind) -> Option<String> {
    if kind == NodeKind::Dir {
        return None;
    }
    let (_, extension) = name.rsplit_once('.')?;
    if extension.is_empty() || name.starts_with('.') && !name[1..].contains('.') {
        return None;
    }
    Some(extension.to_lowercase())
}

/// 暂存目录里一个新索引文件的路径(`<卷名>-<令牌>.db`)。
pub fn staged_index_path(staging_dir: &Path, volume_key: &str) -> PathBuf {
    staging_dir.join(format!(
        "{}-{}.db",
        crate::casework::case_paths::volume_segment(volume_key),
        crate::casework::case_paths::new_token()
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::path::{Path, PathBuf};

    use super::IndexSource;
    use crate::catalog::snapshot::volume_snapshot::{
        write_snapshot_entry, write_snapshot_header, SnapshotEntry, SnapshotHeader,
    };
    use crate::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};

    pub fn node(id: &str, name: &str, kind: NodeKind) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: None,
            kind,
            size: 123,
            deleted: false,
            inode: Some(42),
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    }

    pub fn entry(parent: &str, id: &str, name: &str, kind: NodeKind, path: &str) -> SnapshotEntry {
        SnapshotEntry {
            parent: parent.to_string(),
            node: node(id, name, kind),
            path: path.to_string(),
        }
    }

    pub fn entries() -> Vec<SnapshotEntry> {
        vec![
            entry("part:0", "dir-b", "b", NodeKind::Dir, "/b"),
            entry("part:0", "file-a", "a", NodeKind::File, "/a"),
            entry("dir-b", "empty", "empty", NodeKind::Dir, "/b/empty"),
            entry("dir-b", "file-c", "c", NodeKind::File, "/b/c"),
        ]
    }

    pub fn header() -> SnapshotHeader {
        SnapshotHeader {
            schema_version: crate::catalog::snapshot::volume_snapshot::SNAPSHOT_SCHEMA,
            volume_key: "part:0".to_string(),
            image_id: "img".to_string(),
            source_volume_size: 1024,
            built_at: "2026-07-27T00:00:00Z".to_string(),
        }
    }

    /// The index source of the `img` / `part:0` test volume.
    pub fn source(volume_key: &str) -> IndexSource<'_> {
        IndexSource {
            image_id: "img",
            volume_key,
            vol_id: 0,
            built_at: "2026-07-27T00:00:00Z",
            fingerprint: "sha256:fp",
            truncated: false,
        }
    }

    /// Builds `entries` into `<dir>/index-<volume>.db` and returns its path.
    pub fn build_test_index(dir: &Path, volume_key: &str, entries: &[SnapshotEntry]) -> PathBuf {
        let path = dir.join(format!(
            "index-{}.db",
            crate::casework::case_paths::volume_segment(volume_key)
        ));
        super::build_index_from_entries(entries, &path, &source(volume_key)).expect("build index");
        path
    }

    pub fn write_snapshot_file(path: &Path, entries: &[SnapshotEntry]) {
        write_snapshot_file_with_header(path, &header(), entries);
    }

    pub fn write_snapshot_file_with_header(
        path: &Path,
        header: &SnapshotHeader,
        entries: &[SnapshotEntry],
    ) {
        let file = std::fs::File::create(path).expect("create snapshot file");
        let mut encoder =
            zstd::stream::write::Encoder::new(std::io::BufWriter::new(file), 3).expect("encoder");
        write_snapshot_header(&mut encoder, header).expect("header");
        let mut scratch = Vec::new();
        for entry in entries {
            write_snapshot_entry(&mut encoder, entry, &mut scratch).expect("entry");
        }
        encoder.finish().expect("finish");
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::filesystem::tree::fs_tree::NodeKind;

    fn reader(path: &Path) -> Connection {
        filedb_file::open_reader(path).expect("open index")
    }

    #[test]
    fn built_index_holds_rows_lowercase_copies_and_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut all = entries();
        all.push(entry(
            "part:0",
            "ntfs:part:0:77",
            "Photo IMG_10.JPG",
            NodeKind::File,
            "/Photo IMG_10.JPG",
        ));
        let path = build_test_index(dir.path(), "part:0", &all);
        let conn = reader(&path);
        let row: (String, String, String, Option<String>, i64, i64, i64) = conn
            .query_row(
                "SELECT name_low, full_path, full_path_low, extension, is_delete, meta_addr,
                        meta_addr_high
                 FROM filedb WHERE nid = 'ntfs:part:0:77'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("row");
        assert_eq!(
            row,
            (
                "photo img_10.jpg".to_string(),
                "/Photo IMG_10.JPG".to_string(),
                "/photo img_10.jpg".to_string(),
                Some("jpg".to_string()),
                2,
                77,
                0
            )
        );
        let counts: Vec<(String, i64, i64, i64)> = conn
            .prepare(
                "SELECT pid, child_count, child_delete_count, child_count_inexact
                 FROM filedb_child_counts ORDER BY pid",
            )
            .expect("prepare")
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("dir-b".to_string(), 2, 0, 0),
                ("empty".to_string(), 0, 0, 0),
                ("part:0".to_string(), 3, 0, 0),
            ]
        );
        let meta = filedb_file::read_meta(&conn).expect("meta");
        assert_eq!(
            (
                meta.image_id.as_str(),
                meta.volume_key.as_str(),
                meta.entry_count
            ),
            ("img", "part:0", 5)
        );
    }

    #[test]
    fn deleted_entries_use_is_delete_one_and_feed_delete_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut deleted = entry(
            "part:0",
            "ntfs:part:0:90",
            "gone.txt",
            NodeKind::File,
            "/gone.txt",
        );
        deleted.node.deleted = true;
        let path = build_test_index(
            dir.path(),
            "part:0",
            &[
                entry("part:0", "file-a", "a", NodeKind::File, "/a"),
                deleted,
            ],
        );
        let conn = reader(&path);
        let (count, deleted_count): (i64, i64) = conn
            .query_row(
                "SELECT child_count, child_delete_count FROM filedb_child_counts
                 WHERE pid = 'part:0'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("counts");
        assert_eq!((count, deleted_count), (2, 1));
    }

    #[test]
    fn only_walked_directories_get_coverage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut gone = entry("part:0", "gone", "gone", NodeKind::Dir, "/gone");
        gone.node.deleted = true;
        let path = build_test_index(
            dir.path(),
            "part:0",
            &[
                entry("part:0", "live", "live", NodeKind::Dir, "/live"),
                gone,
                entry(
                    "gone",
                    "orphan",
                    "orphan.txt",
                    NodeKind::File,
                    "/gone/orphan.txt",
                ),
                entry("live", "a", "a.txt", NodeKind::File, "/live/a.txt"),
            ],
        );
        let conn = reader(&path);
        let walked: Vec<String> = conn
            .prepare("SELECT pid FROM filedb_walked_dirs ORDER BY pid")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(walked, vec!["live".to_string(), "part:0".to_string()]);
    }

    #[test]
    fn snapshot_file_streams_into_a_new_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("tree.dirz");
        write_snapshot_file(&snapshot, &entries());
        let output = dir.path().join("index.db");
        let mut source = source("part:0");
        source.truncated = true;
        let mut seen = 0;
        let mut progress = |count| seen = count;
        let count =
            build_index(&snapshot, &output, &source, Some(&mut progress), None).expect("build");
        assert_eq!((count, seen), (4, 4));
        let conn = reader(&output);
        let inexact: i64 = conn
            .query_row(
                "SELECT child_count_inexact FROM filedb_child_counts WHERE pid = 'part:0'",
                [],
                |row| row.get(0),
            )
            .expect("inexact");
        assert_eq!(inexact, 1, "truncated snapshots publish inexact counts");
        assert!(
            build_index(&snapshot, &output, &source, None, None).is_err(),
            "an existing file is never overwritten"
        );
    }

    #[test]
    fn mismatched_header_and_cancellation_leave_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("wrong.dirz");
        let mut header = header();
        header.volume_key = "part:9".to_string();
        write_snapshot_file_with_header(&snapshot, &header, &entries());
        let output = dir.path().join("index.db");
        let error = build_index(&snapshot, &output, &source("part:0"), None, None)
            .expect_err("volume mismatch");
        assert!(format!("{error:#}").contains("snapshot volume mismatch"));
        assert!(!output.exists());

        let snapshot = dir.path().join("tree.dirz");
        write_snapshot_file(&snapshot, &entries());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = build_index(&snapshot, &output, &source("part:0"), None, Some(&cancel))
            .expect_err("cancelled");
        assert!(format!("{error:#}").contains("cancelled"));
        assert!(!output.exists());
    }

    #[test]
    fn mid_import_cancellation_disconnects_the_producer() {
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("large.dirz");
        let large: Vec<_> = (0..INDEX_BATCH_SIZE * 5)
            .map(|ordinal| {
                entry(
                    "part:0",
                    &format!("new-{ordinal}"),
                    "new",
                    NodeKind::File,
                    "/new",
                )
            })
            .collect();
        write_snapshot_file(&snapshot, &large);
        let output = dir.path().join("index.db");
        let worker_output = output.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let cancel = CancellationToken::new();
            let mut progress = |_| cancel.cancel();
            let result = build_index(
                &snapshot,
                &worker_output,
                &source("part:0"),
                Some(&mut progress),
                Some(&cancel),
            );
            sender
                .send(result.map_err(|error| format!("{error:#}")))
                .expect("report");
        });
        let error = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("cancel must not deadlock joining the producer")
            .expect_err("cancelled");
        worker.join().expect("worker");
        assert!(error.contains("cancelled"), "{error}");
        assert!(!output.exists());
    }
}
