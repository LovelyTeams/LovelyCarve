//! The C ABI: mount an image read-only, stat a path, list a directory,
//! read a file. Every entry point catches panics; a failure returns -1 (or
//! NULL) and leaves its reason in `fs_core_last_error_message()` and its
//! errno, the platform's number, in `fs_bcachefs_last_errno()`.
//! `include/fs_bcachefs.h` is the contract.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Arc;

use fs_core::ffi::{ffi_guard_or, set_last_error, FsCoreDevice};
use fs_core::{BlockDevice, FileDevice};

use crate::{Error, Filesystem};

thread_local! {
    /// The errno of this thread's most recent failure.
    static LAST_ERRNO: std::cell::Cell<c_int> = const { std::cell::Cell::new(0) };
}

/// The errno a missing extended attribute is: ENOATTR where the platform
/// has one (macOS), ENODATA where that is its name (Linux).
#[cfg(target_vendor = "apple")]
pub(crate) const ENOATTR: c_int = libc::ENOATTR;
#[cfg(not(target_vendor = "apple"))]
pub(crate) const ENOATTR: c_int = libc::ENODATA;

/// Record a failure: `msg` for `fs_bcachefs_last_error`, `errno` for
/// `fs_bcachefs_last_errno`.
pub(crate) fn fail_with(errno: c_int, msg: impl Into<String>) {
    set_last_error(msg);
    LAST_ERRNO.with(|e| e.set(errno));
}

/// The errno a reader's error is: ENOENT for a path or key that is not
/// there, ENOTSUP for something this driver does not read, EIO for the
/// rest (a device failure, a bad checksum, a corrupt structure).
pub(crate) fn read_errno(e: &Error) -> c_int {
    match e {
        Error::NotFound(_) => libc::ENOENT,
        Error::Unsupported(_) => libc::ENOTSUP,
        _ => libc::EIO,
    }
}

/// Record a reader's error.
fn fail(e: &Error) {
    fail_with(read_errno(e), e.to_string());
}

/// Record a device's error: the operating system's own errno where it
/// gave one (ENOENT for an image that is not there), EIO otherwise.
pub(crate) fn fail_device(e: &fs_core::Error) {
    let errno = match e {
        fs_core::Error::Io(io) => io.raw_os_error().unwrap_or(libc::EIO),
        _ => libc::EIO,
    };
    fail_with(errno, e.to_string());
}

/// `ffi_guard_or`, with the errno set to EIO before `f` runs: a failure
/// that names no errno of its own, a caught panic among them, reads as an
/// I/O error rather than as whatever an earlier call left.
pub(crate) fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    ffi_guard_or(default, || {
        LAST_ERRNO.with(|e| e.set(libc::EIO));
        f()
    })
}

/// The errno of this thread's most recent failed call, read or write, as
/// the platform numbers it: ENOENT, ENOTDIR, EINVAL, ENOTSUP, ENOSPC,
/// ENOTEMPTY, EEXIST, ERANGE, ENOATTR (ENODATA on Linux) or EIO. Only
/// meaningful after a call has failed.
#[no_mangle]
pub extern "C" fn fs_bcachefs_last_errno() -> c_int {
    LAST_ERRNO.with(|e| e.get())
}

/// An open, read-only filesystem.
pub struct fs_bcachefs_fs {
    /// The device is an fs_core trait object, which does not promise unwind
    /// safety; every entry point catches panics and leaves the handle
    /// usable, so the handle keeps promising it as it did with a concrete
    /// file device.
    fs: std::panic::AssertUnwindSafe<Filesystem<Arc<dyn BlockDevice>>>,
}

/// What `fs_bcachefs_stat` reports.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct fs_bcachefs_attr_t {
    pub ino: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub sectors: u64,
}

/// Called once per directory entry; return non-zero to stop.
pub type fs_bcachefs_dirent_cb = Option<
    unsafe extern "C" fn(
        ctx: *mut c_void,
        name: *const u8,
        name_len: usize,
        ino: u64,
        d_type: u8,
    ) -> c_int,
>;

