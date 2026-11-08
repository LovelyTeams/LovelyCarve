//! Serialized ReFS volume and node operations.
use super::*;

impl RefsVolume {
    /// Mount a ReFS volume backed by an arbitrary byte reader. `backing`
    /// is consumed and dropped (together with whatever resources it
    /// holds) when the returned `RefsVolume` is dropped.
    pub fn open(backing: Box<dyn RefsBacking>) -> anyhow::Result<Self> {
        let lib = lib()?;
        let size_bytes = backing.size();
        let state = Box::into_raw(Box::new(BackingState { backing }));
        let cb = SysDeviceCallbacks {
            user_data: state as *mut c_void,
            pread: Some(pread_trampoline),
            get_size: Some(get_size_trampoline),
            close: Some(close_trampoline),
        };

        let mut dev: DevicePtr = ptr::null_mut();
        let rc = unsafe { (lib.sys_device_open_callbacks)(&mut dev, &cb) };
        if rc != 0 {
            // The library didn't take ownership of the state on failure;
            // we must drop it ourselves.
            unsafe { drop(Box::from_raw(state)) };
            return Err(anyhow!("sys_device_open_callbacks failed: errno={rc}"));
        }

        let mut vol: VolumePtr = ptr::null_mut();
        let mut root: NodePtr = ptr::null_mut();
        let rc = unsafe {
            (lib.fsapi_volume_mount)(
                dev,
                1, /* read_only */
                ptr::null(),
                &mut vol,
                &mut root,
                ptr::null_mut(),
            )
        };
        if rc != 0 {
            // Close the device — its close_trampoline drops the state.
            let mut dev_slot = dev;
            unsafe {
                let _ = (lib.sys_device_close)(&mut dev_slot);
            }
            return Err(anyhow!("fsapi_volume_mount failed: errno={rc}"));
        }

        Ok(Self {
            inner: Mutex::new(RefsInner { vol, dev, root }),
            size_bytes,
        })
    }

    /// Length in bytes of the underlying partition slice.
    pub fn size(&self) -> u64 {
        self.size_bytes
    }

    /// Look up a node by absolute POSIX-style path (`/Users/Administrator/...`).
    /// Returns an owned handle; pair every successful lookup with
    /// `release_node` so librefs can free the entry. An empty path or
    /// `"/"` returns a fresh reference to the root.
    pub fn lookup(&self, path: &str) -> anyhow::Result<NodeHandle> {
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let bytes = path.as_bytes();
        let mut out: NodePtr = ptr::null_mut();
        let rc = unsafe {
            (lib.fsapi_node_lookup)(
                inner.vol,
                ptr::null_mut(), /* relative to root */
                bytes.as_ptr(),
                bytes.len(),
                &mut out,
                ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(anyhow!("fsapi_node_lookup({path}) failed: errno={rc}"));
        }
        if out.is_null() {
            return Err(anyhow!("fsapi_node_lookup({path}) returned NULL"));
        }
        Ok(NodeHandle { ptr: out })
    }

    /// Release a node handle returned by `lookup`. Roots returned from
    /// `fsapi_volume_mount` are managed by the volume's lifecycle and
    /// MUST NOT be released here.
    pub fn release_node(&self, handle: NodeHandle) {
        let Ok(lib) = lib() else {
            return;
        };
        let Ok(inner) = self.inner.lock() else {
            return;
        };
        let mut ptr = handle.ptr;
        if !ptr.is_null() {
            unsafe {
                let _ = (lib.fsapi_node_release)(inner.vol, &mut ptr, 1);
            }
        }
    }

    /// Get attributes for a node (size, timestamps, mode, ...).
    pub fn get_attributes(&self, node: &NodeHandle) -> anyhow::Result<NodeAttrs> {
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let mut attrs = FsapiNodeAttributes::new_requested(FSAPI_ATTR_ALL);
        let rc = unsafe { (lib.fsapi_node_get_attributes)(inner.vol, node.ptr, &mut attrs) };
        if rc != 0 {
            return Err(anyhow!("fsapi_node_get_attributes failed: errno={rc}"));
        }
        Ok(NodeAttrs::from_raw(&attrs))
    }

    /// List a directory node. The root handle from `root_node` is the
    /// canonical input for the volume root.
    pub fn list_directory(
        &self,
        node: &NodeHandle,
        capture_attrs: bool,
    ) -> anyhow::Result<Vec<DirEntry>> {
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let mut ctx = ListCtx {
            entries: Vec::new(),
            capture_attrs,
        };
        let mut attrs = FsapiNodeAttributes::new_requested(if capture_attrs {
            FSAPI_ATTR_SIZE
                | FSAPI_ATTR_INODE_NUMBER
                | FSAPI_ATTR_CREATION_TIME
                | FSAPI_ATTR_LAST_DATA_CHANGE_TIME
                | FSAPI_ATTR_LAST_DATA_ACCESS_TIME
        } else {
            0
        });
        let attrs_ptr = if capture_attrs {
            &mut attrs as *mut _
        } else {
            ptr::null_mut()
        };
        let rc = unsafe {
            (lib.fsapi_node_list)(
                inner.vol,
                node.ptr,
                attrs_ptr,
                &mut ctx as *mut _ as *mut c_void,
                list_dirent_trampoline,
            )
        };
        if rc != 0 {
            return Err(anyhow!("fsapi_node_list failed: errno={rc}"));
        }
        Ok(ctx.entries)
    }

    /// Whether the loaded `librefs-13.dll` exports `fsapi_volume_walk_tree`
    /// (the single-pass whole-tree enumeration). `false` on an older DLL, so
    /// the caller can fall back to the per-directory walk.
    pub fn supports_walk_tree(&self) -> bool {
        lib()
            .map(|l| l.fsapi_volume_walk_tree.is_some())
            .unwrap_or(false)
    }

    /// Enumerate every directory entry on the volume in a single sequential
    /// metadata pass (`fsapi_volume_walk_tree`), returning a flat edge list a
    /// caller can rebuild the tree from plus every `refsutil streamsnapshot`
    /// snapshot stream found in the same pass. Each [`WalkEntry`] carries its
    /// parent directory's object ID and — for directories — its own object ID;
    /// the volume root's children use [`REFS_ROOT_OBJECT_ID`] as the parent.
    /// Each [`WalkStream`] is attributed to its owning file by
    /// `(owner_parent_object_id, owner_name)`.
    ///
    /// This reads each metadata node at most once instead of paying one
    /// targeted B-tree descent per directory, so a whole-volume snapshot over
    /// an image-backed volume (E01 via the Rust EWF reader) goes from O(directories) random
    /// reads to one mostly-sequential pass. Returns an error if the DLL does
    /// not export the symbol (check [`supports_walk_tree`](Self::supports_walk_tree)
    /// first to fall back gracefully).
    pub fn walk_tree(&self) -> anyhow::Result<WalkResult> {
        let lib = lib()?;
        let walk = lib.fsapi_volume_walk_tree.ok_or_else(|| {
            anyhow!("librefs-13.dll does not export fsapi_volume_walk_tree (DLL too old)")
        })?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let mut ctx = WalkCtx {
            entries: Vec::new(),
            streams: Vec::new(),
        };
        let requested = FSAPI_ATTR_SIZE
            | FSAPI_ATTR_INODE_NUMBER
            | FSAPI_ATTR_CREATION_TIME
            | FSAPI_ATTR_LAST_DATA_CHANGE_TIME
            | FSAPI_ATTR_LAST_DATA_ACCESS_TIME;
        let rc = unsafe {
            walk(
                inner.vol,
                requested,
                &mut ctx as *mut _ as *mut c_void,
                walk_tree_trampoline,
                walk_stream_trampoline,
            )
        };
        if rc != 0 {
            return Err(anyhow!("fsapi_volume_walk_tree failed: errno={rc}"));
        }
        Ok(WalkResult {
            entries: ctx.entries,
            streams: ctx.streams,
        })
    }

    /// Borrow the root node for direct passing to `list_directory` /
    /// `get_attributes`. The handle is invalidated when the volume is
    /// dropped — do not call `release_node` on it.
    pub fn root_node(&self) -> anyhow::Result<NodeHandle> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        Ok(NodeHandle { ptr: inner.root })
    }

