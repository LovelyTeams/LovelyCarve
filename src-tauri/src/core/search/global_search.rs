//! 全局搜索引擎:范围解析、流式扫描与命中落库。
//!
//! 一次全局搜索是一个后台任务：按范围（分区 / 标签 / 树节点）
//! 把可扫对象解析成「文件 + 可选未分配空间」清单，
//! 用 [`MatcherSet`]（关键词 + 正则 × 选定编码页）逐对象流式扫描，命中
//! 即时落库（[`global_search_store`]）供服务端分页；进度经回调上抛。
//!
//! - **范围**：文件来自台账登记的各卷当前索引（只读打开，发布后不再修改）；
//!   分区勾选按 volume key（`part:N` 连带其 `lvm:N:*` 逻辑卷）；标签勾选取
//!   `tag_store` 中命中标签的节点；树节点勾选把目录解析成所在卷上的
//!   `full_path_low` 半开前缀子树（与文件全显的 Subtree 作用域同法）。
//! - **已删除文件**：`search_deleted=false` 时剔除 `is_delete=1` 行
//!   （默认含已删除——取证语义）。
//! - **未分配空间**：`search_empty_space=true` 时把各分区 `$未分配簇`
//!   虚拟节点（`unalloc:`）与分区表间隙（`diskgap:`）当作可扫源，经
//!   `parse_virtual_unalloc_id` 直接从镜像字节流读取——物理搜索通道；
//!   标签 / 树节点范围下无意义，自动跳过。
//! - **宿主 Docker 文件**：容器文件在范围解析时一次性映射到底层卷上的文件
//!   节点（容器 overlay 在主镜像上只组合一次），worker 按底层节点读取；命中
//!   仍记在容器节点上。
//! - **CalcHash**：命中过的源在扫描收尾后多 worker 并行二次流式计算
//!   MD5+SHA256 回填命中行。
//! - **扫描底座**见 [`crate::core::search::global_search_scan`]：动态领取工作单元、
//!   可随机读的大源切段直读、命中上下文随扫随取、封装镜像（EWF/VMDK/VHD/
//!   VHDX/PMFX）每 worker 重开句柄、读错误逐源跳过不致命。

mod scope;
pub use scope::resolve_encodings;
use scope::*;
mod scan;
pub use scan::run_global_search;
use scan::*;
mod hash;
use hash::*;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;
use sha2::Digest as _;
use tokio_util::sync::CancellationToken;

use crate::core::casework::case_ledger_volumes::{indexed_volumes, IndexedVolume};
use crate::core::casework::tag_store;
use crate::core::evidence::image::ImageFormat;
use crate::core::filesystem::docker::host_docker;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::FsSession;
use crate::core::filesystem::tree::fs_tree_nodes::{disk_gap_nodes, fs_unalloc_nodes};
use crate::core::search::global_search_scan::{
    build_work_plan, scan_worker, ScanLayout, ScanPool, ScanSource, SourceOutcome,
};
use crate::core::search::global_search_store::{GlobalSearchHit, HitWriter};
use crate::core::search::keyword::keyword_search::{MatcherSet, SearchEncoding};
use crate::core::{catalog::filedb::filedb_file, catalog::filedb::filedb_query};

/// 落库批次大小（控制单事务与单次查询可见延迟）。
const HIT_FLUSH_EVERY: usize = 500;
/// 落库/进度节流。
const FLUSH_THROTTLE: Duration = Duration::from_millis(150);
/// Worker 上限。Kali 18 GiB VMDK（热页缓存）实测 8 与 16 相当、24 明显变慢——
/// 瓶颈已不在单线程；E01 等需要解压的来源更吃 CPU，取 16。
const MAX_SEARCH_WORKERS: usize = 16;
const DEFAULT_PER_FILE_CAP: u32 = 100;
const DEFAULT_GLOBAL_CAP: u32 = 200_000;
/// CalcHash 二次流式读取的分块。
const HASH_CHUNK_BYTES: usize = 4 * 1024 * 1024;
/// CalcHash 回填批次（单事务更新的源数）。
const HASH_WRITEBACK_BATCH: usize = 256;
/// 命中落库失败后还允许积压待重试的条数。失败的批次留在缓冲里随下一批整批
/// 重试；积压超过它说明落库持续失败，中止扫描、整次搜索报错。
const HIT_RETRY_BACKLOG: usize = 20 * HIT_FLUSH_EVERY;
/// CalcHash 回填失败后还允许积压待重试的源数（同上）。
const HASH_RETRY_BACKLOG: usize = 8 * HASH_WRITEBACK_BATCH;

