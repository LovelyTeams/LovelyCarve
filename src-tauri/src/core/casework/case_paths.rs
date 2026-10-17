//! 案件目录布局:案件内每个文件与目录的名字只在这里定义。
//!
//! ```text
//! <案件目录>/
//!   case.db                    案件台账(core::casework::case_ledger):身份、检材、卷发布、任务、标签、审计
//!   case.lock                  进程独占锁(core::casework::case_lock)
//!   staging/                   构建中的快照与索引;打开案件时清空
//!   evidence/<uid>/            一检材一目录,uid = 检材 image id;移除检材即删除该目录
//!     facts.db                 文件哈希与类型识别(core::casework::facts_store)
//!     analysis.db              插件分析结果(core::analysis::results::plugin_result_store)
//!     timeline.db              时间线投影(core::timeline::timeline_store)
//!     search.db                全局搜索任务与命中(core::search::global_search_store)
//!     cache/                   可随时删除的缓存(Docker 容器来源映射、ZIP 备份展开清单等)
//!     volumes/<卷名>/          一卷一目录
//!       tree-<token>.dirz      卷快照(目录树交换格式)
//!       index-<token>.db       由快照导入的文件系统索引,发布后不再修改
//! ```
//!
//! 拆库按「写者」而不是按业务领域:SQLite 在 WAL 下读写互不阻塞,冲突只发生
//! 在同一个库文件的两个写者之间。台账只承受毫秒级短事务;与文件数量同阶的
//! 数据落在检材目录下,每个库只有一类写者——卷索引在 `staging/` 里建成独立
//! 文件后整体发布,发布后只读,导入再久也不占任何共享库的写锁。

use std::path::{Path, PathBuf};

/// 案件台账文件名。
pub const LEDGER_FILE: &str = "case.db";
/// 进程锁文件名。
pub const LOCK_FILE: &str = "case.lock";
/// 构建中间产物目录。
pub const STAGING_DIR: &str = "staging";
/// 检材目录的父目录。
pub const EVIDENCE_DIR: &str = "evidence";
/// 检材目录下的卷目录。
pub const VOLUMES_DIR: &str = "volumes";
/// 检材目录下的可丢弃缓存目录。
pub const CACHE_DIR: &str = "cache";

/// 卷快照文件名前缀与扩展名(`tree-<token>.dirz`)。
pub const SNAPSHOT_PREFIX: &str = "tree-";
pub const SNAPSHOT_EXTENSION: &str = "dirz";
/// 卷索引文件名前缀与扩展名(`index-<token>.db`)。
pub const INDEX_PREFIX: &str = "index-";
pub const INDEX_EXTENSION: &str = "db";

/// 卷目录名的长度上限。更长的卷键(Docker 容器根)截断后接哈希,控制整条
/// 路径长度。
const VOLUME_SEGMENT_MAX: usize = 32;

/// 检材目录下按写者划分的数据库。每个库只有一类写者:
///
/// | 库 | 写者 | 写入形态 |
/// |---|---|---|
/// | `facts.db` | 哈希 / 类型识别命令 | 批量 upsert,短事务 |
/// | `analysis.db` | 插件任务提交 | 每次运行一个 IMMEDIATE 事务 |
/// | `timeline.db` | 重建任务、插件收尾投影 | 分批事务 |
/// | `search.db` | 运行中的全局搜索 | 命中分批落库 |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvidenceDb {
    Facts,
    Analysis,
    Timeline,
    Search,
}

impl EvidenceDb {
    pub const ALL: [Self; 4] = [Self::Facts, Self::Analysis, Self::Timeline, Self::Search];

    pub fn file_name(self) -> &'static str {
        match self {
            Self::Facts => "facts.db",
            Self::Analysis => "analysis.db",
            Self::Timeline => "timeline.db",
            Self::Search => "search.db",
        }
    }
}

/// 目录名安全化:保留 ASCII 字母数字、`-`、`_`、`.`,`:` 换成 `-`,其余换成 `_`。
pub fn path_segment(raw: &str, empty_fallback: &str) -> String {
    let mut segment = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => segment.push(character),
            ':' => segment.push('-'),
            _ => segment.push('_'),
        }
    }
    let segment = segment.trim_matches(['.', '-']);
    if segment.is_empty() {
        empty_fallback.to_string()
    } else {
        segment.to_string()
    }
}

/// 卷目录名:`part:1` → `part-1`,`lvm:2:0` → `lvm-2-0`。过长的卷键截断后接
/// 8 位十六进制哈希,不同卷键不会落到同一目录。
pub fn volume_segment(volume_key: &str) -> String {
    let segment = path_segment(volume_key, "root");
    if segment.len() <= VOLUME_SEGMENT_MAX {
        return segment;
    }
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(volume_key.as_bytes()));
    let keep = VOLUME_SEGMENT_MAX - 9;
    format!("{}-{}", &segment[..keep], &digest[..8])
}

