//! Native callback dispatch and state ownership.
use super::*;

// ---------- Callback trampolines for sys_device_callbacks ----------

/// Heap-allocated state stashed in the C side's `user_data`. Owns the
/// trait object; dropped by `close_trampoline` when `sys_device_close`
/// fires from `Drop`.
pub(super) struct BackingState {
    pub(super) backing: Box<dyn RefsBacking>,
}

// ---------- Dir listing callback ----------

/// Internal context used by `RefsVolume::list_directory`. The library
/// invokes the dirent callback once per entry; we collect names into a
/// `Vec` so the Rust caller gets a flat list without holding the lock.
pub(super) struct ListCtx {
    pub(super) entries: Vec<DirEntry>,
    pub(super) capture_attrs: bool,
}

/// Collects every [`WalkEntry`] (and snapshot [`WalkStream`]) a whole-tree walk
/// emits. A plain struct keeps the trampolines `extern "C"`-simple and matches
/// the [`ListCtx`] pattern.
pub(super) struct WalkCtx {
    pub(super) entries: Vec<WalkEntry>,
    pub(super) streams: Vec<WalkStream>,
}

pub(super) struct StreamListCtx {
    pub(super) entries: Vec<StreamInfo>,
}

pub(super) unsafe extern "C" fn pread_trampoline(
    user_data: *mut c_void,
    offset: u64,
    size: usize,
    buf: *mut c_void,
) -> c_int {
    if user_data.is_null() || (size > 0 && buf.is_null()) {
        return 22; // EINVAL
    }
    let state = &*(user_data as *const BackingState);
    let slice = std::slice::from_raw_parts_mut(buf as *mut u8, size);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.backing.pread(offset, slice)
    })) {
        Ok(Ok(())) => 0,
        Ok(Err(errno)) => errno,
        Err(_) => 5, // EIO — Rust closure panicked; suppress + fail
    }
}

pub(super) unsafe extern "C" fn get_size_trampoline(
    user_data: *mut c_void,
    out_size: *mut u64,
) -> c_int {
    if user_data.is_null() || out_size.is_null() {
        return 22; // EINVAL
    }
    let state = &*(user_data as *const BackingState);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.backing.size())) {
        Ok(size) => {
            *out_size = size;
            0
        }
        Err(_) => 5,
    }
}

pub(super) unsafe extern "C" fn close_trampoline(user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    let _ = Box::from_raw(user_data as *mut BackingState);
}

pub(super) unsafe extern "C" fn list_dirent_trampoline(
    context: *mut c_void,
    name: *const u8,
    name_length: usize,
    attrs: *mut FsapiNodeAttributes,
) -> c_int {
    if context.is_null() || name.is_null() {
        return 22;
    }
    let ctx = &mut *(context as *mut ListCtx);
    let bytes = std::slice::from_raw_parts(name, name_length);
    // ReFS stores names as UTF-16 internally; fsapi exposes them as
    // UTF-8. Use lossy decoding so a corrupted name shows up instead of
    // aborting the whole listing.
    let name_str = String::from_utf8_lossy(bytes).into_owned();

    let (is_directory, size, inode, ct, mt, at) = if ctx.capture_attrs && !attrs.is_null() {
        let a = &*attrs;
        let inode = if a.valid & FSAPI_ATTR_INODE_NUMBER != 0 {
            Some(a.inode_number)
        } else {
            None
        };
        let size = if a.valid & FSAPI_ATTR_SIZE != 0 {
            a.size
        } else {
            0
        };
        let ct = if a.valid & FSAPI_ATTR_CREATION_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.creation_time.tv_sec,
                tv_nsec: a.creation_time.tv_nsec,
            })
        } else {
            None
        };
        let mt = if a.valid & FSAPI_ATTR_LAST_DATA_CHANGE_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.last_data_change_time.tv_sec,
                tv_nsec: a.last_data_change_time.tv_nsec,
            })
        } else {
            None
        };
        let at_ts = if a.valid & FSAPI_ATTR_LAST_DATA_ACCESS_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.last_data_access_time.tv_sec,
                tv_nsec: a.last_data_access_time.tv_nsec,
            })
        } else {
            None
        };
        (a.is_directory != 0, size, inode, ct, mt, at_ts)
    } else {
        (false, 0, None, None, None, None)
    };

    ctx.entries.push(DirEntry {
        name: name_str,
        is_directory,
        size,
        inode,
        creation_time: ct,
        last_data_change_time: mt,
        last_data_access_time: at,
    });
    0
}

