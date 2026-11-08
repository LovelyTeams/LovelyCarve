//! Thin dynamic FFI wrapper around `librefs-13.dll` (Windows / Linux).
//!
//! Loads `librefs-13.dll` on first use via `libloading` and exposes the
//! handful of `fsapi_*` / `sys_device_*` symbols needed to mount a ReFS
//! volume, list its directories, read file content, and pull node
//! attributes.
//!
//! The library does not accept a file path — it
//! takes a `sys_device` handle. The bundled DLL exports a callback
//! constructor `sys_device_open_callbacks` (added by the `feature/
//! build-librefs-dll` branch on the b3nguang/refsprogs fork) so we drive
//! the library from a Rust closure backed by `DiskImage::read_range`
//! instead of dumping the partition to a temp file. That is the same
//! pattern every other lovelycarve filesystem adapter uses.
//!
//! Thread-safety: librefs is single-threaded per volume; the caller is
//! expected to serialise concurrent operations on the same `RefsVolume`
//! (the `RefsVolume` Rust handle wraps each call in a `Mutex`).

mod abi;
use abi::*;
pub use abi::{
    FSAPI_ATTR_ALL, FSAPI_ATTR_ALLOCATED_SIZE, FSAPI_ATTR_CREATION_TIME, FSAPI_ATTR_INODE_NUMBER,
    FSAPI_ATTR_LAST_DATA_ACCESS_TIME, FSAPI_ATTR_LAST_DATA_CHANGE_TIME,
    FSAPI_ATTR_LAST_STATUS_CHANGE_TIME, FSAPI_ATTR_LINK_COUNT, FSAPI_ATTR_MODE, FSAPI_ATTR_SIZE,
};
mod library;
use library::*;
mod callbacks;
use callbacks::*;
mod streams;
mod volume;

#[cfg(test)]
mod tests;

use std::ffi::{c_int, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context};
use libloading::Library;

use super::common::dll_search_path;

/// User-provided I/O backing for a ReFS volume. Implementations must be
/// `Send + Sync` because `sys_device_pread` may be invoked from whichever
/// thread is currently driving the volume; lovelycarve serialises calls
/// through the `RefsVolume` mutex, but the trait object lifetime is bound
/// to `'static` so it can be boxed into librefs's `user_data` slot.
pub trait RefsBacking: Send + Sync {
    /// Read exactly `buf.len()` bytes starting at `offset` from the
    /// partition. Returns 0 on success or a non-zero errno-like code on
    /// failure. The volume length sits at `size()`; reads past the end
    /// MUST return non-zero.
    fn pread(&self, offset: u64, buf: &mut [u8]) -> Result<(), i32>;

    /// Total volume length in bytes (length of the ReFS partition slice).
    fn size(&self) -> u64;
}

/// One directory entry produced by `RefsVolume::list_directory`.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub inode: Option<u64>,
    pub creation_time: Option<SystemTimestamp>,
    pub last_data_change_time: Option<SystemTimestamp>,
    pub last_data_access_time: Option<SystemTimestamp>,
}

#[derive(Debug, Clone, Copy)]
pub struct SystemTimestamp {
    pub tv_sec: u64,
    pub tv_nsec: u32,
}

// ---------- Whole-tree walk callback ----------

/// One directory entry produced by [`RefsVolume::walk_tree`]. Mirrors
/// [`DirEntry`] but additionally carries the object IDs needed to rebuild the
/// tree in memory: `parent_object_id` (the directory the entry lives in) and,
/// for directories, `object_id` (the entry's own directory object ID, the key
/// its children are grouped under). For non-directories `object_id` is 0.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub parent_object_id: u64,
    pub object_id: u64,
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub inode: Option<u64>,
    pub creation_time: Option<SystemTimestamp>,
    pub last_data_change_time: Option<SystemTimestamp>,
    pub last_data_access_time: Option<SystemTimestamp>,
}

/// Object ID of the ReFS volume root directory (children of the root are
/// reported by [`RefsVolume::walk_tree`] with this `parent_object_id`).
pub const REFS_ROOT_OBJECT_ID: u64 = 0x600;

/// One `refsutil streamsnapshot` snapshot stream surfaced during a whole-tree
/// walk, attributed to its owning file by `owner_parent_object_id` +
/// `owner_name` (the directory the file lives in, plus the file's name — the
/// key needed to pair it with the matching [`WalkEntry`]). Only snapshot
/// streams are collected; ordinary ADS are filtered out by the C layer's
/// `is_snapshot` classification.
#[derive(Debug, Clone)]
pub struct WalkStream {
    pub owner_parent_object_id: u64,
    pub owner_name: String,
    pub stream_name: String,
    pub size: u64,
}

