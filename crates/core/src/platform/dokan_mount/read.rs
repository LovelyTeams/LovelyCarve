//! Bounded file reads and per-handle read-window caching.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

/// Cache-assisted Dokan read. Dokan typically calls `read_file` with 64 KiB
/// buffers. A cache miss uses the filesystem's seekable range reader where
/// available; other filesystems retain the bounded stream-and-skip fallback.
/// The last 1 MiB window per handle serves nearby reads from RAM, and a seek
/// outside that window replaces it without caching a failed or short read.
pub(super) fn read_node_range_cached(
    image: &Arc<EvidenceSession>,
    node: &FsNode,
    cache: &Mutex<Option<ChunkCache>>,
    offset: u64,
    buffer: &mut [u8],
) -> anyhow::Result<u32> {
    if buffer.is_empty() || offset >= node.size {
        return Ok(0);
    }
    {
        let guard = cache.lock();
        if let Some(chunk) = guard.as_ref() {
            if offset >= chunk.start && offset < chunk.start + chunk.data.len() as u64 {
                let cache_off = (offset - chunk.start) as usize;
                let avail = chunk.data.len() - cache_off;
                let want = avail.min(buffer.len());
                if want > 0 {
                    buffer[..want].copy_from_slice(&chunk.data[cache_off..cache_off + want]);
                    return Ok(want as u32);
                }
            }
        }
    }

    let remaining = node.size.saturating_sub(offset);
    let window_size = (DOKAN_STREAM_CHUNK as u64).min(remaining) as usize;
    let window = fs_tree::read_file_range(image, &node.id, offset, window_size)?;
    anyhow::ensure!(
        window.len() == window_size,
        "short Dokan read for {} at {offset}: expected {window_size} bytes, got {}",
        node.id,
        window.len()
    );
    let want = window.len().min(buffer.len());
    if want > 0 {
        buffer[..want].copy_from_slice(&window[..want]);
    }
    *cache.lock() = Some(ChunkCache {
        start: offset,
        data: window,
    });
    Ok(want as u32)
}
