//! 一个卷的文件系统索引文件:结构、构建连接与只读连接。
//!
//! 每个发布的卷有自己的 SQLite 文件(`evidence/<uid>/volumes/<卷>/index-*.db`)。
//! `core::catalog::filedb::filedb_store` 在 `staging/` 里一次把它建成:不写回滚日志也不用
//! WAL、独占锁定、先批量插入再建二级索引,写完 fsync 后移入卷目录,此后
//! 不再修改。读者以 `immutable=1` 只读打开,SQLite 既不加锁也不做变更检测。
//! 导入再久也只占着一个无人读取的暂存文件,不会挡住任何读者或写者;重建
//! 产生新文件,旧文件在没有读者后回收。
//!
//! 一个文件只装一个卷,行与索引里不再有检材 / 卷编号前缀;卷的身份记在
//! `filedb_meta` 唯一一行里。

use std::path::Path;

use anyhow::Context;
use rusqlite::{params, Connection, OpenFlags};

/// `PRAGMA application_id`:ASCII "LCIX"(LovelyCarve IndeX)。
pub const APPLICATION_ID: i32 = 0x4C43_4958;
/// 索引文件结构版本。不是当前版本的索引按「没有索引」处理并重建。
/// 2:去掉 `name` 索引,新增部分索引 `deleted_dir_path`。
pub const SCHEMA_VERSION: i32 = 2;
/// 索引文件页大小。
pub const PAGE_SIZE: u32 = 16_384;

/// 表结构。二级索引在批量插入之后由 [`create_secondary_indexes`] 建立。
const TABLES: &str = r#"
CREATE TABLE filedb_meta (
    singleton    INTEGER PRIMARY KEY CHECK (singleton = 1),
    image_id     TEXT NOT NULL,
    volume_key   TEXT NOT NULL,
    vol_id       INTEGER NOT NULL,
    built_at     TEXT NOT NULL,
    fingerprint  TEXT NOT NULL,
    entry_count  INTEGER NOT NULL,
    truncated    INTEGER NOT NULL,
    indexed_at   TEXT NOT NULL
);

CREATE TABLE filedb (
    ordinal         INTEGER PRIMARY KEY,
    nid             TEXT NOT NULL,
    pid             TEXT NOT NULL,
    name            TEXT NOT NULL,
    name_low        TEXT NOT NULL,
    full_path       TEXT NOT NULL,
    full_path_low   TEXT NOT NULL,
    kind            TEXT NOT NULL,
    extension       TEXT,
    size            INTEGER NOT NULL,
    mtime           TEXT,
    atime           TEXT,
    ctime           TEXT,
    crtime          TEXT,
    is_delete       INTEGER NOT NULL,
    meta_addr       INTEGER NOT NULL,
    meta_addr_high  INTEGER NOT NULL,
    node_json       TEXT NOT NULL
);

CREATE TABLE filedb_walked_dirs (
    pid  TEXT PRIMARY KEY
) WITHOUT ROWID;

CREATE TABLE filedb_child_counts (
    pid                  TEXT PRIMARY KEY,
    child_count          INTEGER NOT NULL,
    child_delete_count   INTEGER NOT NULL,
    child_count_inexact  INTEGER NOT NULL
) WITHOUT ROWID;
"#;

