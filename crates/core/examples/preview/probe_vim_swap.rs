//! Drive the vim_swap parser against a real image.
//! Usage: cargo run --example probe_vim_swap -- <image> <node_id>
//!
//! `node_id` is the backend id (e.g. `part:2/root/.env.swp`). If you
//! don't know it, run `probe_ext4` first to confirm the partition, then
//! walk the tree via `children_for_image`.

use std::env;
use std::sync::Arc;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree;
use lovelycarve_core::preview::vim_swap::preview_vim_swap;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = env::args().skip(1);
    let image_path = args
        .next()
        .expect("usage: probe_vim_swap <image> [node_id]");
    let explicit = args.next();

    let image = Arc::new(EvidenceSession::open(&image_path)?);
    println!("partitions: {}", image.partitions.len());
    for p in &image.partitions {
        println!(
            "  #{} kind={:?} start_lba={} length_lba={}",
            p.id, p.kind, p.start_lba, p.length_lba
        );
    }

    let node_id = match explicit {
        Some(id) => id,
        None => discover_swap(&image)?
            .ok_or_else(|| anyhow::anyhow!("no .swp under /root on any ext4 partition"))?,
    };
    println!("\nopening swap at: {node_id}");

    let digest = preview_vim_swap(&image, &node_id)?;
    println!("meta:");
    println!("  version    = {}", digest.meta.version);
    println!("  page_size  = {}", digest.meta.page_size);
    println!(
        "  mtime      = {} ({})",
        digest.meta.mtime,
        digest.meta.mtime_iso.as_deref().unwrap_or("—")
    );
    println!("  inode      = {}", digest.meta.inode);
    println!("  pid        = {}", digest.meta.pid);
    println!("  user@host  = {} @ {}", digest.meta.user, digest.meta.host);
    println!("  fname      = {}", digest.meta.fname);
    println!(
        "blocks: data={} ptr={} other={} encrypted={}",
        digest.data_block_count,
        digest.pointer_block_count,
        digest.unknown_block_count,
        digest.encrypted
    );
    println!(
        "bytes read: {}{}",
        digest.bytes_read,
        if digest.truncated { " (truncated)" } else { "" }
    );
    if !digest.notes.is_empty() {
        println!("notes:");
        for note in &digest.notes {
            println!("  - {note}");
        }
    }
    println!(
        "\nrecovered lines ({}) — byte order (newest-first within each data block):",
        digest.lines.len()
    );
    for (idx, line) in digest.lines.iter().enumerate().take(200) {
        println!("  {:>4}  {line}", idx + 1);
    }
    if digest.lines.len() > 200 {
        println!("  ... ({} more)", digest.lines.len() - 200);
    }
    Ok(())
}

fn discover_swap(image: &Arc<EvidenceSession>) -> anyhow::Result<Option<String>> {
    for part in &image.partitions {
        let part_root = format!("part:{}", part.id);
        let Ok(level1) = fs_tree::children_for_image(image, &part_root) else {
            continue;
        };
        for top in level1 {
            if top.name != "root" {
                continue;
            }
            let Ok(level2) = fs_tree::children_for_image(image, &top.id) else {
                continue;
            };
            for entry in level2 {
                if entry.name.ends_with(".swp") {
                    return Ok(Some(entry.id));
                }
            }
        }
    }
    Ok(None)
}
