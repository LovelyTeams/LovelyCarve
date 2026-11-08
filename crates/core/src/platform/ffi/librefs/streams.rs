//! Serialized named-stream enumeration and reads.
use super::*;

impl RefsVolume {
    /// Enumerate the named data streams attached to `node` — regular
    /// ADS plus the `:$SNAPSHOT` streams created by
    /// `refsutil streamsnapshot /c`. The default `:$DATA` stream is not
    /// included (read it via `read`).
    pub fn list_streams(&self, node: &NodeHandle) -> anyhow::Result<Vec<StreamInfo>> {
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let mut ctx = StreamListCtx {
            entries: Vec::new(),
        };
        let rc = unsafe {
            (lib.fsapi_node_list_streams)(
                inner.vol,
                node.ptr,
                &mut ctx as *mut _ as *mut c_void,
                list_streams_trampoline,
            )
        };
        if rc != 0 {
            return Err(anyhow!("fsapi_node_list_streams failed: errno={rc}"));
        }
        Ok(ctx.entries)
    }

    /// Read up to `size` bytes from the named data stream `stream_name`
    /// of `node`, starting at `offset` within the stream. Pairs with
    /// `list_streams` for snapshot recovery and forensic ADS dumping.
    pub fn read_stream(
        &self,
        node: &NodeHandle,
        stream_name: &str,
        offset: u64,
        size: usize,
    ) -> anyhow::Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let lib = lib()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RefsVolume mutex poisoned"))?;
        let name_bytes = stream_name.as_bytes();
        let mut buf = vec![0u8; size];
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
        let mut stream_size: u64 = 0;
        let rc = unsafe {
            (lib.fsapi_node_read_stream)(
                inner.vol,
                node.ptr,
                name_bytes.as_ptr(),
                name_bytes.len(),
                offset,
                size,
                &mut handler,
                &mut stream_size,
            )
        };
        if rc != 0 {
            return Err(anyhow!(
                "fsapi_node_read_stream({stream_name}) failed at offset {offset}: errno={rc}"
            ));
        }
        if buf_ctx.remaining_size > size {
            return Err(anyhow!(
                "fsapi_node_read_stream({stream_name}) reported {} bytes remaining for a {size}-byte buffer",
                buf_ctx.remaining_size
            ));
        }
        let written = size - buf_ctx.remaining_size;
        buf.truncate(written);
        Ok(buf)
    }
}
