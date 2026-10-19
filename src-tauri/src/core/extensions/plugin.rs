//! Plugin runtime (LPP v2).
//!
//! A plugin is a package directory: `plugin.toml`, a native library that
//! exports the C ABI of `lovelycarve_plugin::abi`, the `schema.json` its
//! build exports and translations in `locales/`. The library runs inside
//! `lc-plugin-host`, which LovelyCarve drives with LPP v2 frames
//! (length-prefixed MessagePack) over the child's stdin and stdout; plugin
//! stdout is protocol-only and stderr carries diagnostics.
//!
//! Analyzers run on per-task workers ([`TaskWorker`]): the host opens the
//! run's view (one detected system, or every volume of the evidence),
//! resolves the analyzer's declared inputs, and streams the typed records
//! the plugin emits into the evidence's `records.db`
//! ([`crate::core::analysis::results::record_store`]). Packages with
//! filesystem parsers keep one shared instance for browsing (the frozen
//! `fs.*` / `source.*` methods).

mod client;
mod codec;
mod connection;
mod directory_cache;
mod error;
mod fingerprint;
mod log_file;
mod manager;
mod manifest;
mod package;
mod package_install;
mod process_guard;
mod remote_volume;
mod run;
mod run_context;
mod run_setup;
mod source_registry;
mod view;
mod wire;

pub use codec::MAX_FRAME_SIZE;
pub use connection::PluginConnection;
pub use error::{PluginError, Result};
pub use log_file::{PluginLogLevel, PluginLogStore};
pub use lovelycarve_plugin::wire::{LppMessage, RpcError};
pub use manager::{PluginManager, PluginState, ProbeMatch, RejectedPackage};
pub use manifest::{
    AnalyzerManifest, AnalyzerScope, FilesystemManifest, OptionChoice, OptionKind, OptionManifest,
    PluginManifest, PluginPermission, TaskTrigger, SYSTEM_FAMILIES,
};
pub use package::{Locales, PluginDefinition};
pub use package_install::{InstalledPackage, PACKAGE_EXTENSION};
pub(crate) use remote_volume::RemoteVolume;
pub use run::TaskWorker;
pub use run_context::{ProgressSink, RunContext};
pub use run_setup::{prepare_run, PreparedRun, RunPlan, RunTargetSpec};
pub use source_registry::{SourceHandle, SourceInfo, SourceRegistry};
pub use view::{FindScan, HostView, Located, LocatedFile, Resolved};
