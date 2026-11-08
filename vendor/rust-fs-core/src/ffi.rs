//! C ABI for the block-device framework.
//!
//! Every sister crate (qcow2 reader, partition probe, fs-* drivers) speaks
//! through the [`FsCoreDevice`] handle defined here, so consumers (Swift
//! FSKit modules, Go callers, C programs) only learn one device-handle
//! type and one error convention.
//!
//! ## Conventions
//!
//! - Handles are opaque `*mut FsCoreDevice`. Allocate via a constructor in
//!   one of the sister crates (e.g. `qcow2_open` from rust-img-qcow2),
//!   free via [`fs_core_device_close`] regardless of which crate created
//!   it.
//! - Error reporting is errno-style: every fallible function returns an
//!   [`FsCoreErrorCode`] (0 = OK, non-zero = failure) and stashes a human
//!   message in a thread-local. Read it via
//!   [`fs_core_last_error_message`].
//! - Every entry point catches Rust panics with `catch_unwind` and maps
//!   them to [`FsCoreErrorCode::Panic`]. Crossing an FFI boundary while
//!   unwinding is UB; the catch-net is non-negotiable.
//! - Thread safety: handles wrap `Arc<dyn BlockDevice>`, which is
//!   `Send + Sync` by trait bound. Multiple threads can call read/write
//!   concurrently as long as the underlying device's locking permits it.

#![allow(clippy::missing_safety_doc)]

use crate::block::{BlockDevice, BlockRead};
use crate::callback_device::CallbackDevice;
use crate::error::Error;
use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::io;
use std::panic::AssertUnwindSafe;
use std::ptr;
use std::slice;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Error codes — kept dense and stable so consumers can hard-code them.
// ---------------------------------------------------------------------------

/// Numeric error codes mirrored across every sister crate's C ABI.
///
/// `#[repr(i32)]` so the layout is identical to the matching C `enum`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsCoreErrorCode {
    /// Success.
    Ok = 0,
    /// Underlying I/O failed.
    Io = 1,
    /// A read the source could not satisfy in full — it ran out of data.
    /// What a file-backed handle returns for a read off the end of the
    /// file, and what a slice returns for a read past its own end.
    ShortRead = 2,
    /// Write attempted on a read-only device.
    ReadOnly = 3,
    /// A request refused up front because its range lies outside the
    /// device's declared size; nothing was transferred.
    ///
    /// This crate returns it for a **write** past the end of an RW
    /// slice, and for a write past the end of a file-backed device --
    /// both refused up front, nothing transferred. It reaches **reads**
    /// from sister crates whose container
    /// declares a virtual size (the `img-*` readers), and from this
    /// crate's caching / read-only / slice wrappers when they forward
    /// such a parent's error. A C consumer that only wants to know "the
    /// read overran the device", and does not control which crate opened
    /// the handle, should accept this and `FS_CORE_SHORT_READ` alike —
    /// and should not treat the pair as exhaustive, since a
    /// callback-backed handle reports its host's refusal as
    /// `FS_CORE_IO`.
    OutOfBounds = 4,
    /// Driver-specific error — message in the thread-local last-error.
    Custom = 5,
    /// One of the input pointers was null.
    NullArg = 6,
    /// `catch_unwind` caught a panic crossing the FFI boundary.
    Panic = 7,
    /// Reserved. Never returned.
    ///
    /// It was meant for a path that is not valid UTF-8, but the one
    /// function that meets that case — `fs_core_file_open` — returns a
    /// POINTER, not a code, so it reports the failure as NULL plus a
    /// message and cannot return this. No other entry point takes a
    /// path.
    ///
    /// Kept rather than removed because the numbering is published in
    /// `include/fs_core.h` and a consumer may already switch on 8;
    /// renumbering the codes after it would be an ABI break for a
    /// tidiness gain. A future path-taking function that returns a code
    /// should use this rather than invent another.
    BadString = 8,
}

