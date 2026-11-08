//! Dynamic library loading and symbol lifetime ownership.
use super::*;

// ---------- Loaded library ----------

pub(super) struct LibRefs {
    pub(super) _lib: Library,
    pub(super) sys_device_open_callbacks: FnSysDeviceOpenCallbacks,
    pub(super) sys_device_close: FnSysDeviceClose,
    pub(super) fsapi_volume_mount: FnFsapiVolumeMount,
    pub(super) fsapi_volume_unmount: FnFsapiVolumeUnmount,
    pub(super) fsapi_node_lookup: FnFsapiNodeLookup,
    pub(super) fsapi_node_release: FnFsapiNodeRelease,
    pub(super) fsapi_node_list: FnFsapiNodeList,
    pub(super) fsapi_node_list_streams: FnFsapiNodeListStreams,
    pub(super) fsapi_node_get_attributes: FnFsapiNodeGetAttributes,
    pub(super) fsapi_node_read: FnFsapiNodeRead,
    pub(super) fsapi_node_read_stream: FnFsapiNodeReadStream,
    /// Whole-tree single-pass enumeration. `None` when loaded against a DLL
    /// that predates the `fsapi_volume_walk_tree` export — callers then fall
    /// back to the per-directory walk.
    pub(super) fsapi_volume_walk_tree: Option<FnFsapiVolumeWalkTree>,
    pub(super) iohandler_buffer_handle_io: FnIohandlerBufferHandleIo,
    pub(super) iohandler_buffer_copy_data: FnIohandlerBufferCopyData,
}

unsafe impl Send for LibRefs {}
unsafe impl Sync for LibRefs {}

pub(super) static LIB: OnceLock<LibRefs> = OnceLock::new();

pub(super) fn lib() -> anyhow::Result<&'static LibRefs> {
    if let Some(lib) = LIB.get() {
        return Ok(lib);
    }
    let loaded = LibRefs::load()?;
    Ok(LIB.get_or_init(|| loaded))
}

impl LibRefs {
    pub(super) fn load() -> anyhow::Result<Self> {
        let library = open_library()?;
        unsafe {
            let sys_device_open_callbacks = *library
                .get::<FnSysDeviceOpenCallbacks>(b"sys_device_open_callbacks\0")
                .context("sys_device_open_callbacks symbol missing — DLL too old?")?;
            let sys_device_close = *library
                .get::<FnSysDeviceClose>(b"sys_device_close\0")
                .context("sys_device_close symbol missing")?;
            let fsapi_volume_mount = *library
                .get::<FnFsapiVolumeMount>(b"fsapi_volume_mount\0")
                .context("fsapi_volume_mount symbol missing")?;
            let fsapi_volume_unmount = *library
                .get::<FnFsapiVolumeUnmount>(b"fsapi_volume_unmount\0")
                .context("fsapi_volume_unmount symbol missing")?;
            let fsapi_node_lookup = *library
                .get::<FnFsapiNodeLookup>(b"fsapi_node_lookup\0")
                .context("fsapi_node_lookup symbol missing")?;
            let fsapi_node_release = *library
                .get::<FnFsapiNodeRelease>(b"fsapi_node_release\0")
                .context("fsapi_node_release symbol missing")?;
            let fsapi_node_list = *library
                .get::<FnFsapiNodeList>(b"fsapi_node_list\0")
                .context("fsapi_node_list symbol missing")?;
            let fsapi_node_list_streams = *library
                .get::<FnFsapiNodeListStreams>(b"fsapi_node_list_streams\0")
                .context(
                    "fsapi_node_list_streams symbol missing — DLL pre-dates the snapshot patch?",
                )?;
            let fsapi_node_get_attributes = *library
                .get::<FnFsapiNodeGetAttributes>(b"fsapi_node_get_attributes\0")
                .context("fsapi_node_get_attributes symbol missing")?;
            let fsapi_node_read = *library
                .get::<FnFsapiNodeRead>(b"fsapi_node_read\0")
                .context("fsapi_node_read symbol missing")?;
            let fsapi_node_read_stream = *library
                .get::<FnFsapiNodeReadStream>(b"fsapi_node_read_stream\0")
                .context(
                    "fsapi_node_read_stream symbol missing — DLL pre-dates the snapshot patch?",
                )?;
            let iohandler_buffer_handle_io = *library
                .get::<FnIohandlerBufferHandleIo>(b"fsapi_iohandler_buffer_handle_io\0")
                .context("fsapi_iohandler_buffer_handle_io symbol missing")?;
            let iohandler_buffer_copy_data = *library
                .get::<FnIohandlerBufferCopyData>(b"fsapi_iohandler_buffer_copy_data\0")
                .context("fsapi_iohandler_buffer_copy_data symbol missing")?;
            // Optional: present only on DLLs carrying the whole-tree walk
            // export. Absence is not an error — the caller falls back to the
            // per-directory enumeration.
            let fsapi_volume_walk_tree = library
                .get::<FnFsapiVolumeWalkTree>(b"fsapi_volume_walk_tree\0")
                .map(|sym| *sym)
                .ok();

            Ok(Self {
                _lib: library,
                sys_device_open_callbacks,
                sys_device_close,
                fsapi_volume_mount,
                fsapi_volume_unmount,
                fsapi_node_lookup,
                fsapi_node_release,
                fsapi_node_list,
                fsapi_node_list_streams,
                fsapi_node_get_attributes,
                fsapi_node_read,
                fsapi_node_read_stream,
                fsapi_volume_walk_tree,
                iohandler_buffer_handle_io,
                iohandler_buffer_copy_data,
            })
        }
    }
}

pub(super) fn open_library() -> anyhow::Result<Library> {
    let candidates = dll_search_path("librefs-13.dll");
    let mut last_error: Option<libloading::Error> = None;
    let mut tried: Vec<PathBuf> = Vec::new();
    for candidate in &candidates {
        tried.push(candidate.clone());
        match unsafe { Library::new(candidate) } {
            Ok(library) => return Ok(library),
            Err(error) => last_error = Some(error),
        }
    }
    let listing = tried
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(match last_error {
        Some(error) => anyhow!("failed to load librefs-13.dll: {error} (tried: {listing})"),
        None => anyhow!("failed to load librefs-13.dll: no candidate paths"),
    })
}
