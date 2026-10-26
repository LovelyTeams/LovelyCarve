//! The C ABI's writes (#103), behind the `write` feature: open an image
//! read-write, then create, remove, rename, link, change attributes and
//! write files by absolute path. `include/fs_bcachefs.h` is the contract.
//!
//! WRITING IN PLACE. Every call is one transaction committed in place
//! (`Writer::open`), as `fs.bcachefs` does without `--journal`: when a call
//! returns 0 the image on disk holds its result, and `fs_bcachefs_rw_close`
//! has nothing left to write. A filesystem that was not cleanly unmounted
//! is refused at open.
//!
//! A handle writes to an image or device named by path
//! (`fs_bcachefs_mount_rw`) or to any fs_core device
//! (`fs_bcachefs_mount_rw_with_fs_core_device`): a file, a slice, or host
//! callbacks, for a caller that reaches its device no other way.
//!
//! Every entry point catches panics and returns 0 on success or a negative
//! errno, the platform's number, with the reason in
//! `fs_bcachefs_last_error()` and the errno again in
//! `fs_bcachefs_last_errno()`: -ENOENT for a path or name that is not
//! there, -ENOTSUP for what the writer does not do, -ENOSPC for no free
//! space, -ENOTEMPTY for a directory that is not empty, -EEXIST for a name
//! already taken, -EINVAL for a NULL or malformed argument, -EIO for
//! anything else.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Arc;

use fs_core::ffi::FsCoreDevice;
use fs_core::{BlockDevice, FileDevice};

use crate::capi::{fail_device, fail_with, guard};
use crate::write::Writer;
use crate::{Error, Filesystem};

use libc::{EEXIST, EINVAL, EIO, ENOENT, ENOSPC, ENOTEMPTY, ENOTSUP};

/// An image open for writing.
pub struct fs_bcachefs_rw {
    /// The device the writer writes to, which each lookup reads afresh.
    /// An fs_core trait object does not promise unwind safety; every entry
    /// point catches panics and leaves the handle usable, so the handle
    /// keeps promising it, as it did over a concrete file device.
    dev: std::panic::AssertUnwindSafe<Arc<dyn BlockDevice>>,
    w: std::panic::AssertUnwindSafe<Writer<Arc<dyn BlockDevice>>>,
}

/// The negative errno for a writer's error, its message kept as the last
/// error.
fn fail(e: Error) -> c_int {
    let errno = match &e {
        Error::NotFound(_) => ENOENT,
        Error::Unsupported(m) if m.contains("free bucket") => ENOSPC,
        Error::Unsupported(_) => ENOTSUP,
        Error::Corrupt(m) if m.contains("not empty") => ENOTEMPTY,
        Error::Corrupt(m) if m.contains("exists") || m.contains("already") => EEXIST,
        _ => EIO,
    };
    fail_with(errno, e.to_string());
    -errno
}

unsafe fn text<'a>(p: *const c_char, what: &str) -> Result<&'a str, c_int> {
    if p.is_null() {
        fail_with(EINVAL, format!("{what} is NULL"));
        return Err(-EINVAL);
    }
    unsafe { CStr::from_ptr(p) }.to_str().map_err(|_| {
        fail_with(EINVAL, format!("{what} is not UTF-8"));
        -EINVAL
    })
}

unsafe fn bytes<'a>(p: *const c_void, len: u64) -> Result<&'a [u8], c_int> {
    if len == 0 {
        return Ok(&[]);
    }
    if p.is_null() {
        fail_with(EINVAL, "data is NULL");
        return Err(-EINVAL);
    }
    Ok(unsafe { std::slice::from_raw_parts(p as *const u8, len as usize) })
}

impl fs_bcachefs_rw {
    /// The inode at an absolute path, as the device holds it now.
    fn lookup(&self, path: &str) -> Result<u64, c_int> {
        Filesystem::open(Arc::clone(&*self.dev))
            .and_then(|fs| fs.lookup(path))
            .map_err(fail)
    }