impl FsCoreErrorCode {
    fn from_error(e: &Error) -> Self {
        match e {
            Error::Io(_) => FsCoreErrorCode::Io,
            Error::ShortRead { .. } => FsCoreErrorCode::ShortRead,
            Error::ReadOnly => FsCoreErrorCode::ReadOnly,
            Error::OutOfBounds { .. } => FsCoreErrorCode::OutOfBounds,
            Error::Custom(_) => FsCoreErrorCode::Custom,
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-local last-error — errno-style detail companion.
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Stash a message in the thread-local, replacing any previous one. Public
/// to sister crates so they can populate it for their own error paths.
pub fn set_last_error(message: impl Into<String>) {
    let s = message.into();
    let cs = CString::new(s.replace('\0', "?")).expect("contains no NUL after replace");
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(cs);
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Return a pointer to the calling thread's most recent error message, or
/// NULL if there is none. The pointer is owned by the framework and remains
/// valid until the next FFI call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn fs_core_last_error_message() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|cs| cs.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// Helper for sister crates: run `body`, catch panics, map errors to codes,
/// stash the message in the thread-local. Returns the error code.
pub fn ffi_guard<F>(body: F) -> FsCoreErrorCode
where
    F: FnOnce() -> Result<(), Error>,
{
    clear_last_error();
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => FsCoreErrorCode::Ok,
        Ok(Err(e)) => {
            let code = FsCoreErrorCode::from_error(&e);
            set_last_error(e.to_string());
            code
        }
        Err(panic) => {
            set_last_error(panic_message(&panic));
            FsCoreErrorCode::Panic
        }
    }
}

/// Run `body`, catching a panic and returning `fail` instead — and
/// recording the panic's message where a caller can read it.
///
/// # Why the message matters more than the fallback
///
/// Every fallback value here is also a legitimate answer. Zero is what
/// an empty device reports for its size; `false` is what a read-only
/// device reports for writability; a null pointer is what a failed open
/// returns. So a caller that only sees the fallback cannot tell an
/// ordinary answer from a driver that exploded computing it.
///
/// [`fs_core_last_error_message`] is what separates them, and a guard
/// that returns the fallback without setting it throws away the only
/// evidence there was.
///
/// # Why this is separate from [`ffi_guard`]
///
/// `ffi_guard` returns an [`FsCoreErrorCode`] and takes a body that
/// returns `Result<(), Error>`. That fits an entry point whose whole
/// answer is a status code, and fits nothing else — which is why the
/// eight entry points in this file that return a size, a flag or a
/// pointer each wrote `catch_unwind(AssertUnwindSafe(…)).unwrap_or(…)`
/// by hand instead, sixty lines below the helper.
///
/// Sister crates did the same: eleven of them re-roll one of these two
/// shapes rather than share either.
///
/// The error slot is cleared on entry, like [`ffi_guard`]: a call that
/// succeeds must not leave the previous call's message in place for a
/// caller to read and attribute to this one.
///
/// `AssertUnwindSafe` is used deliberately. The bodies here touch a
/// handle the caller owns and a thread-local error slot; a panic can
/// leave neither in a state another call can observe as inconsistent,
/// because the handle is not read again on this path and the slot is
/// overwritten whole.
pub fn ffi_guard_or<T, F>(fail: T, body: F) -> T
where
    F: FnOnce() -> T,
{
    clear_last_error();
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(panic) => {
            set_last_error(panic_message(&panic));
            fail
        }
    }
}

/// Run a cleanup body — a `close`, a `free` — catching a panic so it
/// cannot unwind into C, and **leaving the error slot untouched**.
///
/// # Why a third guard rather than one of the two above
///
/// The other two own the slot, because they have something to say
/// through it: a status code to explain, or a fallback value that needs
/// separating from a legitimate answer. A cleanup function that returns
/// `void` has neither. Running it through [`ffi_guard_or`] therefore
/// cleared a slot it could never fill, and destroyed the diagnostic in
/// the ordinary C shape where the free comes before the log — see
/// [`fs_core_device_close`].
///
/// So the rule this restores is that the slot's lifecycle belongs to the
/// call that can report through it, rather than to whichever helper
/// happened to wrap the body.
///
/// # A panic here is caught and NOT reported, deliberately
///
/// There is nowhere to report it. The return type is `()`, so a caller
/// learns nothing from the call itself, and the slot is the one thing it
/// is about to read for the *earlier* failure that sent it down the
/// cleanup path. Overwriting that with a message about the free would
/// destroy the very diagnostic this exists to preserve, and it is the
/// earlier error a caller is looking for. Swallowing the panic is the
/// lesser loss of the two, and it is a choice rather than an oversight.
///
/// `AssertUnwindSafe` for the same reason as [`ffi_guard_or`]: the body
/// touches a handle the caller owns and is not read again on this path.
pub fn ffi_guard_cleanup<F>(body: F)
where
    F: FnOnce(),
{
    let _ = std::panic::catch_unwind(AssertUnwindSafe(body));
}

/// What a caught panic actually said.
///
/// PUBLIC BECAUSE THE OTHER ELEVEN CRATES NEED IT. Each of them guards
/// its own C entry points with `catch_unwind` and, having no way to
/// reach this, reports the panic as `"panic in <function>"` -- the name
/// of the function that was running, which the caller already knew, in
/// place of the message, which is the only part it did not. An index
/// out of bounds, a slice out of range, an `expect` with a sentence in
/// it: all of it was thrown away at the boundary.
///
/// The guards themselves are NOT shareable, and that is why this is
/// what moved rather than [`ffi_guard`]. Each crate's guard records the
/// message into that crate's own thread-local, which is what its own C
/// callers read; a guard from here would record into this crate's, and
/// every panic message would land in a slot nobody reads.
pub fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = panic.downcast_ref::<String>() {
        return s.clone();
    }
    "panic in FFI".to_string()
}

// ---------------------------------------------------------------------------
// Device handle — opaque to C callers, shared across crates.
// ---------------------------------------------------------------------------

/// Opaque handle wrapping an `Arc<dyn BlockDevice>`. Allocated by sister
/// crates' constructors and freed via [`fs_core_device_close`].
pub struct FsCoreDevice {
    inner: Arc<dyn BlockDevice>,
}

impl FsCoreDevice {
    /// Internal constructor — sister crates use this to wrap their own
    /// device types (Qcow2Reader, FileDevice, OwnedSlice, etc.) into the
    /// shared handle type. Returns a `Box::into_raw` pointer ready to hand
    /// across the FFI boundary.
    pub fn into_handle(inner: Arc<dyn BlockDevice>) -> *mut FsCoreDevice {
        Box::into_raw(Box::new(FsCoreDevice { inner }))
    }

    /// Borrow the inner device. `Arc::clone` it if you want shared
    /// ownership — e.g. when handing the device to a slice adapter while
    /// keeping the original handle alive.
    pub fn inner(&self) -> &Arc<dyn BlockDevice> {
        &self.inner
    }
}

/// Free a device handle. Safe to call with NULL (no-op).
///
/// # THIS PRESERVES THE LAST ERROR MESSAGE
///
/// It returns `void`, so it can never report anything through the error
/// slot — and it used to clear the slot anyway, because it went through
/// [`ffi_guard_or`]. That destroyed the diagnostic in the ordinary C
/// cleanup shape, where the close comes before the log:
///
/// ```c
/// if (fs_core_device_read_at(h, off, buf, len) != FS_CORE_OK) goto fail;
/// ...
/// fail:
///     fs_core_device_close(h);
///     log("%s", fs_core_last_error_message());   /* was NULL */
/// ```
///
/// A caller may now close before reading the message. Nothing on this
/// path reads or writes the slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_close(handle: *mut FsCoreDevice) {
    if handle.is_null() {
        return;
    }
    ffi_guard_cleanup(|| unsafe {
        drop(Box::from_raw(handle));
    });
}

/// Total device size in bytes. Returns 0 if `handle` is NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_size_bytes(handle: *const FsCoreDevice) -> u64 {
    if handle.is_null() {
        // 0 is also what an empty device reports, so the message is the
        // only thing that separates the two -- and leaving the previous
        // call's message here explained this answer with something that
        // happened somewhere else.
        set_last_error("fs_core_device_size_bytes: handle is null");
        return 0;
    }
    ffi_guard_or(0, || unsafe { (*handle).inner.size_bytes() })
}

/// True if `write_at` is likely to succeed. Returns false on NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_is_writable(handle: *const FsCoreDevice) -> bool {
    if handle.is_null() {
        // `false` is also what a perfectly good read-only device reports.
        set_last_error("fs_core_device_is_writable: handle is null");
        return false;
    }
    ffi_guard_or(false, || unsafe { (*handle).inner.is_writable() })
}

/// Read exactly `len` bytes from `offset` into `buf`. `buf` must be at
/// least `len` bytes. Returns an `FsCoreErrorCode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_read_at(
    handle: *const FsCoreDevice,
    offset: u64,
    buf: *mut u8,
    len: usize,
) -> FsCoreErrorCode {
    // A null buffer is refused whatever the length. `from_raw_parts_mut`
    // requires a non-null, aligned pointer even for a zero-length slice,
    // so `(NULL, 0)` was undefined behaviour rather than the no-op it
    // looks like -- in a crate that otherwise denies
    // `unsafe_op_in_unsafe_fn`.
    if handle.is_null() {
        set_last_error("fs_core_device_read_at: handle is null");
        return FsCoreErrorCode::NullArg;
    }
    if buf.is_null() {
        set_last_error("fs_core_device_read_at: buf is null");
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| {
        let slice_buf = unsafe { slice::from_raw_parts_mut(buf, len) };
        unsafe { (*handle).inner.read_at(offset, slice_buf) }
    })
}