fn default_true() -> bool {
    true
}

fn default_per_file_cap() -> u32 {
    DEFAULT_PER_FILE_CAP
}

fn default_global_cap() -> u32 {
    DEFAULT_GLOBAL_CAP
}

/// 搜索范围:分区、标签或树节点。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GlobalSearchScope {
    /// 卷键勾选（`part:N` 连带 `lvm:N:M`）；空 = 全镜像。
    #[serde(default)]
    pub partitions: Vec<String>,
    /// 标签值勾选（ LovelyCarve 标签是 node→tag 字符串映射）。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 树节点勾选（目录解析为子树，文件解析为单文件）。
    #[serde(default)]
    pub tree_nodes: Vec<String>,
}

/// 一次全局搜索的全部参数（wire 结构，snake_case）。
#[derive(Debug, Clone, Deserialize)]
pub struct GlobalSearchOptions {
    /// 关键词，逐行。
    #[serde(default)]
    pub keys: Vec<String>,
    /// 正则，逐行。
    #[serde(default)]
    pub regexes: Vec<String>,
    /// 编码页标签；空 = 全部。
    #[serde(default)]
    pub codepages: Vec<String>,
    /// 是否区分大小写。
    #[serde(default)]
    pub match_case: bool,
    /// 是否搜索已删除文件，默认开启。
    #[serde(default = "default_true")]
    pub search_deleted: bool,
    /// 是否搜索未分配空间。
    #[serde(default)]
    pub search_empty_space: bool,
    /// 是否为命中源回填 MD5/SHA256。
    #[serde(default)]
    pub calc_hash: bool,
    #[serde(default)]
    pub scope: GlobalSearchScope,
    #[serde(default = "default_per_file_cap")]
    pub per_file_cap: u32,
    /// 每源首命中即停。
    #[serde(default)]
    pub stop_after_first: bool,
    #[serde(default = "default_global_cap")]
    pub global_cap: u32,
}

/// 进度回调（已扫描/总数、累计命中、累计命中源）。`Sync` 供扫描工作池
/// 各线程共享引用。
pub type ProgressSink<'a> = &'a (dyn Fn(u64, u64, u64, u64) + Sync);

/// 引擎入参（模式和编码由命令层编译一次后传入）。
pub struct GlobalSearchParams<'a> {
    pub image: &'a Arc<EvidenceSession>,
    pub case_dir: &'a Path,
    pub image_id: &'a str,
    pub search_id: &'a str,
    pub options: &'a GlobalSearchOptions,
    /// 合并模式表：关键词在前、正则在后（与 `MatcherSet` 编号一致）。
    pub patterns: &'a [String],
    pub keyword_count: usize,
    pub matchers: &'a MatcherSet,
    pub cancel_flag: &'a CancellationToken,
    pub on_progress: ProgressSink<'a>,
}

/// 扫描结果摘要（wire 结构）。
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct GlobalSearchSummary {
    pub search_id: String,
    /// `success` | `cancelled` | `failed`。
    pub status: String,
    pub keyword_list: Vec<String>,
    pub regex_list: Vec<String>,
    pub codepages: Vec<String>,
    pub matched_file_count: u64,
    pub total_matches: u64,
    pub record_count: u64,
    pub elapsed_ms: u64,
    pub files_scanned: u64,
    pub files_total: u64,
    pub read_errors: u64,
    pub truncated: bool,
}

/// 树节点范围项解析出的子树:卷键 + `full_path_low` 半开区间
/// `[目录, 目录 + "/\u{10FFFF}")`。
struct SubtreeScope {
    volume_key: String,
    low: String,
    high: String,
}

#[cfg(test)]
mod tests;
