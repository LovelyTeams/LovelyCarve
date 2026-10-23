//! filesystem command adapters.

pub mod digests;
pub mod filedb;
#[expect(
    clippy::module_inception,
    reason = "Keep the existing CLI help group and command leaf name stable"
)]
pub mod filesystem;
pub mod filesystem_analysis;
pub(crate) mod filesystem_directory;
pub mod filesystem_docker;
pub mod filesystem_export;
pub mod filesystem_flat;
pub mod filesystem_journal;
pub mod filesystem_page;
pub mod filesystem_size_tree;
pub mod filesystem_snapshot_index;
pub mod filesystem_source;
pub mod filesystem_streams;
pub mod ftypes;
