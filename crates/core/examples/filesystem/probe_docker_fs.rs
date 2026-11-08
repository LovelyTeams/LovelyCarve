//! End-to-end probe: open a docker tar through EvidenceSession and walk via fs_tree.
//!
//! Usage: cargo run --example probe_docker_fs -- <archive.tar>

use std::env;
use std::sync::Arc;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, file_stat, read_file_limited,
};

fn main() -> anyhow::Result<()> {
    let path = env::args()
        .nth(1)
        .expect("usage: probe_docker_fs <archive.tar>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("format: {:?}", info.source_format);
    println!("partitions: {}", info.partitions.len());

    let roots = children_for_image(&image, "")?;
    println!("synthetic roots: {}", roots.len());
    for node in &roots {
        println!("  id={} name={:?} kind={:?}", node.id, node.name, node.kind);
    }

    let docker_root = roots.first().expect("a docker root").clone();
    let children = children_for_image(&image, &docker_root.id)?;
    println!("/ children: {}", children.len());
    for node in children.iter().take(5) {
        println!(
            "  id={} name={:?} kind={:?} size={}",
            node.id, node.name, node.kind, node.size
        );
    }

    if let Some(etc) = children.iter().find(|n| n.name == "etc") {
        let etc_children = children_for_image(&image, &etc.id)?;
        println!("/etc children: {} (showing 8)", etc_children.len());
        for node in etc_children.iter().take(8) {
            println!(
                "  id={} name={:?} kind={:?} size={}",
                node.id, node.name, node.kind, node.size
            );
        }
        if let Some(target) = etc_children
            .iter()
            .find(|n| n.name == "os-release" || n.name == "alpine-release")
        {
            let bytes = read_file_limited(&image, &target.id, 4096)?;
            println!("--- read {} ({} bytes) ---", target.name, bytes.len());
            print!("{}", String::from_utf8_lossy(&bytes));
            let stat = file_stat(&image, &target.id)?;
            println!(
                "stat: size={} mode={:?} mime={}",
                stat.size, stat.mode, stat.mime
            );
        }
    }

    Ok(())
}
