//! Application operations shared by command adapters and lifecycle hooks.
pub mod casework;
pub(crate) mod docker;
pub mod evidence;
pub(crate) mod filesystem;
pub(crate) mod identify;
pub(crate) mod mount;
pub(crate) mod scheduler;
pub(crate) mod snapshot;
pub mod task_center;