/// `filedb` 的二级索引。`ordinal` 是 rowid,每条索引都隐含携带,覆盖扫描
/// 拿得到主键而不必回表。
pub const FILEDB_INDEXES: &[(&str, &str)] = &[
    // 目录浏览:父节点定位、自然序排序与 keyset 游标都由这一条覆盖。
    (
        "pid",
        "CREATE INDEX pid ON filedb (pid, name COLLATE natsort, nid)",
    ),
    ("name_low", "CREATE INDEX name_low ON filedb (name_low)"),
    ("full_path", "CREATE INDEX full_path ON filedb (full_path)"),
    (
        "full_path_low",
        "CREATE INDEX full_path_low ON filedb (full_path_low)",
    ),
    ("datatype", "CREATE INDEX datatype ON filedb (kind)"),
    ("extension", "CREATE INDEX extension ON filedb (extension)"),
    ("time", "CREATE INDEX time ON filedb (mtime)"),
    ("pid_time", "CREATE INDEX pid_time ON filedb (pid, mtime)"),
    ("is_delete", "CREATE INDEX is_delete ON filedb (is_delete)"),
    // MetaAddr 通道:(metaAddr, metaAddrHigh) → nid。
    (
        "meta_addr",
        "CREATE INDEX meta_addr ON filedb (meta_addr, meta_addr_high)",
    ),
    // 文件全显以当前节点为根:先按 nid 定位起点行拿到 full_path,再用
    // full_path_low 的前缀区间圈定子树。
    ("nid", "CREATE INDEX nid ON filedb (nid)"),
    // 文件全显分页的排序覆盖索引(内层只取排序键与主键,外层按主键回表):
    //   flat_default —— 默认序 `kind, name natsort, nid`;尾部 extension /
    //     name_low / full_path_low 只为覆盖,关键词、名称通配、路径子串、
    //     扩展名与类别条过滤都在索引内求值,取页与 COUNT 都不回表。
    //   flat_name —— 名称序 `name natsort, nid`,kind 作为索引内残差。它也服务
    //     其它按名称自然序的扫描(如 `filedb_query::query_files`),所以没有
    //     单独的 `(name, nid)` 索引。
    (
        "flat_default",
        "CREATE INDEX flat_default ON filedb \
         (kind, name COLLATE natsort, nid, extension, name_low, full_path_low)",
    ),
    (
        "flat_name",
        "CREATE INDEX flat_name ON filedb (name COLLATE natsort, nid, kind)",
    ),
    // 文件全显的已删除目录探测(`filedb_flat::deleted_dirs_under`)专用部分
    // 索引:只收已删除目录,整卷起点扫它,目录起点按前缀区间定位,开销只与
    // 范围内的已删目录数有关,与整卷的已删除行数无关。条件里的 1 是
    // `meta_addr::IS_DELETE_DELETED`;探测必须写同一常量(绑定参数匹配不上
    // 部分索引的条件)并以 INDEXED BY 锁定。
    (
        "deleted_dir_path",
        "CREATE INDEX deleted_dir_path ON filedb (full_path_low) \
         WHERE kind = 'dir' AND is_delete = 1",
    ),
];

/// 索引文件里记录的卷身份与构建结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub image_id: String,
    pub volume_key: String,
    pub vol_id: i64,
    /// 来源快照的构建时间。
    pub built_at: String,
    pub fingerprint: String,
    pub entry_count: u64,
    pub truncated: bool,
    pub indexed_at: String,
}

/// 新建一个索引文件并返回构建连接。文件已存在时失败。连接为单写者批量
/// 导入配置:无日志、不同步、独占锁定;崩溃留下的半成品只会是暂存文件。
pub fn create_build_connection(path: &Path) -> anyhow::Result<Connection> {
    if path.exists() {
        anyhow::bail!("index file {} already exists", path.display());
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("create filesystem index {}", path.display()))?;
    crate::common::natsort::register_collation(&conn).context("register natsort collation")?;
    // page_size 必须在第一次写入之前设置。
    conn.execute_batch(&format!(
        "PRAGMA page_size = {PAGE_SIZE};
         PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE;
         PRAGMA cache_size = -65536;
         PRAGMA application_id = {APPLICATION_ID};
         PRAGMA user_version = {SCHEMA_VERSION};"
    ))?;
    conn.execute_batch(TABLES)?;
    Ok(conn)
}

/// 批量插入完成后建全部二级索引。
pub(crate) fn create_secondary_indexes(conn: &Connection) -> anyhow::Result<()> {
    for (name, ddl) in FILEDB_INDEXES {
        conn.execute_batch(ddl)
            .with_context(|| format!("create filedb index {name}"))?;
    }
    Ok(())
}

/// 写入卷身份行(构建事务内调用)。
pub fn write_meta(conn: &Connection, meta: &IndexMeta) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO filedb_meta (singleton, image_id, volume_key, vol_id, built_at, fingerprint,
                                  entry_count, truncated, indexed_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            meta.image_id,
            meta.volume_key,
            meta.vol_id,
            meta.built_at,
            meta.fingerprint,
            i64::try_from(meta.entry_count).context("index entry count exceeds i64")?,
            meta.truncated,
            meta.indexed_at,
        ],
    )?;
    Ok(())
}