/// Write exactly `len` bytes from `buf` to `offset`. Returns `ReadOnly`
/// for read-only devices.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_write_at(
    handle: *const FsCoreDevice,
    offset: u64,
    buf: *const u8,
    len: usize,
) -> FsCoreErrorCode {
    // Null is refused whatever the length; see `fs_core_device_read_at`.
    if handle.is_null() {
        set_last_error("fs_core_device_write_at: handle is null");
        return FsCoreErrorCode::NullArg;
    }
    if buf.is_null() {
        set_last_error("fs_core_device_write_at: buf is null");
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| {
        let slice_buf = unsafe { slice::from_raw_parts(buf, len) };
        unsafe { (*handle).inner.write_at(offset, slice_buf) }
    })
}

/// Flush pending writes to stable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_flush(handle: *const FsCoreDevice) -> FsCoreErrorCode {
    if handle.is_null() {
        set_last_error("fs_core_device_flush: handle is null");
        return FsCoreErrorCode::NullArg;
    }
    ffi_guard(|| unsafe { (*handle).inner.flush() })
}

// ---------------------------------------------------------------------------
// Convenience: open a regular file as a device. Saves callers the trouble
// of building a Rust crate just to wrap `FileDevice`.
// ---------------------------------------------------------------------------

/// Open `path` (NUL-terminated UTF-8) as a `FileDevice` and return a
/// handle. Pass `writable=true` for RW. On failure returns NULL and the
/// thread-local last-error has detail.
#[cfg(any(unix, windows))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_file_open(
    path: *const c_char,
    writable: bool,
) -> *mut FsCoreDevice {
    if path.is_null() {
        set_last_error("path is null");
        return ptr::null_mut();
    }
    ffi_guard_or(ptr::null_mut(), || {
        let cstr = unsafe { std::ffi::CStr::from_ptr(path) };
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("path is not valid UTF-8");
                return ptr::null_mut();
            }
        };
        let dev = if writable {
            crate::file_device::FileDevice::open_rw(s)
        } else {
            crate::file_device::FileDevice::open(s)
        };
        match dev {
            Ok(d) => FsCoreDevice::into_handle(Arc::new(d)),
            Err(e) => {
                set_last_error(e.to_string());
                ptr::null_mut()
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Callback-backed device. Used when the caller already owns the underlying
// resource (FSKit FSBlockDeviceResource, Go file handle, C-side fd) and
// wants to expose it as an `FsCoreDevice` so it can be stacked under a
// container reader (qcow2, vhd, ...) before reaching a filesystem driver.
// ---------------------------------------------------------------------------

/// Read callback. Returns 0 on success, non-zero (errno-like) on failure.
/// Must fully fill `len` bytes — short reads are treated as I/O errors.
pub type FsCoreReadCb =
    Option<unsafe extern "C" fn(ctx: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> c_int>;

/// Write callback. NULL → device is read-only.
pub type FsCoreWriteCb = Option<
    unsafe extern "C" fn(ctx: *mut c_void, offset: u64, buf: *const u8, len: usize) -> c_int,
>;

/// Flush/fsync callback. NULL → flush is a no-op.
pub type FsCoreFlushCb = Option<unsafe extern "C" fn(ctx: *mut c_void) -> c_int>;

/// Configuration passed to [`fs_core_device_from_callbacks`].
#[repr(C)]
pub struct FsCoreCallbackCfg {
    pub read: FsCoreReadCb,
    pub write: FsCoreWriteCb,
    pub flush: FsCoreFlushCb,
    pub ctx: *mut c_void,
    pub size: u64,
}

/// Turn a callback's non-zero return into an `io::Error`.
fn cb_io_err(rc: c_int, op: &str) -> io::Error {
    io::Error::other(format!("callback {op} returned {rc}"))
}

/// The host callback contract, in one place: **zero is success**.
///
/// All three adapters below wrapped a call in the same four lines —
/// invoke, compare against zero, `Ok(())` or `cb_io_err`. Three copies
/// of a convention is three chances to write `rc != 0` where the others
/// write `rc == 0`, and a caller would see reads succeed while writes
/// reported failure on the very same device.
///
/// `op` names the operation in the error, which is the only thing the
/// three genuinely differ in.
fn cb_result(rc: c_int, op: &'static str) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(cb_io_err(rc, op))
    }
}

/// Build an [`FsCoreDevice`] backed by host-provided callbacks. Returns NULL
/// on failure (config null, read callback null, etc.) and stashes detail in
/// the thread-local last-error.
///
/// `cfg.ctx` is opaque to fs-core; it is passed back verbatim to every
/// callback invocation. The caller is responsible for ensuring it remains
/// valid until [`fs_core_device_close`] is called on the returned handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_from_callbacks(
    cfg: *const FsCoreCallbackCfg,
) -> *mut FsCoreDevice {
    if cfg.is_null() {
        set_last_error("cfg is null");
        return ptr::null_mut();
    }
    ffi_guard_or(ptr::null_mut(), || unsafe {
        let cfg = &*cfg;
        let read_fn = match cfg.read {
            Some(f) => f,
            None => {
                set_last_error("cfg.read is null");
                return ptr::null_mut();
            }
        };
        let write_fn = cfg.write;
        let flush_fn = cfg.flush;
        // `*mut c_void` is `!Send + !Sync` by default, and `unsafe impl
        // Send` on a newtype does not propagate cleanly through closure
        // auto-traits. Round-tripping the pointer through `usize` gives
        // something that is `Copy + Send + Sync`, and the callback
        // contract already puts the host on the hook for using `ctx`
        // safely across threads.
        let ctx_addr = cfg.ctx as usize;
        let size = cfg.size;

        let read_cb: crate::callback_device::ReadCb = Box::new(move |off, buf| {
            let ctx = ctx_addr as *mut c_void;
            cb_result(read_fn(ctx, off, buf.as_mut_ptr(), buf.len()), "read")
        });
        let write_cb: Option<crate::callback_device::WriteCb> = write_fn.map(|f| {
            Box::new(move |off, buf: &[u8]| {
                let ctx = ctx_addr as *mut c_void;
                cb_result(f(ctx, off, buf.as_ptr(), buf.len()), "write")
            }) as crate::callback_device::WriteCb
        });
        let flush_cb: Option<crate::callback_device::FlushCb> = flush_fn.map(|f| {
            Box::new(move || {
                let ctx = ctx_addr as *mut c_void;
                cb_result(f(ctx), "flush")
            }) as crate::callback_device::FlushCb
        });

        let dev = CallbackDevice {
            size,
            read: read_cb,
            write: write_cb,
            flush: flush_cb,
        };
        FsCoreDevice::into_handle(Arc::new(dev))
    })
}

// ---------------------------------------------------------------------------
// Slice constructor. Returns a child `FsCoreDevice` whose byte 0 maps to
// `start` of the parent and whose addressable range is `length` bytes.
// Useful for partition-table walkers that want to hand one partition to
// a filesystem driver without copying. The slice keeps an `Arc` to the
// parent, so closing the parent before the slice is fine.
// ---------------------------------------------------------------------------

/// Read-only slice. Writes via the returned handle return
/// `FS_CORE_READ_ONLY` regardless of the parent's writability.
///
/// `length` is clamped to what the parent can back, and a `start` at or
/// past the parent's end returns NULL with a message — see
/// [`crate::slice::window_on_parent`]. Read `fs_core_device_size_bytes`
/// on the returned handle rather than assuming it is `length`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_slice_ro(
    parent: *const FsCoreDevice,
    start: u64,
    length: u64,
) -> *mut FsCoreDevice {
    if parent.is_null() {
        set_last_error("parent is null");
        return ptr::null_mut();
    }
    ffi_guard_or(ptr::null_mut(), || unsafe {
        let parent_arc = (*parent).inner().clone();
        let Some(length) = slice_window(parent_arc.size_bytes(), start, length, "slice_ro") else {
            return ptr::null_mut();
        };
        // OwnedSlice takes Arc<dyn BlockRead>; trait upcast from
        // BlockDevice -> BlockRead is supported in the pinned toolchain.
        let parent_read: Arc<dyn crate::block::BlockRead> = parent_arc;
        let slice = crate::slice::OwnedSlice::new(parent_read, start, length);
        FsCoreDevice::into_handle(Arc::new(slice))
    })
}

/// Read-write slice. Writes are forwarded to the parent at `start +
/// offset`; writes outside `[0, length)` return `FS_CORE_OUT_OF_BOUNDS`.
/// If the parent reports `is_writable() == false`, write attempts return
/// `FS_CORE_READ_ONLY`.
///
/// `length` is clamped to what the parent can back, and a `start` at or
/// past the parent's end returns NULL with a message — see
/// [`crate::slice::window_on_parent`]. Read `fs_core_device_size_bytes`
/// on the returned handle rather than assuming it is `length`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fs_core_device_slice_rw(
    parent: *const FsCoreDevice,
    start: u64,
    length: u64,
) -> *mut FsCoreDevice {
    if parent.is_null() {
        set_last_error("parent is null");
        return ptr::null_mut();
    }
    ffi_guard_or(ptr::null_mut(), || unsafe {
        let parent_arc = (*parent).inner().clone();
        let Some(length) = slice_window(parent_arc.size_bytes(), start, length, "slice_rw") else {
            return ptr::null_mut();
        };
        let slice = crate::slice::OwnedRwSlice::new(parent_arc, start, length);
        FsCoreDevice::into_handle(Arc::new(slice))
    })
}

