use crate::core::analysis::identify::systems::SystemCache;
use crate::core::preview::media_cache::{MediaScratch, MediaTempCache};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use crate::core::carving::carve_session::CarveScanSession;
use crate::core::casework::case::CaseState;
use crate::core::extensions::plugin::PluginManager;
use crate::core::extensions::task_scheduler::TaskScheduler;
use crate::core::filesystem::evidence_session::EvidenceSession;
#[cfg(target_os = "windows")]
use crate::core::platform::dokan_mount::{DokanMountSession, MountShareView};
use crate::task_manager::TaskManager;
use tokio::sync::Mutex;

/// Shared app state, handed to every backend command by the desktop app and
/// by the standalone CLI alike.
/// Wrapped in `Arc<Mutex<…>>` at the field level so individual sections can be
/// locked independently during long-running work.
///
/// Runtime handles and caches live here for exactly as long as the process
/// does; durable state (case manifest, filedb index, task store, search /
/// timeline results) is persisted by the case layer, not by this struct.
pub struct AppState {
    pub images: Arc<Mutex<HashMap<String, Arc<EvidenceSession>>>>,
    /// Case generation and publication gate, never held during decompression.
    pub image_load_context: Mutex<u64>,
    /// Per-path single-flight locks for `image_load`. The map keeps weak
    /// references so completed/abandoned paths do not accumulate, while every
    /// concurrent caller for the same evidence path serialises behind the
    /// first opener and then reuses the registered `EvidenceSession`.
    pub image_load_locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
    /// Discovered plugin packages. The setup hook installs the manager
    /// after Tauri resolves the per-user application-data directory.
    pub plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>>,
    /// Operating systems detected per loaded image (system-scope plugin
    /// runs and the analysis dialog). Purged per image on unload.
    pub systems: Arc<SystemCache>,
    /// Serialises lazy Unknown-partition probe/open so concurrent directory
    /// requests cannot create duplicate subprocess filesystem sessions.
    pub plugin_open_lock: Arc<Mutex<()>>,
    pub tasks: TaskManager,
    /// 插件任务调度器。案件打开时绑定台账 `case.db` 的任务表,关闭时解绑;
    /// 未绑定时所有控制面返回“无案件”。
    pub scheduler: TaskScheduler,
    /// Raw signature-carving sessions. Workers update one session through a
    /// short-lived parking_lot lock while IPC readers page stable results.
    pub carve_scans: Arc<Mutex<HashMap<String, Arc<parking_lot::RwLock<CarveScanSession>>>>>,
    /// Idempotency cache for `raid_auto_detect`: maps an assembled device's
    /// label (its `assembled://` path) → already-registered synthetic image
    /// id. Repeated detect calls reuse the existing assembled `EvidenceSession`
    /// instead of allocating a duplicate.
    pub assembled_arrays: Arc<Mutex<HashMap<String, String>>>,
    /// Live read-only Dokan projections keyed by mount id.
    #[cfg(target_os = "windows")]
    pub dokan_mounts: Arc<Mutex<HashMap<String, Arc<DokanMountSession>>>>,
    /// SMB shares published for live Dokan projections, keyed by mount id.
    #[cfg(target_os = "windows")]
    pub dokan_mount_shares: Arc<Mutex<HashMap<String, MountShareView>>>,
    /// The single active forensic case, if one is open: the case ledger
    /// connection, its cached evidence and volume records, and in-memory
    /// snapshot bodies used when a volume index is unavailable. `None` means
    /// "no case open" — in that state `image_load` keeps its lazy,
    /// in-memory-only behaviour.
    pub active_case: Arc<Mutex<Option<CaseState>>>,
    /// Image ids explicitly removed from the active case while a matching
    /// `image_load` may still be opening in a blocking worker. Late and queued
    /// loads keep seeing this tombstone until the case changes, so none can
    /// resurrect the evidence row the user just removed.
    pub removed_case_evidence: Arc<Mutex<HashSet<String>>>,
    /// Single-slot cache for the SQLite preview: the most recently opened
    /// database imported into the process-local preview VFS. Paging / sorting
    /// the same file reuses it instead of re-reading the bytes; switching to
    /// another database releases the cached session. In-flight SQL readers
    /// own an Arc, run outside this mutex, and release their VFS files on exit.
    pub sqlite_session:
        Arc<Mutex<Option<Arc<crate::core::preview::sqlite::sqlite_preview::SqliteSession>>>>,
    /// Single-slot cache of the most recently materialised inner ZIP's bytes,
    /// keyed by `"{image_id}\u{1f}{node_id}"`. The structure preview must read
    /// the whole archive (its central directory sits at the file end, and the
    /// file-node read primitives have no ranged read), so member-content clicks
    /// reuse these bytes instead of re-reading the file. Switching to another
    /// archive drops the previous buffer.
    pub zip_session: Arc<Mutex<Option<ZipBytesCache>>>,
    /// Single-slot cache for the Realm native preview: the most recently read
    /// `.realm` file bytes, keyed by `"{image_id}\u{1f}{node_id}"`. The vendored
    /// reader works from immutable in-memory bytes, so paging a table reuses
    /// this shared slice instead of re-reading the file or creating a temp copy.
    pub realm_session: Arc<Mutex<Option<RealmBytesCache>>>,
    /// Volume snapshots built on image load when NO case is open, keyed by
    /// `(image_id, volume_key)`. Process-local — the `.dirz` is built to a temp
    /// file, imported into a temp index file, then deleted — so caseless
    /// browsing / 全显 / child-counts are served from an index just like the
    /// case path, without ever creating a case directory. Purged per-image on
    /// unload; the case takes priority and supersedes these once a case is
    /// opened. Built and installed by `commands::casework::snapshot_session`.
    pub session_snapshots: SessionSnapshots,
    /// Per-session cache of media (video / audio) files materialised to a temp
    /// directory so the Tauri `asset` protocol — which serves real files with
    /// full HTTP Range / seek support — can stream them at any size. The
    /// in-memory Blob path (`fs_read` → object URL) is capped at 16 MiB and
    /// cannot seek, so large clips never previewed. Keyed by
    /// `"{image_id}\u{1f}{node_id}\u{1f}{ext}"`; a byte-bounded LRU whose
    /// viewer leases prevent eviction until release. The base directory is created and
    /// added to the asset-protocol scope by the `lib.rs` setup hook.
    pub media_temp: Arc<Mutex<MediaTempCache>>,
    pub media_scratch: parking_lot::Mutex<Option<Arc<MediaScratch>>>,
    /// Serialises materialisation and byte-budget admission across keys.
    pub media_writer: Arc<Mutex<()>>,
    /// 在跑的全局搜索任务登记（`state.global_searches`）：命令层任务列表 /
    /// 删除时判定运行态、孤儿 running 行修复的存活集。命中与终态的权威
    /// 存储在检材的 `search.db`（`core::search::global_search_store`）。
    pub global_searches: Arc<Mutex<HashMap<String, GlobalSearchRunState>>>,
    /// 在跑的时间线重建登记(`state.timeline_builds`,按检材 image id 一条):
    /// 时间线 running 判定 + 重复 start 拦截 + 孤儿 running 状态修复的存活集。
    /// 权威状态在检材的 `timeline.db`。
    pub timeline_builds: Arc<Mutex<HashMap<String, TimelineBuildRunState>>>,
    /// 文件全显(allowAllFiles)实时遍历结果的小缓存,见 [`FlatWalkCache`]。
    /// 同步锁:只在 `spawn_blocking` 线程里短暂持有。
    pub flat_walk_cache: Arc<parking_lot::Mutex<FlatWalkCache>>,
}