/// 以只读、不可变方式打开一个已发布的索引文件。
pub fn open_reader(path: &Path) -> anyhow::Result<Connection> {
    let uri = sqlite_file_uri(path, "mode=ro&immutable=1");
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open filesystem index {}", path.display()))?;
    crate::common::natsort::register_collation(&conn).context("register natsort collation")?;
    let application_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application_id != APPLICATION_ID || version != SCHEMA_VERSION {
        anyhow::bail!(
            "{} is not a filesystem index of this build (application id {application_id:#x}, \
             schema {version}); rebuild the volume snapshot",
            path.display()
        );
    }
    Ok(conn)
}

/// 读卷身份行。
pub fn read_meta(conn: &Connection) -> anyhow::Result<IndexMeta> {
    conn.query_row(
        "SELECT image_id, volume_key, vol_id, built_at, fingerprint, entry_count, truncated,
                indexed_at
         FROM filedb_meta WHERE singleton = 1",
        [],
        |row| {
            let entry_count: i64 = row.get(5)?;
            Ok(IndexMeta {
                image_id: row.get(0)?,
                volume_key: row.get(1)?,
                vol_id: row.get(2)?,
                built_at: row.get(3)?,
                fingerprint: row.get(4)?,
                entry_count: u64::try_from(entry_count).unwrap_or(0),
                truncated: row.get(6)?,
                indexed_at: row.get(7)?,
            })
        },
    )
    .context("read filesystem index identity")
}

/// SQLite `file:` URI。路径里 URI 保留字符与非 ASCII 字节一律百分号转义;
/// Windows 盘符路径写成 `file:///C:/…`,UNC 路径写成 `file:////server/share/…`。
pub fn sqlite_file_uri(path: &Path, query: &str) -> String {
    let raw = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    let normalized = {
        let without_verbatim = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else if let Some(local) = raw.strip_prefix(r"\\?\") {
            local.to_string()
        } else {
            raw.to_string()
        };
        let slashed = without_verbatim.replace('\\', "/");
        if slashed.starts_with("//") {
            slashed
        } else {
            format!("/{slashed}")
        }
    };
    #[cfg(not(target_os = "windows"))]
    let normalized = raw.to_string();
    let mut uri = String::from("file://");
    for byte in normalized.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(char::from(byte));
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(query);
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal(path: &Path) {
        let conn = create_build_connection(path).expect("create");
        write_meta(
            &conn,
            &IndexMeta {
                image_id: "img".into(),
                volume_key: "part:0".into(),
                vol_id: 3,
                built_at: "2026-10-09T00:00:00.000Z".into(),
                fingerprint: "fp".into(),
                entry_count: 0,
                truncated: false,
                indexed_at: "2026-10-09T00:00:01.000Z".into(),
            },
        )
        .expect("meta");
        create_secondary_indexes(&conn).expect("indexes");
    }

    #[test]
    fn built_files_open_read_only_through_awkward_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let nested = directory.path().join("案件 #1 %20 &x");
        std::fs::create_dir_all(&nested).expect("nested");
        let path = nested.join("index-test.db");
        build_minimal(&path);
        assert!(create_build_connection(&path).is_err(), "never overwrites");

        let conn = open_reader(&path).expect("open reader");
        let meta = read_meta(&conn).expect("meta");
        assert_eq!((meta.vol_id, meta.volume_key.as_str()), (3, "part:0"));
        let page_size: u32 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .expect("page size");
        assert_eq!(page_size, PAGE_SIZE);
        assert!(conn.execute("DELETE FROM filedb", []).is_err(), "read only");
        let indexes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count indexes");
        assert_eq!(indexes, FILEDB_INDEXES.len() as i64);
    }

    #[test]
    fn other_files_are_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("other.db");
        Connection::open(&path)
            .expect("open")
            .execute_batch("CREATE TABLE x (y);")
            .expect("seed");
        let error = open_reader(&path).expect_err("not an index");
        assert!(format!("{error:#}").contains("rebuild the volume snapshot"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_become_file_uris() {
        assert_eq!(
            sqlite_file_uri(Path::new(r"C:\cases\a b\index.db"), "immutable=1"),
            "file:///C:/cases/a%20b/index.db?immutable=1"
        );
        assert_eq!(
            sqlite_file_uri(Path::new(r"\\?\D:\x\y.db"), ""),
            "file:///D:/x/y.db"
        );
        assert_eq!(
            sqlite_file_uri(Path::new(r"\\server\share\y.db"), ""),
            "file:////server/share/y.db"
        );
    }
}