    /// The parent directory's inode and the last component of `path`.
    fn parent<'p>(&self, path: &'p str) -> Result<(u64, &'p [u8]), c_int> {
        let trimmed = path.trim_end_matches('/');
        let Some((dir, name)) = trimmed.rsplit_once('/') else {
            fail_with(EINVAL, format!("{path} is not an absolute path"));
            return Err(-EINVAL);
        };
        if name.is_empty() {
            fail_with(EINVAL, format!("{path} names no entry"));
            return Err(-EINVAL);
        }
        let dir = if dir.is_empty() { "/" } else { dir };
        Ok((self.lookup(dir)?, name.as_bytes()))
    }
}

/// Run `f` on a live handle; 0 or a negative errno.
unsafe fn with_rw(
    fs: *mut fs_bcachefs_rw,
    f: impl FnOnce(&mut fs_bcachefs_rw) -> Result<(), c_int>,
) -> c_int {
    guard(-EIO, || {
        if fs.is_null() {
            fail_with(EINVAL, "fs is NULL");
            return -EINVAL;
        }
        match f(unsafe { &mut *fs }) {
            Ok(()) => 0,
            Err(e) => e,
        }
    })
}

/// A write handle on `dev`, or NULL with the reason recorded. A device
/// that cannot be written is refused here, with EROFS, rather than at the
/// first write.
fn open_rw(dev: Arc<dyn BlockDevice>) -> *mut fs_bcachefs_rw {
    if !dev.is_writable() {
        fail_with(libc::EROFS, "the device is read-only");
        return std::ptr::null_mut();
    }
    match Writer::open(Arc::clone(&dev)) {
        Ok(w) => Box::into_raw(Box::new(fs_bcachefs_rw {
            dev: std::panic::AssertUnwindSafe(dev),
            w: std::panic::AssertUnwindSafe(w),
        })),
        Err(e) => {
            fail(e);
            std::ptr::null_mut()
        }
    }
}

/// Open the image or device at `path` for writing. NULL on failure.
///
/// # Safety
/// `path` must be NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount_rw(path: *const c_char) -> *mut fs_bcachefs_rw {
    guard(std::ptr::null_mut(), || {
        let Ok(path) = (unsafe { text(path, "path") }) else {
            return std::ptr::null_mut();
        };
        match FileDevice::open_rw(path) {
            Ok(d) => open_rw(Arc::new(d)),
            Err(e) => {
                fail_device(&e);
                std::ptr::null_mut()
            }
        }
    })
}

/// Open a handle for writing through an fs_core device handle (a file, a
/// slice, host callbacks: `fs_core_device_from_callbacks` with a write
/// callback). The handle stays the caller's to close; the writer holds its
/// own reference to the device. NULL on failure.
///
/// # Safety
/// `dev` must be NULL or a live `FsCoreDevice` handle.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mount_rw_with_fs_core_device(
    dev: *const FsCoreDevice,
) -> *mut fs_bcachefs_rw {
    guard(std::ptr::null_mut(), || {
        if dev.is_null() {
            fail_with(EINVAL, "dev is NULL");
            return std::ptr::null_mut();
        }
        open_rw(Arc::clone(unsafe { &*dev }.inner()))
    })
}

/// Close a handle from `fs_bcachefs_mount_rw`. NULL is a no-op. Every
/// successful call has already been written.
///
/// # Safety
/// `fs` must be NULL or a live handle, not used again afterwards.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_rw_close(fs: *mut fs_bcachefs_rw) {
    if !fs.is_null() {
        drop(unsafe { Box::from_raw(fs) });
    }
}

/// Create the file `path` holding `len` bytes of `data`, with permissions
/// `mode`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `data`
/// readable for `len` bytes, or NULL when `len` is 0.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_create(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    data: *const c_void,
    len: u64,
    mode: u32,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let data = bytes(data, len)?;
            let (dir, name) = fs.parent(path)?;
            fs.w.create_file(dir, name, data, mode)
                .map(drop)
                .map_err(fail)
        })
    }
}

/// Make the directory `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_mkdir(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    mode: u32,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let (dir, name) = fs.parent(path)?;
            fs.w.mkdir(dir, name, mode).map(drop).map_err(fail)
        })
    }
}

