//! End-to-end probe of the `filedb` metadata index layer on a real image.
//!
//! Builds a `.dirz` snapshot for one volume, imports it into a temporary index
//! file, then exercises the metadata query paths:
//! natural-order paging, counter cache, lowercase-column filters, MetaAddr
//! round trips and a content read through the resolved address.
//!
//! Usage:
//!
//! ```text
//! just run-example probe_filedb <image> [volume_key] [max_entries]
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use lovelycarve_core::catalog::filedb::filedb_query::{self, FileFilter};
use lovelycarve_core::catalog::filedb::filedb_store::{self, IndexSource};
use lovelycarve_core::catalog::snapshot::volume_snapshot::{
    build_volume_snapshot, snapshot_file_name, DEFAULT_MAX_SNAPSHOT_ENTRIES,
};
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::fs_all::FsAll;
use lovelycarve_core::filesystem::meta_addr::{self, MetaAddrKind, IS_DELETE_DELETED};
use lovelycarve_core::filesystem::tree::fs_tree_read;
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .context("usage: probe_filedb <image> [volume_key] [max_entries]")?;
    let volume_key = args.next().unwrap_or_else(|| "part:0".to_string());
    let max_entries: usize = args
        .next()
        .map(|value| value.parse())
        .transpose()
        .context("max_entries must be an integer")?
        .unwrap_or(DEFAULT_MAX_SNAPSHOT_ENTRIES);

    let image = Arc::new(EvidenceSession::open(&path)?);
    println!(
        "image: {path} ({:?}) id={}",
        image.info().source_format,
        image.id
    );

    let fs_all = FsAll::for_image(&image);
    println!("fs_all ({} entries):", fs_all.entries().len());
    for entry in fs_all.entries() {
        println!(
            "  vol_id={} key={:<12} kind={:?} prefix={} offset={:#x} size={}",
            entry.vol_id, entry.volume_key, entry.kind, entry.fs_prefix, entry.offset, entry.size
        );
    }
    let vol_id = fs_all
        .vol_id_of(&volume_key)
        .with_context(|| format!("volume {volume_key} not in fs_all"))?;
    println!("volume_key={volume_key} -> vol_id={vol_id}");

    let work_dir = tempfile::tempdir()?;
    let snapshot_path = work_dir
        .path()
        .join(snapshot_file_name(&image.id, &volume_key));
    let cancel = CancellationToken::new();
    let mut last = 0u64;
    let mut on_progress = |count: u64| {
        if count >= last + 100_000 {
            last = count;
            println!("  .. walked {count} entries");
        }
    };
    let started = Instant::now();
    let built = build_volume_snapshot(
        &image,
        &volume_key,
        &snapshot_path,
        max_entries,
        "2026-09-14T00:00:00Z".to_string(),
        &cancel,
        &mut on_progress,
    )?;
    println!(
        "snapshot: {} entries truncated={} in {:.1}s",
        built.entry_count,
        built.truncated,
        started.elapsed().as_secs_f64()
    );

    let index = work_dir.path().join("index.db");
    let started = Instant::now();
    let rows = filedb_store::build_index(
        &snapshot_path,
        &index,
        &IndexSource {
            image_id: &image.id,
            volume_key: &volume_key,
            vol_id,
            built_at: &built.header.built_at,
            fingerprint: &image.evidence_fingerprint(),
            truncated: built.truncated,
        },
        None,
        None,
    )?;
    println!(
        "index import: {rows} rows in {:.1}s ({} MiB)",
        started.elapsed().as_secs_f64(),
        std::fs::metadata(&index)?.len() / (1024 * 1024)
    );

    probe_browsing(&index, &volume_key)?;
    probe_filters(&index)?;
    probe_meta_addr(&index, &image, vol_id)?;
    Ok(())
}

