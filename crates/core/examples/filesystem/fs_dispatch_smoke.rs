//! Read-only behavioural fingerprint of the filesystem dispatch surface, for
//! the P0-10 trait/registry refactor golden-diff. Walks every volume to a
//! bounded entry count and, per file, samples `read_file_limited`, `file_stat`
//! and `stream_file_chunks`, then repeats the walk through a `SnapshotWalker`.
//! Emits a deterministic, sorted summary so the same image produces identical
//! output before and after the refactor (diff must be empty).
//!
//! Usage: cargo run --example fs_dispatch_smoke -- <image> [max_entries]
//!   e.g. cargo run --example fs_dispatch_smoke -- D:\VM\api.E01 5000

use std::collections::BTreeMap;
use std::sync::Arc;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, file_stat, read_file_limited, stream_file_chunks, FsNode, FsWalkEntry,
    NodeKind, SnapshotWalker,
};

/// A cheap order-independent fingerprint: FNV-1a over the bytes fed in. Avoids
/// pulling a hash crate and is stable across runs/platforms.
#[derive(Default, Clone, Copy)]
struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf29ce484222325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
        self.write(&[0]);
    }
}

#[derive(Default)]
struct Summary {
    files: u64,
    dirs: u64,
    symlinks: u64,
    unalloc: u64,
    total_size: u128,
    read_bytes: u128,
    stream_bytes: u128,
    read_errors: u64,
    stat_errors: u64,
    stream_errors: u64,
    /// node-id -> short fingerprint of (read sample + stat fields).
    per_node: BTreeMap<String, u64>,
    kinds: BTreeMap<String, u64>,
}

fn classify(node: &FsNode) -> &'static str {
    match node.kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Symlink => "symlink",
        NodeKind::Unallocated => "unalloc",
    }
}

/// Sample one file: read 4096 bytes, stat, stream-count. Fold everything into a
/// per-node fingerprint plus aggregate counters. Errors are counted, never
/// fatal — a behavioural diff in the error set is itself signal.
fn sample_file(image: &Arc<EvidenceSession>, entry: &FsWalkEntry, s: &mut Summary) {
    let id = &entry.node.id;
    let mut fp = Fnv::new();
    fp.write_str(&entry.path);
    fp.write(&entry.node.size.to_le_bytes());
    fp.write(&[entry.node.deleted as u8]);

    match read_file_limited(image, id, 4096) {
        Ok(buf) => {
            s.read_bytes += buf.len() as u128;
            let mut h = Fnv::new();
            h.write(&buf);
            fp.write(&h.0.to_le_bytes());
        }
        Err(_) => {
            s.read_errors += 1;
            fp.write_str("READ_ERR");
        }
    }

    match file_stat(image, id) {
        Ok(st) => {
            fp.write(&st.size.to_le_bytes());
            fp.write_str(&st.mime);
            fp.write_str(st.magic_hex.as_deref().unwrap_or(""));
            // entropy rounded to 3 dp so float formatting can't perturb the diff
            let ent = (st.entropy.unwrap_or(0.0) * 1000.0).round() as i64;
            fp.write(&ent.to_le_bytes());
            fp.write(&(st.data_runs.len() as u64).to_le_bytes());
            for run in &st.data_runs {
                fp.write(&run.logical_offset.to_le_bytes());
                fp.write(&run.physical_offset.to_le_bytes());
                fp.write(&run.length.to_le_bytes());
            }
        }
        Err(_) => {
            s.stat_errors += 1;
            fp.write_str("STAT_ERR");
        }
    }

    let mut streamed = 0u128;
    match stream_file_chunks(image, id, 64 * 1024, |chunk| {
        streamed += chunk.len() as u128;
        Ok(())
    }) {
        Ok(_) => s.stream_bytes += streamed,
        Err(_) => {
            s.stream_errors += 1;
            fp.write_str("STREAM_ERR");
        }
    }
    fp.write(&streamed.to_le_bytes());

    s.per_node.insert(id.clone(), fp.0);
}