/// Remove the file or symlink `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_unlink(fs: *mut fs_bcachefs_rw, path: *const c_char) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let (dir, name) = fs.parent(path)?;
            fs.w.unlink(dir, name).map_err(fail)
        })
    }
}

/// Remove the empty directory `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_rmdir(fs: *mut fs_bcachefs_rw, path: *const c_char) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let (dir, name) = fs.parent(path)?;
            fs.w.rmdir(dir, name).map_err(fail)
        })
    }
}

/// Move `from` to `to`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `from` and `to` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_rename(
    fs: *mut fs_bcachefs_rw,
    from: *const c_char,
    to: *const c_char,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let from = text(from, "from")?;
            let to = text(to, "to")?;
            let (src, name) = fs.parent(from)?;
            let (dst, to_name) = fs.parent(to)?;
            fs.w.rename(src, name, dst, to_name).map_err(fail)
        })
    }
}

/// Make the symlink `path` pointing at `target`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` and `target` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_symlink(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    target: *const c_char,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let target = text(target, "target")?;
            let (dir, name) = fs.parent(path)?;
            fs.w.symlink(dir, name, target.as_bytes())
                .map(drop)
                .map_err(fail)
        })
    }
}

/// Give the file at `existing` a second name, `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `existing` and `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_link(
    fs: *mut fs_bcachefs_rw,
    existing: *const c_char,
    path: *const c_char,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let existing = text(existing, "existing")?;
            let path = text(path, "path")?;
            let ino = fs.lookup(existing)?;
            let (dir, name) = fs.parent(path)?;
            fs.w.link(ino, dir, name).map_err(fail)
        })
    }
}

/// Replace the whole contents of the file `path` with `len` bytes of
/// `data`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL; `data`
/// readable for `len` bytes, or NULL when `len` is 0.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_write_file(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    data: *const c_void,
    len: u64,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let data = bytes(data, len)?;
            let ino = fs.lookup(path)?;
            fs.w.write_file(ino, data).map_err(fail)
        })
    }
}

/// Set the permission bits of `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_chmod(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    mode: u32,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let ino = fs.lookup(path)?;
            fs.w.set_attributes(ino, Some(mode), None, None)
                .map_err(fail)
        })
    }
}

/// Set the owner and group of `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_chown(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    uid: u32,
    gid: u32,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let ino = fs.lookup(path)?;
            fs.w.set_attributes(ino, None, Some(uid), Some(gid))
                .map_err(fail)
        })
    }
}

/// Set the extended attribute `name` ("user.x") of `path` to `len` bytes of
/// `value`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` and `name` NUL-terminated or NULL;
/// `value` readable for `len` bytes, or NULL when `len` is 0.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_setxattr(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    name: *const c_char,
    value: *const c_void,
    len: u64,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let name = text(name, "name")?;
            let value = bytes(value, len)?;
            let ino = fs.lookup(path)?;
            fs.w.set_xattr(ino, name.as_bytes(), value).map_err(fail)
        })
    }
}

/// Remove the extended attribute `name` of `path`. 0 or a negative errno.
///
/// # Safety
/// `fs` a live handle or NULL; `path` and `name` NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn fs_bcachefs_removexattr(
    fs: *mut fs_bcachefs_rw,
    path: *const c_char,
    name: *const c_char,
) -> c_int {
    unsafe {
        with_rw(fs, |fs| {
            let path = text(path, "path")?;
            let name = text(name, "name")?;
            let ino = fs.lookup(path)?;
            fs.w.remove_xattr(ino, name.as_bytes()).map_err(fail)
        })
    }
}

#[cfg(test)]
mod tests {
    /// The handle promises what it promised over a concrete file device:
    /// a C caller's handle may cross a catch_unwind, and threads.
    #[test]
    fn the_write_handle_keeps_its_auto_traits() {
        fn unwind_safe<T: std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}
        fn send_sync<T: Send + Sync>() {}
        unwind_safe::<super::fs_bcachefs_rw>();
        send_sync::<super::fs_bcachefs_rw>();
    }
}
