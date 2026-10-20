//! Export helpers for filesystem tree nodes.
//!
//! The command layer already owns IPC and progress events; this module keeps
//! the directory / ZIP export logic out of the broad `fs_tree` dispatcher while
//! preserving the public `fs_tree::...` re-exports used by existing callers.

use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio_util::sync::CancellationToken;

use crate::core::common::export::write_new;
use crate::core::evidence::image::ReadFaultReport;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{stream_file_chunks, FsSession, NodeKind};
use crate::core::filesystem::unalloc::is_virtual_unalloc_id;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FsExportResult {
    pub bytes_written: u64,
    #[serde(flatten)]
    pub read_faults: ReadFaultReport,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FsExportSummary {
    pub file_count: u64,
    pub dir_count: u64,
    pub bytes_written: u64,
    pub skipped_count: u64,
    pub skipped_deleted: u64,
    pub truncated: bool,
    pub cancelled: bool,
    #[serde(flatten)]
    pub read_faults: ReadFaultReport,
}

const EXPORT_DIR_MAX_ENTRIES: u64 = 200_000;
const EXPORT_DIR_MAX_DEPTH: usize = 64;

pub fn export_file(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    destination: &Path,
) -> anyhow::Result<u64> {
    image.require_source()?;
    write_new(destination, |file| {
        stream_file_chunks(image, node_id, 1024 * 1024, |chunk| {
            file.write_all(chunk)?;
            Ok(())
        })
    })
}

pub fn export_file_with_fault_report(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    destination: &Path,
) -> anyhow::Result<FsExportResult> {
    let (result, read_faults) =
        image.collect_read_faults(|| export_file(image, node_id, destination));
    Ok(FsExportResult {
        bytes_written: result?,
        read_faults,
    })
}

/// Recursively writes every regular file under `node_id` into `destination`.
pub fn export_dir(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    destination: &Path,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
) -> anyhow::Result<FsExportSummary> {
    image.require_source()?;
    if node_id.trim().is_empty() {
        anyhow::bail!("export_dir requires a directory node id");
    }
    if let Some(parent) = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(destination)?;
    let mut summary = FsExportSummary::default();
    on_progress(&summary, None);
    let mut visited = HashSet::new();
    let mut session = FsSession::new(image);
    export_dir_walk(
        &mut session,
        node_id,
        destination,
        "",
        &mut summary,
        0,
        cancel,
        on_progress,
        &mut visited,
    )?;
    on_progress(&summary, None);
    Ok(summary)
}

pub fn export_dir_with_fault_report(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    destination: &Path,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
) -> anyhow::Result<FsExportSummary> {
    let (result, read_faults) =
        image.collect_read_faults(|| export_dir(image, node_id, destination, cancel, on_progress));
    let mut summary = result?;
    summary.read_faults = read_faults;
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
fn export_dir_walk(
    session: &mut FsSession<'_>,
    node_id: &str,
    target: &Path,
    rel_path: &str,
    summary: &mut FsExportSummary,
    depth: usize,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
    visited: &mut HashSet<String>,
) -> anyhow::Result<()> {
    if summary.cancelled || cancel.is_cancelled() {
        summary.cancelled = true;
        return Ok(());
    }
    if summary.file_count + summary.dir_count >= EXPORT_DIR_MAX_ENTRIES {
        summary.truncated = true;
        return Ok(());
    }
    if depth > EXPORT_DIR_MAX_DEPTH {
        summary.truncated = true;
        return Ok(());
    }
    if !visited.insert(node_id.to_string()) {
        return Ok(());
    }
    let children = session.children(node_id)?;
    tracing::trace!(
        "export_dir_walk node={node_id} depth={depth} children={} target={:?}",
        children.len(),
        target,
    );
    for child in children {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        if summary.file_count + summary.dir_count >= EXPORT_DIR_MAX_ENTRIES {
            summary.truncated = true;
            break;
        }
        if child.deleted {
            tracing::trace!(
                "export_dir: skip deleted id={} name={:?} kind={:?}",
                child.id,
                child.name,
                child.kind,
            );
            summary.skipped_deleted += 1;
            continue;
        }
        if is_virtual_unalloc_id(&child.id) {
            summary.skipped_count += 1;
            continue;
        }
        let safe = sanitize_export_component(&child.name);
        let child_path = unique_export_path(target, &safe);
        let child_rel = if rel_path.is_empty() {
            safe.clone()
        } else {
            format!("{rel_path}/{safe}")
        };
        on_progress(summary, Some(&child_rel));
        match child.kind {
            NodeKind::Dir => {
                if let Err(err) = fs::create_dir(&child_path) {
                    tracing::warn!("export_dir: mkdir {:?} failed: {err}", child_path);
                    summary.skipped_count += 1;
                    continue;
                }
                tracing::trace!(
                    "export_dir: mkdir id={} name={:?} path={:?}",
                    child.id,
                    child.name,
                    child_path,
                );
                summary.dir_count += 1;
                if let Err(err) = export_dir_walk(
                    session,
                    &child.id,
                    &child_path,
                    &child_rel,
                    summary,
                    depth + 1,
                    cancel,
                    on_progress,
                    visited,
                ) {
                    tracing::warn!("export_dir: walk {} failed: {err}", child.id);
                    summary.skipped_count += 1;
                }
                if summary.cancelled {
                    break;
                }
            }
            NodeKind::File => {
                let mut bytes_written = 0u64;
                let res = write_new(&child_path, |file| {
                    session.stream_file(&child.id, 1024 * 1024, |chunk| {
                        if cancel.is_cancelled() {
                            anyhow::bail!("export cancelled");
                        }
                        file.write_all(chunk)?;
                        bytes_written += chunk.len() as u64;
                        Ok(())
                    })
                });
                match res {
                    Ok(_) => {
                        tracing::trace!(
                            "export_dir: file id={} name={:?} bytes={bytes_written} path={:?}",
                            child.id,
                            child.name,
                            child_path,
                        );
                        summary.file_count += 1;
                        summary.bytes_written += bytes_written;
                    }
                    Err(err) => {
                        if cancel.is_cancelled() {
                            tracing::debug!(
                                "export_dir: cancelled during stream id={} name={:?} bytes={bytes_written}",
                                child.id,
                                child.name,
                            );
                            summary.cancelled = true;
                            break;
                        }
                        tracing::warn!(
                            "export_dir: stream id={} name={:?} failed: {err}",
                            child.id,
                            child.name,
                        );
                        summary.skipped_count += 1;
                    }
                }
            }
            NodeKind::Symlink | NodeKind::Unallocated => {
                tracing::trace!(
                    "export_dir: skip kind={:?} id={} name={:?}",
                    child.kind,
                    child.id,
                    child.name,
                );
                summary.skipped_count += 1;
            }
        }
    }
    Ok(())
}

/// Recursively packages every regular file under `node_id` into a single ZIP
/// archive.
pub fn export_dir_zip(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    archive_path: &Path,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
) -> anyhow::Result<FsExportSummary> {
    image.require_source()?;
    if node_id.trim().is_empty() {
        anyhow::bail!("export_dir_zip requires a directory node id");
    }
    if let Some(parent) = archive_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    write_new(archive_path, |file| {
        let mut writer = zip::ZipWriter::new(file);
        let file_opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let dir_opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o755);

        let mut summary = FsExportSummary::default();
        let mut taken: HashSet<String> = HashSet::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut session = FsSession::new(image);
        on_progress(&summary, None);

        zip_walk(
            &mut session,
            node_id,
            "",
            &mut writer,
            &file_opts,
            &dir_opts,
            &mut summary,
            &mut taken,
            0,
            cancel,
            on_progress,
            &mut visited,
        )?;
        on_progress(&summary, None);

        let inner = writer.finish()?;
        inner.flush()?;
        Ok(summary)
    })
}

pub fn export_dir_zip_with_fault_report(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    archive_path: &Path,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
) -> anyhow::Result<FsExportSummary> {
    let (result, read_faults) = image
        .collect_read_faults(|| export_dir_zip(image, node_id, archive_path, cancel, on_progress));
    let mut summary = result?;
    summary.read_faults = read_faults;
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
fn zip_walk(
    session: &mut FsSession<'_>,
    node_id: &str,
    rel_path: &str,
    writer: &mut zip::ZipWriter<&mut File>,
    file_opts: &zip::write::SimpleFileOptions,
    dir_opts: &zip::write::SimpleFileOptions,
    summary: &mut FsExportSummary,
    taken: &mut HashSet<String>,
    depth: usize,
    cancel: &CancellationToken,
    on_progress: &dyn Fn(&FsExportSummary, Option<&str>),
    visited: &mut HashSet<String>,
) -> anyhow::Result<()> {
    if summary.cancelled || cancel.is_cancelled() {
        summary.cancelled = true;
        return Ok(());
    }
    if summary.file_count + summary.dir_count >= EXPORT_DIR_MAX_ENTRIES {
        summary.truncated = true;
        return Ok(());
    }
    if depth > EXPORT_DIR_MAX_DEPTH {
        summary.truncated = true;
        return Ok(());
    }
    if !visited.insert(node_id.to_string()) {
        return Ok(());
    }
    let children = session.children(node_id)?;
    tracing::trace!(
        "zip_walk node={node_id} depth={depth} children={} rel={rel_path:?}",
        children.len(),
    );
    for child in children {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        if summary.file_count + summary.dir_count >= EXPORT_DIR_MAX_ENTRIES {
            summary.truncated = true;
            break;
        }
        if child.deleted {
            tracing::trace!(
                "export_dir_zip: skip deleted id={} name={:?} kind={:?}",
                child.id,
                child.name,
                child.kind,
            );
            summary.skipped_deleted += 1;
            continue;
        }
        if is_virtual_unalloc_id(&child.id) {
            summary.skipped_count += 1;
            continue;
        }
        let safe = sanitize_export_component(&child.name);
        let raw_rel = if rel_path.is_empty() {
            safe.clone()
        } else {
            format!("{rel_path}/{safe}")
        };
        let child_rel = unique_zip_path(taken, &raw_rel);
        taken.insert(child_rel.clone());
        on_progress(summary, Some(&child_rel));

        match child.kind {
            NodeKind::Dir => {
                let dir_entry = format!("{child_rel}/");
                if let Err(err) = writer.add_directory(dir_entry.as_str(), *dir_opts) {
                    tracing::warn!("export_dir_zip: add_dir {dir_entry:?} failed: {err}");
                    summary.skipped_count += 1;
                    continue;
                }
                tracing::trace!("export_dir_zip: dir id={} entry={dir_entry:?}", child.id);
                summary.dir_count += 1;
                if let Err(err) = zip_walk(
                    session,
                    &child.id,
                    &child_rel,
                    writer,
                    file_opts,
                    dir_opts,
                    summary,
                    taken,
                    depth + 1,
                    cancel,
                    on_progress,
                    visited,
                ) {
                    tracing::warn!("export_dir_zip: walk {} failed: {err}", child.id);
                    summary.skipped_count += 1;
                }
                if summary.cancelled {
                    break;
                }
            }
            NodeKind::File => {
                if let Err(err) = writer.start_file(child_rel.as_str(), *file_opts) {
                    tracing::warn!("export_dir_zip: start_file {child_rel:?} failed: {err}");
                    summary.skipped_count += 1;
                    continue;
                }
                let mut bytes_written = 0u64;
                let res = session.stream_file(&child.id, 1024 * 1024, |chunk| {
                    if cancel.is_cancelled() {
                        anyhow::bail!("export cancelled");
                    }
                    writer.write_all(chunk)?;
                    bytes_written += chunk.len() as u64;
                    Ok(())
                });
                match res {
                    Ok(_) => {
                        tracing::trace!(
                            "export_dir_zip: file id={} entry={child_rel:?} bytes={bytes_written}",
                            child.id,
                        );
                        summary.file_count += 1;
                        summary.bytes_written += bytes_written;
                    }
                    Err(err) => {
                        if cancel.is_cancelled() {
                            tracing::debug!(
                                "export_dir_zip: cancelled during stream id={} entry={child_rel:?} bytes={bytes_written}",
                                child.id,
                            );
                            summary.bytes_written += bytes_written;
                            summary.cancelled = true;
                            break;
                        }
                        tracing::warn!(
                            "export_dir_zip: stream id={} entry={child_rel:?} failed: {err}",
                            child.id,
                        );
                        summary.skipped_count += 1;
                    }
                }
            }
            NodeKind::Symlink | NodeKind::Unallocated => {
                tracing::trace!(
                    "export_dir_zip: skip kind={:?} id={} name={:?}",
                    child.kind,
                    child.id,
                    child.name,
                );
                summary.skipped_count += 1;
            }
        }
    }
    Ok(())
}

fn unique_zip_path(taken: &HashSet<String>, raw: &str) -> String {
    if !taken.contains(raw) {
        return raw.to_string();
    }
    let (stem, ext) = match raw.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (raw.to_string(), String::new()),
    };
    for n in 1..=1024 {
        let candidate = format!("{stem}-{n}{ext}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    raw.to_string()
}

pub(crate) fn sanitize_export_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (ch as u32) < 0x20
        {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return "_".to_string();
    }
    // Win32 device names (`CON`, `aux.c`, `nul.txt`, `COM1`…) cannot be
    // created as regular files regardless of extension — Linux evidence hits
    // this routinely (e.g. `aux.c` in kernel trees). Prefix to keep the name
    // recognisable while making it a legal file name on every host.
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    if is_windows_device_name(stem) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn is_windows_device_name(stem: &str) -> bool {
    let upper = stem.trim_end_matches(' ').to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|port| {
                matches!(
                    port,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            }),
    }
}

fn unique_export_path(target: &Path, name: &str) -> PathBuf {
    let candidate = target.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..=1024 {
        let next = target.join(format!("{stem}-{n}{ext}"));
        if !next.exists() {
            return next;
        }
    }
    // Never fall back to the colliding path — that would overwrite an earlier
    // exported file. A random suffix is ugly but always distinct.
    target.join(format!("{stem}-{}{ext}", uuid::Uuid::new_v4().simple()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_exports_refuse_existing_destinations_without_cli_prechecks() {
        let directory = tempfile::tempdir().unwrap();
        let evidence = directory.path().join("evidence");
        fs::create_dir(&evidence).unwrap();
        fs::write(evidence.join("hello.txt"), b"authentic").unwrap();
        let image = Arc::new(EvidenceSession::open(&evidence).unwrap());
        let destination = directory.path().join("existing");
        fs::write(&destination, b"preserve").unwrap();
        let cancel = CancellationToken::new();
        let root = "folder:";
        assert!(export_dir_zip(&image, root, &destination, &cancel, &|_, _| {}).is_err());
        assert!(export_file(&image, "folder:68656c6c6f2e747874", &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"preserve");
        let tree = directory.path().join("tree");
        fs::create_dir(&tree).unwrap();
        assert!(export_dir(&image, root, &tree, &cancel, &|_, _| {}).is_err());
        assert_eq!(fs::read_dir(tree).unwrap().count(), 0);
    }

    #[test]
    fn invalid_single_file_export_leaves_no_partial_and_allows_retry() {
        let directory = tempfile::tempdir().unwrap();
        let evidence = directory.path().join("evidence");
        fs::create_dir(&evidence).unwrap();
        fs::write(evidence.join("hello.txt"), b"authentic").unwrap();
        let image = Arc::new(EvidenceSession::open(&evidence).unwrap());
        let destination = directory.path().join("retry");
        assert!(export_file(&image, "folder:", &destination).is_err());
        assert!(!destination.exists());
        export_file(&image, "folder:68656c6c6f2e747874", &destination).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"authentic");
    }

    #[test]
    fn preserves_leading_dot_for_hidden_files() {
        assert_eq!(sanitize_export_component(".local"), ".local");
        assert_eq!(sanitize_export_component(".bashrc"), ".bashrc");
        assert_eq!(sanitize_export_component(".ssh"), ".ssh");
        assert_eq!(sanitize_export_component(".config"), ".config");
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_export_component("foo."), "foo");
        assert_eq!(sanitize_export_component("foo.. "), "foo");
        assert_eq!(sanitize_export_component("name   "), "name");
    }

    #[test]
    fn replaces_windows_reserved_chars() {
        assert_eq!(sanitize_export_component(r"a<b>c"), "a_b_c");
        assert_eq!(
            sanitize_export_component("a:b\"c|d?e*f/g\\h"),
            "a_b_c_d_e_f_g_h"
        );
    }

    #[test]
    fn prefixes_windows_device_names() {
        assert_eq!(sanitize_export_component("aux.c"), "_aux.c");
        assert_eq!(sanitize_export_component("CON"), "_CON");
        assert_eq!(sanitize_export_component("nul.tar.gz"), "_nul.tar.gz");
        assert_eq!(sanitize_export_component("com1"), "_com1");
        assert_eq!(sanitize_export_component("com10"), "com10");
        assert_eq!(sanitize_export_component("console.log"), "console.log");
        assert_eq!(sanitize_export_component("auxiliary"), "auxiliary");
    }

    #[test]
    fn collapses_empty_or_dots_only() {
        assert_eq!(sanitize_export_component(""), "_");
        assert_eq!(sanitize_export_component("."), "_");
        assert_eq!(sanitize_export_component(".."), "_");
        assert_eq!(sanitize_export_component("   "), "_");
    }
}