pub(super) unsafe extern "C" fn walk_tree_trampoline(
    context: *mut c_void,
    parent_object_id: u64,
    object_id: u64,
    name: *const u8,
    name_length: usize,
    attrs: *const FsapiNodeAttributes,
) -> c_int {
    if context.is_null() || name.is_null() {
        return 22;
    }
    let ctx = &mut *(context as *mut WalkCtx);
    let bytes = std::slice::from_raw_parts(name, name_length);
    let name_str = String::from_utf8_lossy(bytes).into_owned();

    let (is_directory, size, inode, ct, mt, at) = if attrs.is_null() {
        (false, 0, None, None, None, None)
    } else {
        let a = &*attrs;
        let inode = if a.valid & FSAPI_ATTR_INODE_NUMBER != 0 {
            Some(a.inode_number)
        } else {
            None
        };
        let size = if a.valid & FSAPI_ATTR_SIZE != 0 {
            a.size
        } else {
            0
        };
        let ct = if a.valid & FSAPI_ATTR_CREATION_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.creation_time.tv_sec,
                tv_nsec: a.creation_time.tv_nsec,
            })
        } else {
            None
        };
        let mt = if a.valid & FSAPI_ATTR_LAST_DATA_CHANGE_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.last_data_change_time.tv_sec,
                tv_nsec: a.last_data_change_time.tv_nsec,
            })
        } else {
            None
        };
        let at_ts = if a.valid & FSAPI_ATTR_LAST_DATA_ACCESS_TIME != 0 {
            Some(SystemTimestamp {
                tv_sec: a.last_data_access_time.tv_sec,
                tv_nsec: a.last_data_access_time.tv_nsec,
            })
        } else {
            None
        };
        (a.is_directory != 0, size, inode, ct, mt, at_ts)
    };

    ctx.entries.push(WalkEntry {
        parent_object_id,
        object_id,
        name: name_str,
        is_directory,
        size,
        inode,
        creation_time: ct,
        last_data_change_time: mt,
        last_data_access_time: at,
    });
    0
}

/// Stream callback for [`RefsVolume::walk_tree`]. The C layer only invokes this
/// for streams it has classified as `refsutil streamsnapshot` snapshots
/// (`is_snapshot != 0`), so we record every event; ordinary ADS never reach
/// here. Names are lossily UTF-8-decoded like the other trampolines.
pub(super) unsafe extern "C" fn walk_stream_trampoline(
    context: *mut c_void,
    owner_parent_object_id: u64,
    owner_name: *const u8,
    owner_name_length: usize,
    stream_name: *const u8,
    stream_name_length: usize,
    stream_size: u64,
    is_snapshot: u8,
) -> c_int {
    if context.is_null() || owner_name.is_null() || stream_name.is_null() {
        return 22;
    }
    // Defensive: only snapshot streams should arrive, but never record an
    // ordinary ADS even if a future DLL widens what it reports.
    if is_snapshot == 0 {
        return 0;
    }
    let ctx = &mut *(context as *mut WalkCtx);
    let owner_bytes = std::slice::from_raw_parts(owner_name, owner_name_length);
    let stream_bytes = std::slice::from_raw_parts(stream_name, stream_name_length);
    ctx.streams.push(WalkStream {
        owner_parent_object_id,
        owner_name: String::from_utf8_lossy(owner_bytes).into_owned(),
        stream_name: String::from_utf8_lossy(stream_bytes).into_owned(),
        size: stream_size,
    });
    0
}

pub(super) unsafe extern "C" fn list_streams_trampoline(
    context: *mut c_void,
    name: *const u8,
    name_length: usize,
    data_size: u64,
    is_resident: u8,
) -> c_int {
    if context.is_null() {
        return 22;
    }
    let ctx = &mut *(context as *mut StreamListCtx);
    let name_str = if !name.is_null() && name_length > 0 {
        let bytes = std::slice::from_raw_parts(name, name_length);
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        String::new()
    };
    ctx.entries.push(StreamInfo {
        name: name_str,
        size: data_size,
        resident: is_resident != 0,
    });
    0
}