unsafe fn cstr<'a>(p: *const c_char, what: &str) -> Option<&'a str> {
    if p.is_null() {
        fail_with(libc::EINVAL, format!("{what} is NULL"));
        return None;
    }
    match unsafe { CStr::from_ptr(p) }.to_str() {
        Ok(s) => Some(s),
        Err(_) => {
            fail_with(libc::EINVAL, format!("{what} is not UTF-8"));
            None
        }
    }
}

/// Open the image or device at `path` read-only. NULL on failure.
///
/// # Safety
/// `path` must be NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount(path: *const c_char) -> *mut fs_bcachefs_fs {
    guard(std::ptr::null_mut(), || {
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return std::ptr::null_mut();
        };
        let dev = match FileDevice::open(path) {
            Ok(d) => d,
            Err(e) => {
                fail_device(&e);
                return std::ptr::null_mut();
            }
        };
        match Filesystem::open(Arc::new(dev) as Arc<dyn BlockDevice>) {
            Ok(fs) => Box::into_raw(Box::new(fs_bcachefs_fs {
                fs: std::panic::AssertUnwindSafe(fs),
            })),
            Err(e) => {
                fail(&e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Close a handle from `fs_bcachefs_mount`. NULL is a no-op.
///
/// # Safety
/// `fs` must be NULL or a live handle, not used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_umount(fs: *mut fs_bcachefs_fs) {
    if !fs.is_null() {
        drop(unsafe { Box::from_raw(fs) });
    }
}

/// Stat an absolute path (a symlink is not followed). 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `out`
/// writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_stat(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    out: *mut fs_bcachefs_attr_t,
) -> c_int {
    guard(-1, || {
        if fs.is_null() || out.is_null() {
            fail_with(libc::EINVAL, "fs or out is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.inode(ino)) {
            Ok(i) => {
                unsafe {
                    *out = fs_bcachefs_attr_t {
                        ino: i.ino,
                        mode: i.mode,
                        nlink: i.link_count(),
                        uid: i.uid,
                        gid: i.gid,
                        size: i.size,
                        sectors: i.sectors,
                    }
                };
                0
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// Call `cb` for each entry of the directory at `path`. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `cb` safe
/// to call with `ctx`.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_readdir(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    cb: fs_bcachefs_dirent_cb,
    ctx: *mut c_void,
) -> c_int {
    guard(-1, || {
        let (false, Some(cb)) = (fs.is_null(), cb) else {
            fail_with(libc::EINVAL, "fs or cb is NULL");
            return -1;
        };
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.readdir(ino)) {
            Ok(entries) => {
                for d in entries {
                    if unsafe { cb(ctx, d.name.as_ptr(), d.name.len(), d.inum, d.d_type) } != 0 {
                        break;
                    }
                }
                0
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// Copy up to `length` bytes of the file at `path`, from `offset`, into
/// `buf`. Returns the number of bytes copied (0 at or past the end), or -1.
/// Only the extents covering the window are read, so reading a file in
/// pieces costs the pieces, not the file each time.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `length` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_read_file(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    offset: u64,
    length: u64,
) -> i64 {
    guard(-1, || {
        if fs.is_null() || buf.is_null() {
            fail_with(libc::EINVAL, "fs or buf is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        let len = usize::try_from(length).unwrap_or(usize::MAX);
        match fs
            .lookup(path)
            .and_then(|ino| fs.read_range(ino, offset, len))
        {
            Ok(data) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), buf.cast::<u8>(), data.len())
                };
                data.len() as i64
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// The C side's view of the last failure on this thread: the same message
/// as `fs_core_last_error_message()`. Never NULL.
#[no_mangle]
pub extern "C" fn fs_bcachefs_last_error() -> *const c_char {
    let p = fs_core::ffi::fs_core_last_error_message();
    if p.is_null() {
        c"".as_ptr()
    } else {
        p
    }
}

/// Mount through an fs_core device handle (a file, a slice, host callbacks:
/// `fs_core_device_from_callbacks`). The handle stays the caller's to
/// close; the filesystem holds its own reference to the device.
///
/// # Safety
/// `dev` must be NULL or a live `FsCoreDevice` handle.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount_with_fs_core_device(
    dev: *const FsCoreDevice,
) -> *mut fs_bcachefs_fs {
    guard(std::ptr::null_mut(), || {
        if dev.is_null() {
            fail_with(libc::EINVAL, "dev is NULL");
            return std::ptr::null_mut();
        }
        let inner = Arc::clone(unsafe { &*dev }.inner());
        match Filesystem::open(inner) {
            Ok(fs) => Box::into_raw(Box::new(fs_bcachefs_fs {
                fs: std::panic::AssertUnwindSafe(fs),
            })),
            Err(e) => {
                fail(&e);
                std::ptr::null_mut()
            }
        }
    })
}

/// What `fs_bcachefs_get_volume_info` reports.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct fs_bcachefs_volume_info_t {
    pub block_size: u32,
    pub btree_node_size: u32,
    pub nr_devices: u32,
    pub version_major: u16,
    pub version_minor: u16,
    /// The external (user-visible) UUID.
    pub uuid: [u8; 16],
    /// NUL-terminated.
    pub label: [c_char; 33],
    /// 1 when the filesystem was shut down cleanly, 0 when it was read
    /// through a replay of its journal.
    pub clean: u8,
}

impl Default for fs_bcachefs_volume_info_t {
    fn default() -> Self {
        fs_bcachefs_volume_info_t {
            block_size: 0,
            btree_node_size: 0,
            nr_devices: 0,
            version_major: 0,
            version_minor: 0,
            uuid: [0; 16],
            label: [0; 33],
            clean: 0,
        }
    }
}

/// Fill `out` with the volume's identity and geometry. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `out` writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_get_volume_info(
    fs: *mut fs_bcachefs_fs,
    out: *mut fs_bcachefs_volume_info_t,
) -> c_int {
    guard(-1, || {
        if fs.is_null() || out.is_null() {
            fail_with(libc::EINVAL, "fs or out is NULL");
            return -1;
        }
        let sb = unsafe { &*fs }.fs.superblock();
        let mut v = fs_bcachefs_volume_info_t {
            block_size: u32::from(sb.block_size) * 512,
            btree_node_size: sb.btree_node_size() * 512,
            nr_devices: u32::from(sb.nr_devices),
            version_major: sb.version.major(),
            version_minor: sb.version.minor(),
            uuid: sb.user_uuid,
            clean: u8::from(sb.is_clean()),
            ..Default::default()
        };
        for (d, s) in v.label.iter_mut().zip(sb.label_str().bytes().take(32)) {
            *d = s as c_char;
        }
        unsafe { *out = v };
        0
    })
}

/// What `fs_bcachefs_stat_times` reports: each time as seconds and
/// nanoseconds since the epoch.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct fs_bcachefs_times_t {
    pub atime_sec: i64,
    pub atime_nsec: u32,
    pub mtime_sec: i64,
    pub mtime_nsec: u32,
    pub ctime_sec: i64,
    pub ctime_nsec: u32,
}

/// An inode time as seconds and nanoseconds since the epoch: the stored
/// signed two's-complement value counts units of the superblock's time
/// precision (nanoseconds) from its time base (`time_base_lo`, nanoseconds
/// since the epoch):
/// docs/clean-room.md, "Creating a file".
fn epoch_time(sb: &crate::superblock::Superblock, raw: u64) -> (i64, u32) {
    let offset =
        i128::from(i64::from_le_bytes(raw.to_le_bytes())) * i128::from(sb.time_precision.max(1));
    let ns = i128::from(sb.time_base_lo) + offset;
    let sec = ns.div_euclid(1_000_000_000);
    let sec = i64::try_from(sec).unwrap_or(if sec < 0 { i64::MIN } else { i64::MAX });
    (sec, ns.rem_euclid(1_000_000_000) as u32)
}

/// Fill `out` with the access, modification and change times of the path
/// (a symlink is not followed). 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `out`
/// writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_stat_times(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    out: *mut fs_bcachefs_times_t,
) -> c_int {
    guard(-1, || {
        if fs.is_null() || out.is_null() {
            fail_with(libc::EINVAL, "fs or out is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.inode(ino)) {
            Ok(i) => {
                let sb = fs.superblock();
                let (atime_sec, atime_nsec) = epoch_time(sb, i.atime);
                let (mtime_sec, mtime_nsec) = epoch_time(sb, i.mtime);
                let (ctime_sec, ctime_nsec) = epoch_time(sb, i.ctime);
                unsafe {
                    *out = fs_bcachefs_times_t {
                        atime_sec,
                        atime_nsec,
                        mtime_sec,
                        mtime_nsec,
                        ctime_sec,
                        ctime_nsec,
                    }
                };
                0
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// What `fs_bcachefs_statfs` reports.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct fs_bcachefs_statfs_t {
    /// The filesystem's block size, in bytes.
    pub block_size: u32,
    /// The allocation unit, in bytes: space is taken a bucket at a time.
    pub bucket_size: u32,
    /// The device's buckets from its first usable one on, in bytes.
    pub total_bytes: u64,
    /// The buckets the freespace btree holds, in bytes.
    pub free_bytes: u64,
}

/// Fill `out` with the volume's capacity and free space. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `out` writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_statfs(
    fs: *mut fs_bcachefs_fs,
    out: *mut fs_bcachefs_statfs_t,
) -> c_int {
    guard(-1, || {
        if fs.is_null() || out.is_null() {
            fail_with(libc::EINVAL, "fs or out is NULL");
            return -1;
        }
        let fs = &unsafe { &*fs }.fs;
        match fs.usage() {
            Ok(u) => {
                unsafe {
                    *out = fs_bcachefs_statfs_t {
                        block_size: u32::from(fs.superblock().block_size) * 512,
                        bucket_size: u32::try_from(u.bucket_bytes).unwrap_or(u32::MAX),
                        total_bytes: u.total_bytes,
                        free_bytes: u.free_bytes,
                    }
                };
                0
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

fn attr_of(i: &crate::inode::Inode) -> fs_bcachefs_attr_t {
    fs_bcachefs_attr_t {
        ino: i.ino,
        mode: i.mode,
        nlink: i.link_count(),
        uid: i.uid,
        gid: i.gid,
        size: i.size,
        sectors: i.sectors,
    }
}

/// Stat an inode by number. 0 or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `out` writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_stat_ino(
    fs: *mut fs_bcachefs_fs,
    ino: u64,
    out: *mut fs_bcachefs_attr_t,
) -> c_int {
    guard(-1, || {
        if fs.is_null() || out.is_null() {
            fail_with(libc::EINVAL, "fs or out is NULL");
            return -1;
        }
        match unsafe { &*fs }.fs.inode(ino) {
            Ok(i) => {
                unsafe { *out = attr_of(&i) };
                0
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// One entry of a directory iterator.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct fs_bcachefs_dirent_t {
    pub ino: u64,
    /// DT_* numbering.
    pub d_type: u8,
    pub name_len: u16,
    /// NUL-terminated.
    pub name: [c_char; 256],
}

/// A directory being listed; each `fs_bcachefs_dir_next` returns the next
/// entry, valid until the next call or `fs_bcachefs_dir_close`.
pub struct fs_bcachefs_dir_iter {
    entries: Vec<crate::inode::Dirent>,
    next: usize,
    current: fs_bcachefs_dirent_t,
}

/// Open the directory at `path` for listing. NULL on failure.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_open(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
) -> *mut fs_bcachefs_dir_iter {
    guard(std::ptr::null_mut(), || {
        if fs.is_null() {
            fail_with(libc::EINVAL, "fs is NULL");
            return std::ptr::null_mut();
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return std::ptr::null_mut();
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.readdir(ino)) {
            Ok(entries) => Box::into_raw(Box::new(fs_bcachefs_dir_iter {
                entries: entries.to_vec(),
                next: 0,
                current: fs_bcachefs_dirent_t {
                    ino: 0,
                    d_type: 0,
                    name_len: 0,
                    name: [0; 256],
                },
            })),
            Err(e) => {
                fail(&e);
                std::ptr::null_mut()
            }
        }
    })
}

/// The next entry, or NULL at the end (or on a NULL iterator).
///
/// # Safety
/// `it` NULL or a live iterator.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_next(
    it: *mut fs_bcachefs_dir_iter,
) -> *const fs_bcachefs_dirent_t {
    guard(std::ptr::null(), || {
        if it.is_null() {
            return std::ptr::null();
        }
        let it = unsafe { &mut *it };
        let Some(d) = it.entries.get(it.next) else {
            return std::ptr::null();
        };
        it.next += 1;
        let n = d.name.len().min(255);
        it.current.ino = d.inum;
        it.current.d_type = d.d_type;
        it.current.name_len = n as u16;
        it.current.name = [0; 256];
        for (dst, &b) in it.current.name.iter_mut().zip(&d.name[..n]) {
            *dst = b as c_char;
        }
        &it.current
    })
}

/// Free an iterator. NULL is a no-op.
///
/// # Safety
/// `it` NULL or a live iterator, not used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_dir_close(it: *mut fs_bcachefs_dir_iter) {
    if !it.is_null() {
        drop(unsafe { Box::from_raw(it) });
    }
}

/// Copy up to `len` bytes of `src` into `buf`; `len` 0 asks for the size.
/// Returns the full size, or -1 when `buf` is too small.
unsafe fn copy_out(src: &[u8], buf: *mut c_void, len: u64) -> i64 {
    if len == 0 || buf.is_null() {
        return src.len() as i64;
    }
    if (len as usize) < src.len() {
        fail_with(
            libc::ERANGE,
            format!("the buffer holds {len} bytes; {} are needed", src.len()),
        );
        return -1;
    }
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buf.cast::<u8>(), src.len()) };
    src.len() as i64
}

/// A symlink's target into `buf` (not NUL-terminated). Returns its length,
/// or -1 (not a symlink, or `buf` too small).
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_readlink(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    guard(-1, || {
        if fs.is_null() {
            fail_with(libc::EINVAL, "fs is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        let target = fs.lookup(path).and_then(|ino| {
            let i = fs.inode(ino)?;
            if !i.is_symlink() {
                return Ok(None);
            }
            // A target is a path, at most a page; a larger size is damage,
            // and reading it whole would size a buffer from it (#149).
            if i.size > 4096 {
                return Err(crate::Error::Corrupt(format!(
                    "{path}: a symlink target of {} bytes",
                    i.size
                )));
            }
            fs.read(ino).map(Some)
        });
        match target {
            Ok(Some(t)) => unsafe { copy_out(&t, buf, len) },
            // readlink(2)'s answer for a path that is not a symlink.
            Ok(None) => {
                fail_with(libc::EINVAL, format!("{path} is not a symlink"));
                -1
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// The names of a path's extended attributes, each NUL-terminated, into
/// `buf`, as listxattr(2) lays them out. `len` 0 asks for the size.
/// Returns the size, or -1.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `buf`
/// writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_listxattr(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    guard(-1, || {
        if fs.is_null() {
            fail_with(libc::EINVAL, "fs is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.xattrs(ino)) {
            Ok(list) => {
                let mut names = Vec::new();
                for x in list {
                    names.extend_from_slice(&x.name);
                    names.push(0);
                }
                unsafe { copy_out(&names, buf, len) }
            }
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}

/// The value of the extended attribute `name` (with its namespace prefix,
/// `user.x`) of a path, into `buf`. `len` 0 asks for the size. Returns the
/// size, or -1 (no such attribute, or `buf` too small).
///
/// # Safety
/// `fs` a live handle or NULL; `path` and `name` NUL-terminated or NULL;
/// `buf` writable for `len` bytes or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_getxattr(
    fs: *mut fs_bcachefs_fs,
    path: *const c_char,
    name: *const c_char,
    buf: *mut c_void,
    len: u64,
) -> i64 {
    guard(-1, || {
        if fs.is_null() || name.is_null() {
            fail_with(libc::EINVAL, "fs or name is NULL");
            return -1;
        }
        let Some(path) = (unsafe { cstr(path, "path") }) else {
            return -1;
        };
        let want = unsafe { CStr::from_ptr(name) }.to_bytes();
        let fs = &unsafe { &*fs }.fs;
        match fs.lookup(path).and_then(|ino| fs.xattrs(ino)) {
            Ok(list) => match list.iter().find(|x| x.name == want) {
                Some(x) => unsafe { copy_out(&x.value, buf, len) },
                None => {
                    fail_with(
                        ENOATTR,
                        format!("{path} has no attribute {}", String::from_utf8_lossy(want)),
                    );
                    -1
                }
            },
            Err(e) => {
                fail(&e);
                -1
            }
        }
    })
}