/// Result of a whole-tree walk: every directory entry plus every snapshot
/// stream found in the same single pass.
#[derive(Debug, Default)]
pub struct WalkResult {
    pub entries: Vec<WalkEntry>,
    pub streams: Vec<WalkStream>,
}

// ---------- Stream listing callback ----------

/// One named data stream attached to a file. Mirrors what
/// `fsapi_node_list_streams` reports per `node_stream` event — the
/// default `:$DATA` is excluded from this list (read it via
/// [`RefsVolume::read`]).
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// UTF-8 stream name (Windows displays it with a `:$SNAPSHOT` or
    /// `:$DATA` suffix, but refsprogs stores just the bare name).
    pub name: String,
    /// Total stream size in bytes.
    pub size: u64,
    /// `true` when the stream's bytes live inline in the file's
    /// metadata (very small streams). `false` indicates an extent-
    /// backed stream — the cost of reading it is one or more disk
    /// reads.
    pub resident: bool,
}

// ---------- Public Rust wrapper ----------

pub struct RefsVolume {
    inner: Mutex<RefsInner>,
    size_bytes: u64,
}

struct RefsInner {
    vol: VolumePtr,
    dev: DevicePtr,
    root: NodePtr,
}

unsafe impl Send for RefsVolume {}
unsafe impl Sync for RefsVolume {}

/// Snapshot of `fsapi_node_attributes` flattened to plain Rust types.
/// Only the fields lovelycarve consumes are exposed.
#[derive(Debug, Clone, Default)]
pub struct NodeAttrs {
    pub is_directory: bool,
    pub size: u64,
    pub allocated_size: u64,
    pub link_count: u64,
    pub inode_number: u64,
    pub mode: u32,
    pub creation_time: Option<SystemTimestamp>,
    pub last_status_change_time: Option<SystemTimestamp>,
    pub last_data_change_time: Option<SystemTimestamp>,
    pub last_data_access_time: Option<SystemTimestamp>,
}

impl NodeAttrs {
    fn from_raw(raw: &FsapiNodeAttributes) -> Self {
        let take_ts = |bit: c_int, ts: SysTimespec| -> Option<SystemTimestamp> {
            if raw.valid & bit != 0 {
                Some(SystemTimestamp {
                    tv_sec: ts.tv_sec,
                    tv_nsec: ts.tv_nsec,
                })
            } else {
                None
            }
        };
        Self {
            is_directory: raw.is_directory != 0,
            size: if raw.valid & FSAPI_ATTR_SIZE != 0 {
                raw.size
            } else {
                0
            },
            allocated_size: if raw.valid & FSAPI_ATTR_ALLOCATED_SIZE != 0 {
                raw.allocated_size
            } else {
                0
            },
            link_count: if raw.valid & FSAPI_ATTR_LINK_COUNT != 0 {
                raw.link_count
            } else {
                0
            },
            inode_number: if raw.valid & FSAPI_ATTR_INODE_NUMBER != 0 {
                raw.inode_number
            } else {
                0
            },
            mode: if raw.valid & FSAPI_ATTR_MODE != 0 {
                raw.mode
            } else {
                0
            },
            creation_time: take_ts(FSAPI_ATTR_CREATION_TIME, raw.creation_time),
            last_status_change_time: take_ts(
                FSAPI_ATTR_LAST_STATUS_CHANGE_TIME,
                raw.last_status_change_time,
            ),
            last_data_change_time: take_ts(
                FSAPI_ATTR_LAST_DATA_CHANGE_TIME,
                raw.last_data_change_time,
            ),
            last_data_access_time: take_ts(
                FSAPI_ATTR_LAST_DATA_ACCESS_TIME,
                raw.last_data_access_time,
            ),
        }
    }
}

impl Drop for RefsVolume {
    fn drop(&mut self) {
        let Ok(lib) = lib() else {
            return;
        };
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if !inner.vol.is_null() {
            let mut vol_slot = inner.vol;
            unsafe {
                let _ = (lib.fsapi_volume_unmount)(&mut vol_slot);
            }
            inner.vol = ptr::null_mut();
            inner.root = ptr::null_mut();
        }
        if !inner.dev.is_null() {
            let mut dev_slot = inner.dev;
            unsafe {
                let _ = (lib.sys_device_close)(&mut dev_slot);
            }
            inner.dev = ptr::null_mut();
        }
    }
}

/// Opaque handle to a node owned by librefs. The volume must outlive the
/// handle; releasing requires going back through the volume's mutex via
/// `RefsVolume::release_node`.
pub struct NodeHandle {
    ptr: NodePtr,
}

unsafe impl Send for NodeHandle {}

impl NodeHandle {
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}
