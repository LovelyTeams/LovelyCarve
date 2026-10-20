//! Named-stream helpers for filesystem tree nodes.
//!
//! The command layer already owns stream IPC in `commands::filesystem::filesystem_streams`;
//! this module keeps the core NTFS ADS / ReFS stream read and export wrappers
//! out of the broad `fs_tree` dispatcher while preserving the public
//! `fs_tree::...` re-export used by existing callers.

use std::{io::Write, path::Path, sync::Arc};

use crate::core::common::export::write_new;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::open_ntfs_for_source;
use crate::core::filesystem::volumes::ntfs::{parse_ntfs_id, NtfsDataStream};
use crate::core::filesystem::volumes::refs::{open_refs_source, parse_refs_id};

/// Lists every `$DATA` stream attached to an NTFS file (the unnamed default
/// stream + every named ADS). Returns an empty Vec for non-NTFS nodes so
/// the IPC layer can call this unconditionally and let the frontend decide
/// whether to render the streams section.
pub fn list_data_streams(
    image: &Arc<EvidenceSession>,
    node_id: &str,
) -> anyhow::Result<Vec<NtfsDataStream>> {
    let image = image.filesystem_session(node_id);
    if let Some((source, path)) = parse_refs_id(node_id) {
        return open_refs_source(image, source)?.list_streams(&path);
    }
    let Some((source, record)) = parse_ntfs_id(node_id) else {
        return Ok(Vec::new());
    };
    open_ntfs_for_source(image, source)?.list_data_streams(record)
}

/// Reads up to `max_bytes` from a single named `$DATA` stream of an NTFS
/// file. `stream_name == ""` is rejected here on purpose — callers should
/// use `read_file_limited` / `read_file` to get the default stream so
/// the regular file-read code path is hit.
pub fn read_named_stream(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    stream_name: &str,
    max_bytes: usize,
) -> anyhow::Result<Vec<u8>> {
    let image = image.filesystem_session(node_id);
    if stream_name.is_empty() {
        anyhow::bail!("named-stream API requires a non-empty stream name");
    }
    if let Some((source, path)) = parse_refs_id(node_id) {
        return open_refs_source(image, source)?.read_named_stream(&path, stream_name, max_bytes);
    }
    let (source, record) = parse_ntfs_id(node_id).ok_or_else(|| {
        anyhow::anyhow!("alternate data streams are only supported on NTFS and ReFS")
    })?;
    open_ntfs_for_source(image, source)?.read_data_stream(record, stream_name, max_bytes)
}

/// Streams a named NTFS `$DATA` stream to disk. Mirrors `export_file` but
/// uses `NtfsVolume::stream_data_stream` under the hood so a multi-GB ADS
/// can be exported without buffering it in memory.
pub fn export_named_stream(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    stream_name: &str,
    destination: &Path,
) -> anyhow::Result<u64> {
    let image = image.filesystem_session(node_id);
    if stream_name.is_empty() {
        anyhow::bail!("named-stream API requires a non-empty stream name");
    }
    if let Some((source, path)) = parse_refs_id(node_id) {
        return write_new(destination, |file| {
            open_refs_source(image, source)?.stream_named_stream(
                &path,
                stream_name,
                1024 * 1024,
                |chunk| {
                    file.write_all(chunk)?;
                    Ok(())
                },
            )
        });
    }
    let (source, record) = parse_ntfs_id(node_id).ok_or_else(|| {
        anyhow::anyhow!("alternate data streams are only supported on NTFS and ReFS")
    })?;
    write_new(destination, |file| {
        open_ntfs_for_source(image, source)?.stream_data_stream(
            record,
            stream_name,
            1024 * 1024,
            |chunk| {
                file.write_all(chunk)?;
                Ok(())
            },
        )
    })
}