    /// Read up to `size` bytes from `node` starting at `offset`. Uses
    /// librefs's built-in `fsapi_iohandler_buffer_*` callbacks so we
    /// don't have to translate the library's chunked I/O ourselves.
    pub fn read(&self, node: &NodeHandle, offset: u64, size: usize) -> anyhow::Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; size];
        let written = self.read_into(node, offset, &mut buf)?;
        buf.truncate(written);
        Ok(buf)
    }

    /// Read up to `buf.len()` bytes from `node` at `offset` into a
    /// caller-provided buffer, returning the number of bytes written.
    /// Streaming callers (Dokan mount, recursive export) reuse one buffer
    /// across chunks instead of allocating a fresh `Vec` per read.
    pub fn read_into(
        &self,
        node: &NodeHandle,
        offset: u64,
        buf: &mut [u8],
    ) -> anyhow::Result<usize> {
        let size = buf.len();
        if size == 0 {
            return Ok(0);
        }
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let mut buf_ctx = FsapiBufferContext {
            buf: buf.as_mut_ptr() as *mut c_void,
            remaining_size: size,
            is_read: 1,
        };
        let mut handler = SysIohandler {
            context: &mut buf_ctx as *mut _ as *mut c_void,
            handle_io: Some(lib.iohandler_buffer_handle_io),
            handle_hole: None,
            copy_data: Some(lib.iohandler_buffer_copy_data),
            get_data: None,
        };
        let rc = unsafe { (lib.fsapi_node_read)(inner.vol, node.ptr, offset, size, &mut handler) };
        if rc != 0 {
            return Err(anyhow!(
                "fsapi_node_read failed at offset {offset}: errno={rc}"
            ));
        }
        // `remaining_size` decrements as the library consumes the buffer;
        // bytes written = original - remaining. Guard against a buggy handler
        // reporting more remaining than we offered, which would underflow the
        // subtraction (panic in debug, bogus length in release).
        if buf_ctx.remaining_size > size {
            return Err(anyhow!(
                "fsapi_node_read reported {} bytes remaining for a {size}-byte buffer",
                buf_ctx.remaining_size
            ));
        }
        Ok(size - buf_ctx.remaining_size)
    }
}