fn probe_browsing(index: &Path, volume_key: &str) -> anyhow::Result<()> {
    let started = Instant::now();
    let page = filedb_query::children_page(index, volume_key, None, Some(12))?
        .context("root should be covered")?;
    println!(
        "root page: total={:?} first {} in {:.2}ms (natural order):",
        page.total,
        page.items.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    for node in &page.items {
        println!("  {:<40} {:?} {}", node.name, node.kind, node.id);
    }
    let counts = filedb_query::child_counts(
        index,
        &page
            .items
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>(),
    )?;
    let mut shown = 0;
    for node in &page.items {
        if let Some(count) = counts.get(&node.id) {
            println!(
                "  child_counts {:<30} count={} deleted={} inexact={}",
                node.name, count.count, count.deleted, count.inexact
            );
            shown += 1;
        }
    }
    println!("  ({shown} directories had cached counts)");
    Ok(())
}

fn probe_filters(index: &Path) -> anyhow::Result<()> {
    let cases: Vec<(&str, FileFilter)> = vec![
        (
            "extension=conf",
            FileFilter {
                extensions: vec!["CONF".to_string()],
                limit: 5,
                ..FileFilter::default()
            },
        ),
        (
            "name_contains=PASSWD",
            FileFilter {
                name_contains: Some("PASSWD".to_string()),
                limit: 5,
                ..FileFilter::default()
            },
        ),
        (
            "path_prefix=/ETC/SSH/",
            FileFilter {
                path_prefix: Some("/ETC/SSH/".to_string()),
                limit: 5,
                ..FileFilter::default()
            },
        ),
        (
            "size>=100MiB",
            FileFilter {
                min_size: Some(100 * 1024 * 1024),
                kinds: vec!["file".to_string()],
                limit: 5,
                ..FileFilter::default()
            },
        ),
        (
            "is_delete=1",
            FileFilter {
                is_delete: Some(IS_DELETE_DELETED),
                limit: 5,
                ..FileFilter::default()
            },
        ),
    ];
    for (label, filter) in cases {
        let started = Instant::now();
        let hits = filedb_query::query_files(index, &filter.normalized())?;
        println!(
            "query {label}: {} hits in {:.2}ms",
            hits.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        for hit in hits.iter().take(3) {
            println!(
                "  {:<50} size={} is_delete={} nid={}",
                hit.full_path, hit.size, hit.is_delete, hit.nid
            );
        }
    }
    Ok(())
}

fn probe_meta_addr(index: &Path, image: &Arc<EvidenceSession>, vol_id: i64) -> anyhow::Result<()> {
    let sample = filedb_query::query_files(
        index,
        &FileFilter {
            path_prefix: Some("/etc/".to_string()),
            kinds: vec!["file".to_string()],
            min_size: Some(1),
            limit: 200,
            ..FileFilter::default()
        }
        .normalized(),
    )?;
    let mut mismatches = 0usize;
    let mut path_addressed = 0usize;
    let started = Instant::now();
    for hit in &sample {
        let kind = meta_addr::classify(&hit.nid);
        let (addr, high) = kind.columns();
        if kind == MetaAddrKind::Path {
            path_addressed += 1;
        }
        let resolved =
            filedb_query::node_id_for_meta_addr(index, addr, high, Some(&hit.full_path))?;
        if resolved.as_deref() != Some(hit.nid.as_str()) {
            mismatches += 1;
            if mismatches <= 3 {
                println!(
                    "  MISMATCH nid={} addr=({addr},{high}) resolved={resolved:?}",
                    hit.nid
                );
            }
        }
    }
    println!(
        "meta_addr round trip: {} sampled, {} path-addressed, {} mismatches in {:.1}ms",
        sample.len(),
        path_addressed,
        mismatches,
        started.elapsed().as_secs_f64() * 1000.0
    );

    let Some(target) = sample
        .iter()
        .find(|hit| hit.full_path.eq_ignore_ascii_case("/etc/hostname"))
        .or_else(|| sample.first())
    else {
        println!("no /etc files to read");
        return Ok(());
    };
    let (addr, high) = meta_addr::classify(&target.nid).columns();
    let nid = filedb_query::node_id_for_meta_addr(index, addr, high, Some(&target.full_path))?
        .context("address should resolve")?;
    let bytes = fs_tree_read::read_file_limited(image, &nid, 256)?;
    println!(
        "read via MetaAddr ({}, vol {vol_id}, {addr}, {high}) -> {} : {} bytes, head={:?}",
        image.id,
        target.full_path,
        bytes.len(),
        String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap_or_default()
    );
    Ok(())
}