/// Recursive DFS via `children_for_image`, bounded by `max`. Mirrors the
/// dispatcher's own cycle guard so a corrupt tree can't loop forever.
fn walk_generic(
    image: &Arc<EvidenceSession>,
    id: &str,
    depth: usize,
    count: &mut u64,
    max: u64,
    visited: &mut std::collections::HashSet<String>,
    s: &mut Summary,
) {
    if *count >= max || depth > 64 || !visited.insert(id.to_string()) {
        return;
    }
    let children = match children_for_image(image, id) {
        Ok(c) => c,
        Err(_) => return,
    };
    for child in children {
        if *count >= max {
            return;
        }
        *count += 1;
        *s.kinds.entry(classify(&child).to_string()).or_default() += 1;
        match child.kind {
            NodeKind::Dir => {
                s.dirs += 1;
                let child_id = child.id.clone();
                walk_generic(image, &child_id, depth + 1, count, max, visited, s);
            }
            NodeKind::File => {
                s.files += 1;
                s.total_size += child.size as u128;
                let entry = FsWalkEntry {
                    path: child.id.clone(),
                    node: child,
                };
                sample_file(image, &entry, s);
            }
            NodeKind::Symlink => s.symlinks += 1,
            NodeKind::Unallocated => s.unalloc += 1,
        }
    }
}

fn print_summary(label: &str, s: &Summary) {
    let mut nodes_fp = Fnv::new();
    for (id, fp) in &s.per_node {
        nodes_fp.write_str(id);
        nodes_fp.write(&fp.to_le_bytes());
    }
    println!("=== {label} ===");
    println!(
        "files={} dirs={} symlinks={} unalloc={} total_size={}",
        s.files, s.dirs, s.symlinks, s.unalloc, s.total_size
    );
    println!(
        "read_bytes={} stream_bytes={} read_err={} stat_err={} stream_err={}",
        s.read_bytes, s.stream_bytes, s.read_errors, s.stat_errors, s.stream_errors
    );
    print!("kinds:");
    for (k, n) in &s.kinds {
        print!(" {k}={n}");
    }
    println!();
    println!(
        "sampled_nodes={} nodes_fingerprint={:016x}",
        s.per_node.len(),
        nodes_fp.0
    );
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: fs_dispatch_smoke <image> [max_entries]");
    let max: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5000);

    let image = Arc::new(EvidenceSession::open(&path)?);
    println!("image: {} ({:?})", path, image.info().source_format);
    println!("max_entries: {max}");

    // Pass 1 — generic free-function dispatch.
    let mut generic = Summary::default();
    let mut count = 0u64;
    let mut visited = std::collections::HashSet::new();
    walk_generic(&image, "", 0, &mut count, max, &mut visited, &mut generic);
    print_summary("generic", &generic);

    // Pass 2 — SnapshotWalker (cached volume handles + its own children/stream).
    let mut walker = SnapshotWalker::new(&image);
    let mut walker_fp = Fnv::new();
    let mut walker_files = 0u64;
    let mut walker_dirs = 0u64;
    let mut walker_stream_bytes = 0u128;
    walker.walk_visit(&mut |entry: &FsWalkEntry| {
        walker_fp.write_str(&entry.path);
        walker_fp.write(&entry.node.size.to_le_bytes());
        match entry.node.kind {
            NodeKind::Dir => walker_dirs += 1,
            NodeKind::File => walker_files += 1,
            _ => {}
        }
        Ok(true)
    })?;
    // Stream a bounded sample of files through the walker's cached path.
    let mut streamed_via_walker = 0u64;
    for (id, _) in generic.per_node.iter().take(256) {
        let mut n = 0u128;
        if walker
            .stream_file(id, 64 * 1024, |chunk| {
                n += chunk.len() as u128;
                Ok(())
            })
            .is_ok()
        {
            walker_stream_bytes += n;
            streamed_via_walker += 1;
        }
    }
    println!("=== walker ===");
    println!("walker_files={walker_files} walker_dirs={walker_dirs}");
    println!(
        "walker_tree_fingerprint={:016x} streamed_files={streamed_via_walker} walker_stream_bytes={walker_stream_bytes}",
        walker_fp.0
    );

    Ok(())
}
