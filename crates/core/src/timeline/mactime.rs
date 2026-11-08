//! Forensic timeline export — TSK `fls -m` / `mactime` body-file generation.
//!
//! Walks a detected filesystem (the whole image or a chosen subtree) and emits
//! one pipe-delimited line per node in the standard mactime body-file format:
//!
//! ```text
//! MD5|name|inode|mode_as_string|UID|GID|size|atime|mtime|ctime|crtime
//! ```
//!
//! Times are decimal Unix epoch seconds (`0` when unknown), matching what
//! `log2timeline`/`plaso` and `mactime` consume. The MD5 column is `0` — node
//! walking never reads file content, so no hash is available here (a hashed
//! body file would require a separate full-read pass).
//! `UID`/`GID` are `0`: [`FsNode`] carries no ownership.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{FsNode, FsSession, Mactb, NodeKind};

/// Summary returned to the caller once the body-file is fully written.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MactimeSummary {
    pub lines_written: u64,
    pub bytes_written: u64,
    pub cancelled: bool,
}

/// `ls -l`-style mode string a body file expects, e.g. `d/drwxr-xr-x` or
/// `r/rrwxr-xr-x`. The leading letter encodes the entry type; lovelycarve does
/// not carry POSIX permission bits for every filesystem, so the permission
/// triplet is a conventional default per kind.
fn mode_string(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "d/drwxr-xr-x",
        NodeKind::File => "r/rrwxr-xr-x",
        NodeKind::Symlink => "l/lrwxrwxrwx",
        NodeKind::Unallocated => "r/rrwxr-xr-x",
    }
}

/// Body-file epoch column: decimal Unix seconds, or `0` when the timestamp is
/// absent / unparseable.
fn epoch_col(value: Option<i64>) -> i64 {
    value.unwrap_or(0)
}

/// Builds one body-file line for `node` at `path`. `crtime` prefers the
/// dedicated `birth`/`created` field; `ctime` maps to NTFS `$MFT`-changed /
/// inode-change time.
fn body_line(node: &FsNode, path: &str) -> String {
    let ts: &Mactb = &node.timestamps;
    let atime = epoch_col(ts.accessed_epoch());
    let mtime = epoch_col(ts.modified_epoch());
    let ctime = epoch_col(ts.mft_changed_epoch());
    let crtime = epoch_col(ts.birth_epoch().or_else(|| ts.created_epoch()));
    let inode = node.inode.unwrap_or(0);
    // The body file is pipe-delimited and line-delimited, so a name containing
    // a literal '|' or a line break would corrupt the record (splitting one
    // entry into several, or breaking field alignment) and make the output
    // unparseable by mactime/log2timeline. Both are legal in filenames on the
    // supported filesystems, so neutralize them defensively.
    let safe_path: String = path
        .chars()
        .map(|c| match c {
            '|' => '_',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect();
    format!(
        "0|{name}|{inode}|{mode}|0|0|{size}|{atime}|{mtime}|{ctime}|{crtime}",
        name = safe_path,
        inode = inode,
        mode = mode_string(node.kind),
        size = node.size,
        atime = atime,
        mtime = mtime,
        ctime = ctime,
        crtime = crtime,
    )
}

/// Walks `start_id`'s subtree (empty = whole image) and invokes `on_line` for
/// each body-file record. `on_line` returns the number of bytes it wrote so
/// the caller can accumulate a byte count; returning `Err` aborts the walk.
/// Honors `cancel` between nodes.
pub fn export_body_file<F>(
    image: &Arc<EvidenceSession>,
    start_id: &str,
    cancel: &CancellationToken,
    mut on_line: F,
) -> anyhow::Result<MactimeSummary>
where
    F: FnMut(&str) -> anyhow::Result<u64>,
{
    let mut walker = FsSession::new(image);
    let mut summary = MactimeSummary::default();
    let mut visit =
        |entry: &crate::filesystem::tree::fs_tree::FsWalkEntry| -> anyhow::Result<bool> {
            if cancel.is_cancelled() {
                summary.cancelled = true;
                return Ok(false);
            }
            let line = body_line(&entry.node, &entry.path);
            summary.bytes_written += on_line(&line)?;
            summary.lines_written += 1;
            Ok(true)
        };
    walker.walk_visit_from(start_id, &mut visit)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::tree::fs_tree::Mactb;

    fn node(name: &str, kind: NodeKind) -> FsNode {
        FsNode {
            id: "x".into(),
            name: name.into(),
            parent_id: None,
            kind,
            size: 1234,
            deleted: false,
            inode: Some(42),
            timestamps: Mactb {
                modified: Some("2021-01-01T00:00:00Z".into()),
                ..Default::default()
            },
            link_target: None,
            has_snapshot: false,
        }
    }

    #[test]
    fn body_line_has_eleven_pipe_fields_and_numeric_times() {
        let line = body_line(&node("file.txt", NodeKind::File), "/a/file.txt");
        let cols: Vec<&str> = line.split('|').collect();
        assert_eq!(cols.len(), 11, "mactime body file has 11 columns");
        assert_eq!(cols[0], "0"); // MD5 absent
        assert_eq!(cols[1], "/a/file.txt"); // name
        assert_eq!(cols[2], "42"); // inode
        assert_eq!(cols[6], "1234"); // size
        assert_eq!(cols[8], "1609459200"); // mtime as epoch seconds, not a string
    }

    #[test]
    fn body_line_neutralizes_line_breaks_and_pipes_in_names() {
        // A filename carrying a newline / CR / pipe must not corrupt the
        // line- and pipe-delimited body-file format.
        let line = body_line(&node("x", NodeKind::File), "/evil\nname\rwith|pipe");
        assert!(!line.contains('\n'), "newline must be neutralized");
        assert!(!line.contains('\r'), "carriage return must be neutralized");
        // Exactly the 10 structural pipes remain — none injected by the name.
        assert_eq!(line.matches('|').count(), 10);
        assert!(line.contains("/evil name with_pipe"));
    }
}
