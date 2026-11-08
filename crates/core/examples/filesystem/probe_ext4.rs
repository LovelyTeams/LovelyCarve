//! Quick ext4-volume probe. Usage: cargo run --example probe_ext4 -- <image>
use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::volumes::ext4::{open_ext4_source, Ext4Source};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let path = env::args().nth(1).expect("usage: probe_ext4 <image>");
    let image = Arc::new(DiskImage::open(&path)?);
    println!("partitions: {}", image.partitions.len());
    for p in &image.partitions {
        println!(
            "  #{} kind={:?} start_lba={} length_lba={}",
            p.id, p.kind, p.start_lba, p.length_lba
        );
    }
    let part = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Ext4)
        .expect("no ext4 partition detected");
    println!("opening ext4 on partition #{}", part.id);
    match open_ext4_source(&image, Ext4Source::Partition(part.id)) {
        Ok(vol) => {
            let root = vol.children(2, false)?;
            println!("root entries: {}", root.len());
            for n in root.iter().take(20) {
                println!("  {:>10}  {:?}  {}", n.size, n.kind, n.name);
            }
        }
        Err(err) => {
            println!("ext4 open FAILED: {err:#}");
        }
    }
    Ok(())
}