impl AppState {
    pub fn new() -> Self {
        let images: Arc<Mutex<HashMap<String, Arc<EvidenceSession>>>> = Arc::default();
        let plugins: Arc<parking_lot::RwLock<Option<Arc<PluginManager>>>> = Arc::default();
        let systems: Arc<SystemCache> = Arc::default();
        let scheduler = crate::app::setup::build_task_scheduler(
            Arc::clone(&images),
            Arc::clone(&plugins),
            Arc::clone(&systems),
        );
        Self {
            images,
            image_load_locks: Arc::default(),
            image_load_context: Mutex::new(0),
            plugins,
            systems,
            plugin_open_lock: Arc::default(),
            tasks: TaskManager::default(),
            scheduler,
            carve_scans: Arc::default(),
            assembled_arrays: Arc::default(),
            #[cfg(target_os = "windows")]
            dokan_mounts: Arc::default(),
            #[cfg(target_os = "windows")]
            dokan_mount_shares: Arc::default(),
            active_case: Arc::default(),
            removed_case_evidence: Arc::default(),
            sqlite_session: Arc::default(),
            zip_session: Arc::default(),
            realm_session: Arc::default(),
            session_snapshots: Arc::default(),
            media_temp: Arc::default(),
            media_scratch: parking_lot::Mutex::new(None),
            media_writer: Arc::default(),
            global_searches: Arc::default(),
            timeline_builds: Arc::default(),
            flat_walk_cache: Arc::default(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

mod preview;
mod runs;
mod snapshot;
mod walk;
pub use preview::{RealmBytesCache, ZipBytesCache};
pub use runs::{GlobalSearchRunState, TimelineBuildRunState};
pub use snapshot::{SessionIndex, SessionSnapshot, SessionSnapshots};
pub use walk::{FlatWalkCache, FlatWalkKind, FlatWalkRows};
