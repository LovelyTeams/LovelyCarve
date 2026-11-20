//! C ABI v1 between LovelyCarve executables and their native components.
//!
//! A component is a dynamic library beside `LovelyCarve.exe` and
//! `lovelycarve-cli.exe` that carries a large dependency the executables only
//! need for some operations, such as ONNX Runtime for [`inference`]. It
//! exports [`ENTRY_SYMBOL`] with the [`EntryFn`] signature; the host calls it
//! once after loading the library and keeps the returned [`ComponentApiV1`]
//! table for the rest of the process. Components are never unloaded.
//!
//! A call names a method and passes JSON `params` together with a raw
//! `payload` (file bytes), so large inputs are not re-encoded. A
//! [`STATUS_OK`] reply holds the method's JSON result; [`STATUS_ERROR`] and
//! [`STATUS_PANIC`] replies hold a UTF-8 message. Ownership follows the plugin
//! ABI: the component fills a [`Buffer`] and the host releases it through the
//! component's `free_buffer`, so each side frees memory with its own
//! allocator. Every function may be called concurrently from any thread.

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::{Deserialize, Serialize};

pub mod inference;

/// Version of the table layout and call convention; bumped on any
/// incompatible change.
pub const ABI_VERSION: u32 = 1;

/// Exported entry point, NUL-terminated for `GetProcAddress` / `dlsym`.
pub const ENTRY_SYMBOL: &[u8] = b"lovelycarve_component_v1\0";

/// The call succeeded; the reply holds the method's JSON result.
pub const STATUS_OK: i32 = 0;
/// The method failed; the reply holds a UTF-8 message for the user.
pub const STATUS_ERROR: i32 = 1;
/// A pointer argument was null, or the method name or params were invalid;
/// the reply holds a UTF-8 message when `out` was usable.
pub const STATUS_INVALID_ARGUMENT: i32 = 2;
/// A panic reached the boundary; the reply holds its message.
pub const STATUS_PANIC: i32 = 3;

/// Method every component answers with a JSON [`Description`].
pub const METHOD_DESCRIBE: &str = "component.describe";

/// Borrowed bytes; valid only for the duration of the call that receives them.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Bytes {
    pub data: *const u8,
    pub len: usize,
}

impl Bytes {
    pub fn from_slice(slice: &[u8]) -> Self {
        Self {
            data: slice.as_ptr(),
            len: slice.len(),
        }
    }

    /// # Safety
    ///
    /// `data` must point to `len` readable bytes that stay valid for `'a`,
    /// or `len` must be zero.
    pub unsafe fn as_slice<'a>(self) -> &'a [u8] {
        if self.len == 0 || self.data.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.data, self.len) }
        }
    }
}

/// Reply bytes owned by the component that filled them.
#[repr(C)]
#[derive(Debug)]
pub struct Buffer {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl Buffer {
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }

    /// Hands a vector across the boundary. Only [`Buffer::free_vec`] in the
    /// same library may release it.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Self {
            data: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        }
    }

    /// # Safety
    ///
    /// `data` must point to `len` readable bytes, or be null.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.data.is_null() || self.len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.data, self.len) }
        }
    }

    /// Releases a buffer produced by [`Buffer::from_vec`] and leaves it empty.
    ///
    /// # Safety
    ///
    /// The buffer must be empty or come from [`Buffer::from_vec`] in this same
    /// library, and must not have been released already.
    pub unsafe fn free_vec(&mut self) {
        if !self.data.is_null() {
            drop(unsafe { Vec::from_raw_parts(self.data, self.len, self.capacity) });
        }
        *self = Self::empty();
    }
}

/// Runs `method` with JSON `params` and the binary `payload`, and writes the
/// reply to `out`. Blocks until the method finishes.
pub type CallFn =
    unsafe extern "C" fn(method: Bytes, params: Bytes, payload: Bytes, out: *mut Buffer) -> i32;