pub fn ledger_path(case_dir: &Path) -> PathBuf {
    case_dir.join(LEDGER_FILE)
}

pub fn lock_path(case_dir: &Path) -> PathBuf {
    case_dir.join(LOCK_FILE)
}

pub fn staging_dir(case_dir: &Path) -> PathBuf {
    case_dir.join(STAGING_DIR)
}

pub fn evidence_root(case_dir: &Path) -> PathBuf {
    case_dir.join(EVIDENCE_DIR)
}

/// 检材目录 `evidence/<uid>`。
pub fn evidence_dir(case_dir: &Path, image_id: &str) -> PathBuf {
    evidence_root(case_dir).join(path_segment(image_id, "evidence"))
}

pub fn evidence_db_path(case_dir: &Path, image_id: &str, db: EvidenceDb) -> PathBuf {
    evidence_dir(case_dir, image_id).join(db.file_name())
}

pub fn cache_dir(case_dir: &Path, image_id: &str) -> PathBuf {
    evidence_dir(case_dir, image_id).join(CACHE_DIR)
}

pub fn volumes_dir(case_dir: &Path, image_id: &str) -> PathBuf {
    evidence_dir(case_dir, image_id).join(VOLUMES_DIR)
}

/// 卷目录 `evidence/<uid>/volumes/<卷名>`。
pub fn volume_dir(case_dir: &Path, image_id: &str, volume_key: &str) -> PathBuf {
    volumes_dir(case_dir, image_id).join(volume_segment(volume_key))
}

/// 一次构建的随机文件名令牌(8 位十六进制)。
pub fn new_token() -> String {
    let token = uuid::Uuid::new_v4().simple().to_string();
    token[..8].to_string()
}

pub fn snapshot_file_name(token: &str) -> String {
    format!("{SNAPSHOT_PREFIX}{token}.{SNAPSHOT_EXTENSION}")
}

pub fn index_file_name(token: &str) -> String {
    format!("{INDEX_PREFIX}{token}.{INDEX_EXTENSION}")
}

/// 删除一个文件及其 SQLite 边车(`-wal` / `-shm` / `-journal`)。不存在的文件
/// 忽略;返回第一个其它错误(Windows 上被打开的文件删不掉,由调用方决定是否
/// 留待下次清理)。
pub fn remove_database_files(path: &Path) -> std::io::Result<()> {
    let mut first_error = None;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(suffix);
        match std::fs::remove_file(PathBuf::from(candidate)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 删除目录树;不存在视为成功。
pub fn remove_dir_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paths_are_stable() {
        let case_dir = Path::new("/case");
        assert_eq!(ledger_path(case_dir), PathBuf::from("/case/case.db"));
        assert_eq!(lock_path(case_dir), PathBuf::from("/case/case.lock"));
        assert_eq!(
            evidence_db_path(case_dir, "img-1", EvidenceDb::Search),
            PathBuf::from("/case/evidence/img-1/search.db")
        );
        assert_eq!(
            volume_dir(case_dir, "img-1", "lvm:2:0"),
            PathBuf::from("/case/evidence/img-1/volumes/lvm-2-0")
        );
        assert_eq!(
            cache_dir(case_dir, "img-1"),
            PathBuf::from("/case/evidence/img-1/cache")
        );
    }

    #[test]
    fn segments_never_escape_their_directory() {
        assert_eq!(path_segment("../../etc", "x"), "_.._etc");
        assert_eq!(path_segment("..", "fallback"), "fallback");
        assert_eq!(path_segment("a/b\\c", "x"), "a_b_c");
        assert_eq!(volume_segment("root"), "root");
        assert_eq!(volume_segment("part:12"), "part-12");
    }

    #[test]
    fn long_volume_keys_are_bounded_and_distinct() {
        let first = format!("host-docker:{}:", "a".repeat(64));
        let second = format!("host-docker:{}:", "b".repeat(64));
        let first_segment = volume_segment(&first);
        assert_eq!(first_segment.len(), VOLUME_SEGMENT_MAX);
        assert_ne!(first_segment, volume_segment(&second));
        assert_eq!(first_segment, volume_segment(&first));
    }

    #[test]
    fn generated_file_names_carry_the_token() {
        let token = new_token();
        assert_eq!(token.len(), 8);
        assert_eq!(snapshot_file_name("ab12cd34"), "tree-ab12cd34.dirz");
        assert_eq!(index_file_name("ab12cd34"), "index-ab12cd34.db");
    }
}