/// The slice window both C constructors take, or `None` with the error
/// slot already set.
///
/// The clamp itself lives in [`crate::slice::window_on_parent`] and is
/// applied again inside the slice constructors, so calling it here is
/// not the check — it is how the C ABI learns that there was nothing to
/// slice, which is the one outcome a `*mut` return can express and an
/// infallible Rust constructor cannot. A zero-byte handle would be
/// technically honest and useless to debug: the mount that follows fails
/// on its superblock read with no hint that the window was the problem.
fn slice_window(parent_size: u64, start: u64, length: u64, what: &str) -> Option<u64> {
    match crate::slice::window_on_parent(parent_size, start, length) {
        Some(clamped) => Some(clamped),
        None => {
            set_last_error(format!(
                "fs_core_device_{what}: start {start} is at or past the end of the \
                 parent device ({parent_size} bytes), so there is nothing to slice"
            ));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — exercise the FFI surface from Rust. The C side is verified by
// the consumer crates that use these functions through their own headers.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// THE MESSAGE, not the fact that something panicked.
    ///
    /// Both shapes a panic payload takes: `panic!("literal")` gives a
    /// `&'static str`, and `panic!("{x}")` or an out-of-bounds index
    /// gives a `String`. A guard that reports neither tells its caller
    /// only what it already knew.
    #[test]
    fn a_caught_panic_reports_what_it_said() {
        let literal =
            std::panic::catch_unwind(|| panic!("a literal message")).expect_err("it panicked");
        assert_eq!(panic_message(&literal), "a literal message");

        let owned = std::panic::catch_unwind(|| {
            let v: Vec<u8> = Vec::new();
            let _ = v[3];
        })
        .expect_err("it panicked");
        assert!(
            panic_message(&owned).contains("index out of bounds"),
            "the index panic's own words should survive: {}",
            panic_message(&owned)
        );

        // Anything else says so rather than pretending to a message.
        let odd =
            std::panic::catch_unwind(|| std::panic::panic_any(42u8)).expect_err("it panicked");
        assert_eq!(panic_message(&odd), "panic in FFI");
    }

    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn tmp_image(bytes: &[u8]) -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static C: AtomicU32 = AtomicU32::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir()
            .join(format!("fs_core_ffi_{}_{n}.img", std::process::id()))
            .to_string_lossy()
            .into_owned();
        File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    #[test]
    fn open_read_close_round_trip() {
        let path = tmp_image(b"hello, fs-core ffi");
        let cpath = CString::new(path.as_str()).unwrap();
        let h = unsafe { fs_core_file_open(cpath.as_ptr(), false) };
        assert!(!h.is_null(), "open failed");

        unsafe {
            assert_eq!(fs_core_device_size_bytes(h), 18);
            assert!(!fs_core_device_is_writable(h));

            let mut buf = [0u8; 5];
            let rc = fs_core_device_read_at(h, 0, buf.as_mut_ptr(), buf.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(&buf, b"hello");

            // Write should fail with ReadOnly.
            let rc = fs_core_device_write_at(h, 0, b"x".as_ptr(), 1);
            assert_eq!(rc, FsCoreErrorCode::ReadOnly);

            fs_core_device_close(h);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn null_args_return_null_arg() {
        let mut buf = [0u8; 4];
        let rc = unsafe { fs_core_device_read_at(ptr::null(), 0, buf.as_mut_ptr(), buf.len()) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        let rc = unsafe { fs_core_device_flush(ptr::null()) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
    }

    #[test]
    fn last_error_populated_on_open_failure() {
        let cpath = CString::new("/path/that/does/not/exist/we/hope").unwrap();
        let h = unsafe { fs_core_file_open(cpath.as_ptr(), false) };
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned() };
        assert!(!s.is_empty(), "expected an error message");
    }

    // ---- callback-backed device tests --------------------------------

    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    struct CbState {
        data: Vec<u8>,
        flushed: u32,
    }

    /// Trampoline that pulls a `*mut CbState` out of the opaque ctx.
    unsafe extern "C" fn t_read(ctx: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        // `off + len` was computed BEFORE the bounds check that exists
        // to refuse a past-end range, so a wild offset panicked here
        // instead of returning 5 -- the same pair of lines, and the
        // same defect, as the doubles in `test_device`.
        let Ok((off, _)) = crate::test_device::range_within(st.data.len(), offset, len) else {
            return 5; // out of bounds
        };
        unsafe {
            std::ptr::copy_nonoverlapping(st.data.as_ptr().add(off), buf, len);
        }
        0
    }
    unsafe extern "C" fn t_write(
        ctx: *mut c_void,
        offset: u64,
        buf: *const u8,
        len: usize,
    ) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        let Ok((off, _)) = crate::test_device::range_within(st.data.len(), offset, len) else {
            return 5;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(buf, st.data.as_mut_ptr().add(off), len);
        }
        0
    }
    unsafe extern "C" fn t_flush(ctx: *mut c_void) -> c_int {
        let st = unsafe { &mut *(ctx as *mut CbState) };
        st.flushed += 1;
        0
    }

    #[test]
    fn callback_device_round_trip_rw() {
        let mut st = Box::new(CbState {
            data: vec![0u8; 32],
            flushed: 0,
        });
        for (i, b) in st.data.iter_mut().enumerate() {
            *b = i as u8;
        }
        let ctx = &mut *st as *mut CbState as *mut c_void;

        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: Some(t_write),
            flush: Some(t_flush),
            ctx,
            size: 32,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null(), "device_from_callbacks returned NULL");

        unsafe {
            assert_eq!(fs_core_device_size_bytes(h), 32);
            assert!(fs_core_device_is_writable(h));

            let mut buf = [0u8; 4];
            let rc = fs_core_device_read_at(h, 4, buf.as_mut_ptr(), buf.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(buf, [4, 5, 6, 7]);

            let payload = [0xDE, 0xAD, 0xBE, 0xEF];
            let rc = fs_core_device_write_at(h, 8, payload.as_ptr(), payload.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);

            let rc = fs_core_device_flush(h);
            assert_eq!(rc, FsCoreErrorCode::Ok);

            let mut readback = [0u8; 4];
            let rc = fs_core_device_read_at(h, 8, readback.as_mut_ptr(), readback.len());
            assert_eq!(rc, FsCoreErrorCode::Ok);
            assert_eq!(readback, payload);

            fs_core_device_close(h);
        }
        assert_eq!(st.flushed, 1);
        assert_eq!(&st.data[8..12], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn callback_device_readonly_when_write_null() {
        let mut st = Box::new(CbState {
            data: vec![0xAAu8; 16],
            flushed: 0,
        });
        let ctx = &mut *st as *mut CbState as *mut c_void;
        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: None,
            flush: None,
            ctx,
            size: 16,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null());
        unsafe {
            assert!(!fs_core_device_is_writable(h));
            let rc = fs_core_device_write_at(h, 0, [1u8].as_ptr(), 1);
            assert_eq!(rc, FsCoreErrorCode::ReadOnly);
            // Flush is a no-op when callback is NULL.
            assert_eq!(fs_core_device_flush(h), FsCoreErrorCode::Ok);
            fs_core_device_close(h);
        }
        // suppress unused warning
        let _ = StdArc::new(StdMutex::new(0u8));
    }

    /// The last error as text, or `None`. Read directly rather than
    /// through `panic_message_tests::last_error`, which is a different
    /// module.
    fn cb_last_error() -> Option<String> {
        let p = fs_core_last_error_message();
        if p.is_null() {
            return None;
        }
        Some(
            unsafe { std::ffi::CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// THE THIRD COPY OF THE BOUNDS RULE, AND THE ONE NOTHING HELD.
    ///
    /// `tests/common/mod.rs` states the standard this crate works to:
    /// the rule is written twice, so both copies carry a test that pins
    /// it. #84 fixed a THIRD copy -- these two trampolines -- and gave
    /// it none. Reverting both to the pre-fix arithmetic left
    /// `cargo test --locked --lib` at 92 passed, 0 failed, and `--lib`
    /// is the complete check: they are `#[cfg(test)]` items in the lib
    /// target, so no integration test can reach them.
    ///
    /// # What the reverted arithmetic actually does, measured
    ///
    /// Not what I first wrote here. `ffi_guard` wraps the call in
    /// `catch_unwind`, so the obvious expectation is that an overflow
    /// panic returns `FsCoreErrorCode::Panic`. It does not: a
    /// trampoline is `extern "C"`, panicking out of one is
    /// non-unwinding, and the process aborts before any code is
    /// returned. With `t_read` reverted, `cargo test --locked --lib`
    /// gives
    ///
    /// ```text
    /// thread caused non-unwinding panic. aborting.
    /// process didn't exit successfully: ... (signal: 6, SIGABRT)
    /// EXIT=101, and NO `... FAILED` line for any test
    /// ```
    ///
    /// So the control produces a crash rather than a failure, which is
    /// why the exit status is the thing to read: counting `test
    /// result:` lines cannot see an aborted binary.
    ///
    /// # Why the assertion is still about the MESSAGE
    ///
    /// The abort makes the revert impossible to miss, but it is not
    /// what these assertions are for. `ffi_guard` turns any refusal
    /// into a non-`Ok` code, so "not Ok" alone would also be satisfied
    /// by the wrapper refusing before the trampoline was ever called --
    /// a test that passes without exercising the copy it exists to
    /// pin. `callback read returned 5` is the trampoline's own
    /// out-of-bounds path and nothing else produces it. `Panic` is
    /// excluded too, for the case where a future edit makes the
    /// unwinding reachable.
    #[test]
    fn a_callback_read_at_a_wild_offset_is_refused_rather_than_panicking() {
        let mut st = Box::new(CbState {
            data: vec![0x5Au8; 32],
            flushed: 0,
        });
        let ctx = &mut *st as *mut CbState as *mut c_void;
        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: Some(t_write),
            flush: Some(t_flush),
            ctx,
            size: 32,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null());

        // Three shapes, and the first two are the ones the arithmetic
        // used to get wrong: an offset that is itself past every
        // addressable byte, and an offset whose sum with the length
        // wraps. The third is the ordinary past-end read, which the
        // pre-fix code also handled -- it is here so a guard that
        // refused everything would not look like a pass.
        for (what, offset, want) in [
            ("the very top of the address space", u64::MAX, 8usize),
            ("an offset whose sum with len wraps", u64::MAX - 2, 8usize),
            ("an ordinary past-end read", 64u64, 8usize),
        ] {
            let mut buf = [0u8; 8];
            let rc = unsafe { fs_core_device_read_at(h, offset, buf.as_mut_ptr(), want) };
            assert_ne!(
                rc,
                FsCoreErrorCode::Panic,
                "{what}: the trampoline panicked instead of refusing; \
                 last error was {:?}",
                cb_last_error()
            );
            assert_ne!(rc, FsCoreErrorCode::Ok, "{what}: must not succeed");
            let msg = cb_last_error().unwrap_or_default();
            assert!(
                msg.contains("callback read returned 5"),
                "{what}: the refusal must come from the trampoline's own \
                 out-of-bounds path, not from a panic caught by ffi_guard. \
                 last error was {msg:?}"
            );
        }
        unsafe { fs_core_device_close(h) };
    }

    /// The write half. `t_write` is the copy most easily forgotten, and
    /// the one that would corrupt rather than merely panic.
    #[test]
    fn a_callback_write_at_a_wild_offset_is_refused_rather_than_panicking() {
        let mut st = Box::new(CbState {
            data: vec![0x5Au8; 32],
            flushed: 0,
        });
        let ctx = &mut *st as *mut CbState as *mut c_void;
        let cfg = FsCoreCallbackCfg {
            read: Some(t_read),
            write: Some(t_write),
            flush: Some(t_flush),
            ctx,
            size: 32,
        };
        let h = unsafe { fs_core_device_from_callbacks(&cfg) };
        assert!(!h.is_null());

        let payload = [0xEEu8; 8];
        for (what, offset) in [
            ("the very top of the address space", u64::MAX),
            ("an offset whose sum with len wraps", u64::MAX - 2),
            ("an ordinary past-end write", 64u64),
        ] {
            let rc = unsafe { fs_core_device_write_at(h, offset, payload.as_ptr(), payload.len()) };
            assert_ne!(
                rc,
                FsCoreErrorCode::Panic,
                "{what}: the trampoline panicked instead of refusing; \
                 last error was {:?}",
                cb_last_error()
            );
            assert_ne!(rc, FsCoreErrorCode::Ok, "{what}: must not succeed");
            let msg = cb_last_error().unwrap_or_default();
            assert!(
                msg.contains("callback write returned 5"),
                "{what}: the refusal must come from the trampoline's own \
                 out-of-bounds path, not from a panic caught by ffi_guard. \
                 last error was {msg:?}"
            );
        }
        unsafe { fs_core_device_close(h) };
        // Nothing was written anywhere: a refused write must not have
        // narrowed a wild offset into a plausible one on the way out.
        assert!(
            st.data.iter().all(|b| *b == 0x5A),
            "a refused write modified the backing buffer"
        );
    }

    #[test]
    fn callback_device_null_cfg_returns_null() {
        let h = unsafe { fs_core_device_from_callbacks(ptr::null()) };
        assert!(h.is_null());
        let msg = fs_core_last_error_message();
        assert!(!msg.is_null());
    }
}

#[cfg(test)]
mod panic_message_tests {
    use super::*;
    use crate::block::{BlockDevice, BlockRead};

    /// A device whose every method panics.
    ///
    /// Not a hypothetical: a driver's `size_bytes` computes a geometry
    /// from on-disk fields, and an arithmetic overflow there panics.
    /// The FFI boundary is where that has to stop being a panic and
    /// start being a reportable error.
    struct Panicking;

    impl BlockRead for Panicking {
        fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> Result<(), Error> {
            panic!("read_at exploded")
        }
        fn size_bytes(&self) -> u64 {
            panic!("size_bytes exploded")
        }
    }
    impl BlockDevice for Panicking {
        fn is_writable(&self) -> bool {
            panic!("is_writable exploded")
        }
    }

    fn handle() -> *mut FsCoreDevice {
        FsCoreDevice::into_handle(std::sync::Arc::new(Panicking))
    }

    fn last_error() -> Option<String> {
        let p = fs_core_last_error_message();
        if p.is_null() {
            return None;
        }
        Some(
            unsafe { std::ffi::CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// A panic caught at the boundary must leave a message behind.
    ///
    /// `fs_core_device_size_bytes` returns 0 on panic — and 0 is also
    /// what a legitimately empty device returns. Without a message the
    /// caller cannot tell "this device is empty" from "the driver
    /// exploded computing its size", which is the whole reason the
    /// thread-local error slot exists.
    #[test]
    fn a_panic_computing_the_size_is_reported_not_just_swallowed() {
        clear_last_error();
        let h = handle();
        let size = unsafe { fs_core_device_size_bytes(h) };
        assert_eq!(size, 0, "the fallback value is still returned");
        let msg = last_error().expect("a caught panic must leave a message");
        assert!(
            msg.contains("size_bytes exploded"),
            "the message should carry the panic's own text, got: {msg}"
        );
        unsafe { fs_core_device_close(h) };
    }

    /// Same for the writability probe, whose fallback is `false` — the
    /// answer a perfectly good read-only device gives.
    #[test]
    fn a_panic_probing_writability_is_reported() {
        clear_last_error();
        let h = handle();
        let writable = unsafe { fs_core_device_is_writable(h) };
        assert!(!writable, "the fallback value is still returned");
        assert!(
            last_error().is_some(),
            "a caught panic must leave a message"
        );
        unsafe { fs_core_device_close(h) };
    }

    /// A call that succeeds must not leave a stale message behind for
    /// the next one to pick up.
    #[test]
    fn a_successful_call_clears_the_previous_error() {
        let h = handle();
        let _ = unsafe { fs_core_device_size_bytes(h) };
        assert!(last_error().is_some(), "setup: an error is recorded");
        unsafe { fs_core_device_close(h) };

        struct Sixteen;
        impl BlockRead for Sixteen {
            fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> Result<(), Error> {
                Ok(())
            }
            fn size_bytes(&self) -> u64 {
                16
            }
        }
        impl BlockDevice for Sixteen {}
        let h2 = FsCoreDevice::into_handle(std::sync::Arc::new(Sixteen));
        assert_eq!(unsafe { fs_core_device_size_bytes(h2) }, 16);
        assert!(
            last_error().is_none(),
            "a call that worked must not leave the previous panic's message in place"
        );
        unsafe { fs_core_device_close(h2) };
    }

    // ---------------------------------------------------------------------
    // A null argument explains itself, rather than inheriting whatever the
    // previous call left behind.
    //
    // Every null check returned ABOVE the guard, and the guard is the only
    // thing that touches the error slot -- so a null-argument call left the
    // previous call's message readable and a caller attributed something
    // that happened elsewhere to this call.
    //
    // Worst for `size_bytes` and `is_writable`, whose fallbacks are both
    // legitimate answers: the caller got `0` or `false` AND a confident
    // explanation of it belonging to a different operation. That is not
    // "the evidence was thrown away", which this file already argues
    // against -- it is evidence about something else, substituted.
    //
    // The slot is seeded with `set_last_error` rather than by provoking a
    // real failure. That is exactly what a failed call does to it --
    // `ffi_guard` sets it the same way -- and it makes "not the earlier
    // message" an exact comparison rather than a fuzzy one.
    // ---------------------------------------------------------------------

    /// Distinctive enough that finding it in a message is unambiguous.
    const SEEDED: &str = "SEEDED-earlier-failure-belonging-to-another-call";

    /// Assert the slot names a null argument and has lost the seed.
    fn assert_named_null(msg: Option<String>, expect: &str) {
        let msg = msg.expect("a null argument must leave a message of its own");
        assert!(
            msg.contains(expect),
            "the message must name the null argument ({expect}), got: {msg}"
        );
        assert!(
            !msg.contains(SEEDED),
            "the previous call's message must not survive to explain this one, got: {msg}"
        );
    }

    #[test]
    fn a_null_handle_to_size_bytes_names_the_argument() {
        set_last_error(SEEDED);
        let size = unsafe { fs_core_device_size_bytes(std::ptr::null()) };
        assert_eq!(size, 0, "the fallback value is still returned");
        assert_named_null(last_error(), "fs_core_device_size_bytes: handle is null");
    }

    #[test]
    fn a_null_handle_to_is_writable_names_the_argument() {
        set_last_error(SEEDED);
        let writable = unsafe { fs_core_device_is_writable(std::ptr::null()) };
        assert!(!writable, "the fallback value is still returned");
        assert_named_null(last_error(), "fs_core_device_is_writable: handle is null");
    }

    #[test]
    fn a_null_handle_to_flush_names_the_argument() {
        set_last_error(SEEDED);
        let rc = unsafe { fs_core_device_flush(std::ptr::null()) };
        assert_eq!(rc, FsCoreErrorCode::NullArg, "the code is still NullArg");
        assert_named_null(last_error(), "fs_core_device_flush: handle is null");
    }

    /// `read_at` has two null arguments, and the message says which.
    ///
    /// A code of `NullArg` is honest but says nothing about *what* was
    /// null, and `fs_core.h` promises a human-readable message for every
    /// fallible call. Two arms, so neither can pass on the other's back.
    #[test]
    fn a_null_argument_to_read_at_names_which_one() {
        let mut buf = [0u8; 8];

        set_last_error(SEEDED);
        let rc = unsafe { fs_core_device_read_at(std::ptr::null(), 0, buf.as_mut_ptr(), 8) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        assert_named_null(last_error(), "fs_core_device_read_at: handle is null");

        // A real handle, a null buffer: the other arm.
        set_last_error(SEEDED);
        let h = handle();
        let rc = unsafe { fs_core_device_read_at(h, 0, std::ptr::null_mut(), 8) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        assert_named_null(last_error(), "fs_core_device_read_at: buf is null");
        unsafe { fs_core_device_close(h) };
    }

    /// Same for `write_at`.
    #[test]
    fn a_null_argument_to_write_at_names_which_one() {
        let buf = [0u8; 8];

        set_last_error(SEEDED);
        let rc = unsafe { fs_core_device_write_at(std::ptr::null(), 0, buf.as_ptr(), 8) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        assert_named_null(last_error(), "fs_core_device_write_at: handle is null");

        set_last_error(SEEDED);
        let h = handle();
        let rc = unsafe { fs_core_device_write_at(h, 0, std::ptr::null(), 8) };
        assert_eq!(rc, FsCoreErrorCode::NullArg);
        assert_named_null(last_error(), "fs_core_device_write_at: buf is null");
        unsafe { fs_core_device_close(h) };
    }

    /// CLOSE MUST NOT DESTROY THE MESSAGE A CALLER IS ABOUT TO READ.
    ///
    /// `close` returns `void`, so it can never fill the error slot — and it
    /// used to clear it anyway, by going through the guard that owns the
    /// slot for calls that *can* report. That breaks the ordinary C cleanup
    /// shape, where the free comes before the log:
    ///
    /// ```c
    /// fail:
    ///     fs_core_device_close(h);
    ///     log("%s", fs_core_last_error_message());   /* was NULL */
    /// ```
    #[test]
    fn close_preserves_the_message_a_caller_is_about_to_read() {
        set_last_error(SEEDED);
        let h = handle();
        unsafe { fs_core_device_close(h) };
        let msg = last_error().expect("close must not destroy the last error");
        assert!(
            msg.contains(SEEDED),
            "the diagnostic a caller closes before reading must survive, got: {msg}"
        );
    }

    /// The control for the one above: closing NULL is a no-op and always
    /// preserved the slot, because it returns before any guard. It passes
    /// before and after the fix, and it is here so that
    /// `close_preserves_...` failing points at the guard rather than at
    /// something about handles.
    #[test]
    fn closing_null_also_preserves_the_message() {
        set_last_error(SEEDED);
        unsafe { fs_core_device_close(std::ptr::null_mut()) };
        let msg = last_error().expect("a no-op must not clear the slot");
        assert!(msg.contains(SEEDED));
    }
}

// ---------------------------------------------------------------------------
// The published error numbering, pinned on both sides.
//
// `FsCoreErrorCode` and the `FsCoreErrorCode` enum in
// `include/fs_core.h` are two hand-written copies of one ABI, of which
// the header says "Stable: do not renumber." Nothing compiles the
// header, and every other test in this file compares codes symbolically
// — `assert_eq!(rc, FsCoreErrorCode::ShortRead)` — which is invariant
// under precisely the change that breaks the ABI: a variant inserted
// into the middle of one copy, or added to one copy and not the other.
//
// Two checks, because neither sees what the other does. Comparing the
// two files as text catches a variant that reached only one of them.
// Pinning the discriminants against literals catches a renumbering
// applied tidily to both, which is the change the header forbids and
// the one the text comparison would call agreement.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod error_code_abi_tests {
    use super::FsCoreErrorCode;

    /// The C spelling of a Rust variant name: `Ok` is `FS_CORE_OK`,
    /// `OutOfBounds` is `FS_CORE_OUT_OF_BOUNDS`.
    fn c_name(rust_name: &str) -> String {
        let mut out = String::from("FS_CORE_");
        for (i, ch) in rust_name.chars().enumerate() {
            if i != 0 && ch.is_ascii_uppercase() {
                out.push('_');
            }
            out.extend(ch.to_uppercase());
        }
        out
    }

    /// The body of an enum block: everything between an opening marker
    /// that must occur exactly once — so a second enum added to either
    /// file cannot quietly redirect the parse — and the first brace at
    /// column 0 after it, which must be followed by `closes_with`.
    fn enum_body<'a>(src: &'a str, opens_with: &str, closes_with: &str) -> &'a str {
        assert_eq!(
            src.matches(opens_with).count(),
            1,
            "{opens_with:?} must appear exactly once or this parses the wrong enum"
        );
        let start = src.find(opens_with).unwrap() + opens_with.len();
        let end = start
            + src[start..]
                .find("\n}")
                .expect("the enum block is closed by a brace at column 0");
        let after = &src[end + 2..];
        assert!(
            after.starts_with(closes_with),
            "the closing brace should be followed by {closes_with:?}, not {:?} — \
             the parse stopped somewhere other than the end of the enum",
            &after[..closes_with.len().min(after.len())]
        );
        &src[start..end]
    }

    /// The error codes as `src/ffi.rs` declares them, in declaration
    /// order, spelled the way C spells them.
    ///
    /// Reads this very file rather than listing the variants, so a
    /// variant added to the enum cannot be absent from what is compared
    /// against the header — that absence being the drift under guard,
    /// and a hand-maintained list here would share it.
    ///
    /// It refuses to guess. A line in the enum body that is neither
    /// blank, a comment, an attribute nor `Name = <integer>,` fails the
    /// test, because a parser that silently matches nothing agrees with
    /// every header.
    fn codes_declared_in_rust() -> Vec<(String, i32)> {
        // NORMALISED FIRST. A Windows checkout has CRLF, so every marker
        // below would carry a `\r` the source does not, and the
        // uniqueness assertion fails rather than parsing the wrong enum
        // -- which is the guard working, but it fails a correct file.
        // The caller owns the normalised copy because `enum_body`
        // borrows from it.
        let src = include_str!("ffi.rs").replace("\r\n", "\n");
        let body = enum_body(
            &src,
            "pub enum FsCoreErrorCode {\n",
            "\n\nimpl FsCoreErrorCode {",
        );
        assert!(
            !body.contains('{'),
            "the extracted Rust enum body should hold no nested braces: {body:?}"
        );
        body.lines()
            .map(str::trim)
            .filter(|l| !(l.is_empty() || l.starts_with("//") || l.starts_with("#[")))
            .map(|l| {
                let (name, value) = l
                    .strip_suffix(',')
                    .and_then(|entry| entry.split_once('='))
                    .unwrap_or_else(|| panic!("unparsed line in the Rust enum body: {l:?}"));
                let value = value.trim().parse().unwrap_or_else(|e| {
                    panic!("{name:?} has no literal discriminant ({e}): {l:?}")
                });
                (c_name(name.trim()), value)
            })
            .collect()
    }

    /// The same list as `include/fs_core.h` declares it, with the same
    /// refusal to guess: an entry it cannot read fails the test.
    fn codes_declared_in_c() -> Vec<(String, i32)> {
        let src = include_str!("../include/fs_core.h").replace("\r\n", "\n");
        let body = enum_body(&src, "typedef enum {\n", " FsCoreErrorCode;");

        // A C comment spans lines and sits between entries, so it goes
        // before the split on commas rather than after it.
        let mut stripped = String::new();
        let mut rest = body;
        while let Some(open) = rest.find("/*") {
            stripped.push_str(&rest[..open]);
            let tail = &rest[open + 2..];
            let close = tail
                .find("*/")
                .expect("an unterminated comment in the header's enum");
            rest = &tail[close + 2..];
        }
        stripped.push_str(rest);

        stripped
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let (name, value) = entry
                    .split_once('=')
                    .unwrap_or_else(|| panic!("unparsed entry in the C enum body: {entry:?}"));
                let value = value
                    .trim()
                    .parse()
                    .unwrap_or_else(|e| panic!("{name:?} has no literal value ({e}): {entry:?}"));
                (name.trim().to_owned(), value)
            })
            .collect()
    }

    /// Name for name and number for number, in the same order.
    ///
    /// This is the check a ninth code added to one file and not the
    /// other fails. The crate has no other defence against it: the
    /// build succeeds and every symbolic comparison still passes.
    #[test]
    fn the_header_and_the_rust_enum_publish_the_same_error_codes() {
        let rust = codes_declared_in_rust();
        let c = codes_declared_in_c();

        // Nine codes are published and a published code is never
        // withdrawn, so a parse returning fewer read less than the
        // enum — whatever it then agreed with.
        assert!(
            rust.len() >= 9 && c.len() >= 9,
            "both parses should reach every published code, got {} from Rust and {} from C",
            rust.len(),
            c.len()
        );
        assert_eq!(rust, c, "the two copies of the error ABI have drifted");
    }

    /// What the compiler assigns, against the numbers the header
    /// publishes as unchangeable.
    ///
    /// The text comparison above cannot see a renumbering applied to
    /// both files, and `BadString` is carried as a deliberately dead
    /// variant precisely so that 8 keeps its meaning for a consumer
    /// already switching on it.
    #[test]
    fn the_error_codes_still_have_the_numbers_they_were_published_with() {
        assert_eq!(FsCoreErrorCode::Ok as i32, 0);
        assert_eq!(FsCoreErrorCode::Io as i32, 1);
        assert_eq!(FsCoreErrorCode::ShortRead as i32, 2);
        assert_eq!(FsCoreErrorCode::ReadOnly as i32, 3);
        assert_eq!(FsCoreErrorCode::OutOfBounds as i32, 4);
        assert_eq!(FsCoreErrorCode::Custom as i32, 5);
        assert_eq!(FsCoreErrorCode::NullArg as i32, 6);
        assert_eq!(FsCoreErrorCode::Panic as i32, 7);
        assert_eq!(FsCoreErrorCode::BadString as i32, 8);
    }
}