/// Releases a reply filled by [`ComponentApiV1::call`].
pub type FreeBufferFn = unsafe extern "C" fn(buffer: *mut Buffer);

/// Functions of a loaded component, filled in by its entry point.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ComponentApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub call: CallFn,
    pub free_buffer: FreeBufferFn,
}

/// `struct_size` of a v1 [`ComponentApiV1`].
pub const COMPONENT_API_SIZE: u32 = std::mem::size_of::<ComponentApiV1>() as u32;

/// Writes the component's table to `api`. Returns [`STATUS_OK`], or
/// [`STATUS_INVALID_ARGUMENT`] when `api` is null; `api` is only written on
/// success.
pub type EntryFn = unsafe extern "C" fn(api: *mut ComponentApiV1) -> i32;

/// The reply to [`METHOD_DESCRIBE`]: identity plus the component-specific
/// capabilities of this build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Description<C> {
    /// Library name without platform prefix or suffix.
    pub name: String,
    /// Crate version the library was built from.
    pub version: String,
    pub capabilities: C,
}

/// Outcome of one method, before it crosses the boundary.
#[derive(Debug)]
pub enum CallError {
    /// Unknown method, malformed params or an inconsistent payload.
    InvalidArgument(String),
    /// The method ran and failed; the message is shown to the user.
    Failed(String),
}

/// Component-side implementation of a [`CallFn`]: decodes the arguments,
/// runs `dispatch`, catches panics and writes the reply to `out`.
///
/// # Safety
///
/// `method`, `params` and `payload` must satisfy [`Bytes::as_slice`], and
/// `out` must be null or point to writable memory for one [`Buffer`].
pub unsafe fn serve_call(
    method: Bytes,
    params: Bytes,
    payload: Bytes,
    out: *mut Buffer,
    dispatch: impl FnOnce(&str, &[u8], &[u8]) -> Result<Vec<u8>, CallError>,
) -> i32 {
    if out.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let (method, params, payload) =
        unsafe { (method.as_slice(), params.as_slice(), payload.as_slice()) };
    let outcome = catch_unwind(AssertUnwindSafe(|| match std::str::from_utf8(method) {
        Ok(method) => dispatch(method, params, payload),
        Err(_) => Err(CallError::InvalidArgument(
            "component method name is not UTF-8".to_string(),
        )),
    }));
    let (status, reply) = match outcome {
        Ok(Ok(reply)) => (STATUS_OK, reply),
        Ok(Err(CallError::InvalidArgument(message))) => (STATUS_INVALID_ARGUMENT, message.into()),
        Ok(Err(CallError::Failed(message))) => (STATUS_ERROR, message.into()),
        Err(panic) => (STATUS_PANIC, panic_message(panic.as_ref()).into()),
    };
    unsafe { out.write(Buffer::from_vec(reply)) };
    status
}

/// Component-side implementation of a [`FreeBufferFn`].
///
/// # Safety
///
/// `buffer` must be null or a reply this library filled through
/// [`serve_call`] that was not released yet.
pub unsafe fn serve_free_buffer(buffer: *mut Buffer) {
    if let Some(buffer) = unsafe { buffer.as_mut() } {
        unsafe { buffer.free_vec() };
    }
}

/// Component-side implementation of an [`EntryFn`].
///
/// # Safety
///
/// `api` must be null or point to writable memory for one [`ComponentApiV1`].
pub unsafe fn serve_entry(
    api: *mut ComponentApiV1,
    call: CallFn,
    free_buffer: FreeBufferFn,
) -> i32 {
    if api.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    unsafe {
        api.write(ComponentApiV1 {
            abi_version: ABI_VERSION,
            struct_size: COMPONENT_API_SIZE,
            call,
            free_buffer,
        });
    }
    STATUS_OK
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string());
    format!("component panicked: {detail}")
}

#[cfg(test)]
mod tests;
