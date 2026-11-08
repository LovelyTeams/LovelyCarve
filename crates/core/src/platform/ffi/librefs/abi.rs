//! Native ReFS ABI layouts, function signatures and attribute constants.
use super::*;

// ---------- C ABI structs ----------

/// Mirror of `sys_device_callbacks` in `include/refs/sys_user.h`.
#[repr(C)]
pub(super) struct SysDeviceCallbacks {
    pub(super) user_data: *mut c_void,
    pub(super) pread: Option<unsafe extern "C" fn(*mut c_void, u64, usize, *mut c_void) -> c_int>,
    pub(super) get_size: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> c_int>,
    pub(super) close: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// Mirror of `sys_iohandler` in `include/refs/sys.h`. `fsapi_iohandler`
/// is the same struct.
#[repr(C)]
pub(super) struct SysIohandler {
    pub(super) context: *mut c_void,
    pub(super) handle_io:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void, u64, usize) -> c_int>,
    pub(super) handle_hole: Option<unsafe extern "C" fn(*mut c_void, usize) -> c_int>,
    pub(super) copy_data: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> c_int>,
    pub(super) get_data: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> c_int>,
}

/// Mirror of `fsapi_iohandler_buffer_context` — librefs's built-in
/// scratch struct for "read into / write out of a user buffer".
#[repr(C)]
pub(super) struct FsapiBufferContext {
    pub(super) buf: *mut c_void,
    pub(super) remaining_size: usize,
    pub(super) is_read: u8,
}

/// Mirror of `sys_timespec` in `sys_user.h` (`{ u64 tv_sec; u32 tv_nsec; }`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct SysTimespec {
    pub(super) tv_sec: u64,
    pub(super) tv_nsec: u32,
}

/// Mirror of `fsapi_node_attributes` in `include/refs/fsapi.h`. Field
/// order must match exactly — the library reads `requested` and writes
/// every field whose bit is set in `valid`.
#[repr(C)]
pub(super) struct FsapiNodeAttributes {
    pub(super) requested: c_int,
    pub(super) valid: c_int,
    pub(super) is_directory: u8,
    /* implicit 7-byte pad to u64 alignment */
    pub(super) size: u64,
    pub(super) allocated_size: u64,
    pub(super) link_count: u64,
    pub(super) inode_number: u64,
    pub(super) mode: u32,
    pub(super) uid: u32,
    pub(super) gid: u32,
    /* implicit 4-byte pad to u64 alignment for sys_timespec.tv_sec */
    pub(super) creation_time: SysTimespec,
    pub(super) last_status_change_time: SysTimespec,
    pub(super) last_data_change_time: SysTimespec,
    pub(super) last_data_access_time: SysTimespec,
    pub(super) bsd_flags: u32,
    pub(super) windows_flags: u32,
    pub(super) symlink_target: *mut i8,
    pub(super) symlink_target_length: usize,
}

impl FsapiNodeAttributes {
    pub(super) fn new_requested(mask: c_int) -> Self {
        // SAFETY: zero is a valid bit pattern for every field; the C
        // library only consults `requested` on input.
        let mut s: Self = unsafe { std::mem::zeroed() };
        s.requested = mask;
        s
    }
}

/// `fsapi_node_attribute_types` bits we care about (see `fsapi.h:84`).
pub const FSAPI_ATTR_SIZE: c_int = 0x0001;
pub const FSAPI_ATTR_ALLOCATED_SIZE: c_int = 0x0002;
pub const FSAPI_ATTR_LINK_COUNT: c_int = 0x0004;
pub const FSAPI_ATTR_INODE_NUMBER: c_int = 0x0008;
pub const FSAPI_ATTR_MODE: c_int = 0x0010;
pub const FSAPI_ATTR_CREATION_TIME: c_int = 0x0080;
pub const FSAPI_ATTR_LAST_STATUS_CHANGE_TIME: c_int = 0x0100;
pub const FSAPI_ATTR_LAST_DATA_CHANGE_TIME: c_int = 0x0200;
pub const FSAPI_ATTR_LAST_DATA_ACCESS_TIME: c_int = 0x0400;
pub const FSAPI_ATTR_ALL: c_int = 0x1FFF;

// ---------- Function pointer types ----------

pub(super) type DevicePtr = *mut c_void;
pub(super) type VolumePtr = *mut c_void;
pub(super) type NodePtr = *mut c_void;

pub(super) type FnSysDeviceOpenCallbacks =
    unsafe extern "C" fn(*mut DevicePtr, *const SysDeviceCallbacks) -> c_int;
pub(super) type FnSysDeviceClose = unsafe extern "C" fn(*mut DevicePtr) -> c_int;

pub(super) type FnFsapiVolumeMount = unsafe extern "C" fn(
    DevicePtr,
    u8, /* sys_bool read_only */
    *const c_void,
    *mut VolumePtr,
    *mut NodePtr,
    *mut c_void,
) -> c_int;
pub(super) type FnFsapiVolumeUnmount = unsafe extern "C" fn(*mut VolumePtr) -> c_int;
pub(super) type FnFsapiNodeLookup = unsafe extern "C" fn(
    VolumePtr,
    NodePtr,
    *const u8,
    usize,
    *mut NodePtr,
    *mut FsapiNodeAttributes,
) -> c_int;
pub(super) type FnFsapiNodeRelease = unsafe extern "C" fn(VolumePtr, *mut NodePtr, usize) -> c_int;
pub(super) type FnFsapiNodeList = unsafe extern "C" fn(
    VolumePtr,
    NodePtr,
    *mut FsapiNodeAttributes,
    *mut c_void,
    unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut FsapiNodeAttributes) -> c_int,
) -> c_int;
pub(super) type FnFsapiNodeListStreams = unsafe extern "C" fn(
    VolumePtr,
    NodePtr,
    *mut c_void,
    unsafe extern "C" fn(*mut c_void, *const u8, usize, u64, u8) -> c_int,
) -> c_int;
pub(super) type FnFsapiNodeReadStream = unsafe extern "C" fn(
    VolumePtr,
    NodePtr,
    *const u8,
    usize,
    u64,
    usize,
    *mut SysIohandler,
    *mut u64,
) -> c_int;
pub(super) type FnFsapiNodeGetAttributes =
    unsafe extern "C" fn(VolumePtr, NodePtr, *mut FsapiNodeAttributes) -> c_int;
pub(super) type FnFsapiNodeRead =
    unsafe extern "C" fn(VolumePtr, NodePtr, u64, usize, *mut SysIohandler) -> c_int;
pub(super) type FnFsapiVolumeWalkTree = unsafe extern "C" fn(
    VolumePtr,
    c_int, /* fsapi_node_attribute_types requested */
    *mut c_void,
    unsafe extern "C" fn(
        *mut c_void,
        u64, /* parent_object_id */
        u64, /* object_id (0 for non-dirs) */
        *const u8,
        usize,
        *const FsapiNodeAttributes,
    ) -> c_int,
    /* handle_stream: fired per named stream, correlated to the owning entry */
    unsafe extern "C" fn(
        *mut c_void,
        u64,       /* owner_parent_object_id */
        *const u8, /* owner_name */
        usize,
        *const u8, /* stream_name */
        usize,
        u64, /* stream_size */
        u8,  /* is_snapshot */
    ) -> c_int,
) -> c_int;

pub(super) type FnIohandlerBufferHandleIo =
    unsafe extern "C" fn(*mut c_void, *mut c_void, u64, usize) -> c_int;
pub(super) type FnIohandlerBufferCopyData =
    unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> c_int;
