//! Plugin runtime.
//!
//! A plugin is a native library next to its `plugin.toml` that exports the
//! C ABI of `lovelycarve_plugin::abi`. With `isolation = "process"` (the
//! default) it runs inside `lc-plugin-host`, which LovelyCarve drives with
//! LPP v1 frames (length-prefixed MessagePack) over the child's stdin and
//! stdout; plugin stdout is protocol-only and stderr carries diagnostics.
//! With `isolation = "in_process"` LovelyCarve loads the library itself.
//! Both modes carry the same LPP methods and payloads.
//!
//! Each plugin has one shared instance for filesystem browsing and direct
//! analysis; every scheduled task runs on an instance of its own
//! ([`TaskWorker`]), so tasks of one plugin run side by side.

mod analysis;
mod analysis_result;
mod client;
mod codec;
mod connection;
mod discovery;
mod error;
mod evidence_directory_cache;
mod evidence_name_scan;
mod evidence_registry;
mod file_registry;
mod fingerprint;
mod in_process;
mod manager;
mod manifest;
mod process_guard;
mod remote_volume;
mod source_registry;
mod wire;

pub use analysis::{EvidenceAnalysisRequest, FileAnalysisRequest, TaskWorker};

pub use client::{PluginClient, PluginCommand, PluginIdentity, PluginProgressEvent};
pub use codec::MAX_FRAME_SIZE;
pub use connection::PluginConnection;
pub use discovery::{
    CatalogSnapshot, PluginScanEntry, PluginScanReport, PluginScanStatus, PLUGIN_CATALOG_CHANGED,
};
pub use error::{PluginError, Result};
pub use evidence_registry::{
    EvidenceHandle, EvidenceNode, EvidenceRegistry, EvidenceRoot, MAX_EVIDENCE_READ,
    MAX_EVIDENCE_READV_RANGES,
};
pub use file_registry::{FileHandle, FileRegistry, MaterializedFileLease};
pub use in_process::InProcessClient;
pub use lovelycarve_plugin::wire::{LppMessage, RpcError};
pub use manager::{PluginDefinition, PluginManager, PluginState, ProbeMatch};
pub use manifest::{
    EvidenceAnalyzerCapability, FileAnalyzerCapability, FilesystemCapability, LibraryManifest,
    PluginIsolation, PluginManifest, PluginPermission, TaskManifest, TaskTrigger,
};
pub(crate) use remote_volume::RemoteVolume;
pub use source_registry::{SourceHandle, SourceInfo, SourceRegistry};
