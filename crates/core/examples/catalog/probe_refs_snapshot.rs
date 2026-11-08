//! Verify ReFS file-snapshot recovery via the new librefs stream APIs.
//!
//! Usage:
//!   cargo run --example probe_refs_snapshot -- <image>
//!
//! Loads `<image>`, locates the first ReFS partition, walks to
//! `\Users\Administrator\secrets\secret_012.docx`, lists its named
//! streams, finds the `snapshot_sec` stream, reads it fully and prints
//! the md5 — should match the ground-truth `5a2d346f1c49f9cc24b56d984997e24e`
//! derived from raw block extraction.

use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::volumes::refs::{open_refs_source, RefsSource};
use md5::Digest;
use md5::Md5;

const TARGET_PATH: &str = "/Users/Administrator/secrets/secret_012.docx";
const SNAPSHOT_STREAM_NAME: &str = "snapshot_sec";
const EXPECTED_MD5: &str = "5a2d346f1c49f9cc24b56d984997e24e";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let image_path = env::args()
        .nth(1)
        .expect("usage: probe_refs_snapshot <image>");
    let image = Arc::new(DiskImage::open(&image_path)?);
    let part = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Refs)
        .expect("no ReFS partition detected");
    println!("opening ReFS on partition #{}", part.id);

    let volume = open_refs_source(&image, RefsSource::Partition(part.id))?;

    let streams = volume.list_streams(TARGET_PATH)?;
    println!("streams on {TARGET_PATH}: {}", streams.len());
    for s in &streams {
        println!(
            "  name={:?} size={} resident={}",
            s.name, s.size, s.resident
        );
    }

    let snapshot_stream = streams
        .iter()
        .find(|s| s.name == SNAPSHOT_STREAM_NAME)
        .ok_or_else(|| {
            anyhow::anyhow!("stream `{SNAPSHOT_STREAM_NAME}` not found on {TARGET_PATH}")
        })?;
    println!(
        "reading snapshot stream `{SNAPSHOT_STREAM_NAME}` ({} bytes)...",
        snapshot_stream.size
    );

    let bytes = volume.read_named_stream(
        TARGET_PATH,
        SNAPSHOT_STREAM_NAME,
        snapshot_stream.size as usize,
    )?;
    println!("read {} bytes", bytes.len());
    assert_eq!(
        bytes.len() as u64,
        snapshot_stream.size,
        "stream read short: got {} bytes, expected {}",
        bytes.len(),
        snapshot_stream.size
    );

    let mut hasher = Md5::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    println!("md5: {hex}");
    println!("expected md5: {EXPECTED_MD5}");
    if hex == EXPECTED_MD5 {
        println!("VERIFY OK — snapshot bytes match ground truth.");
    } else {
        anyhow::bail!("md5 mismatch: got {hex}, expected {EXPECTED_MD5}");
    }

    let head = &bytes[..bytes.len().min(16)];
    let hex_head = head
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!("first 16 bytes (expect ZIP magic 50 4b 03 04): {hex_head}");

    Ok(())
}
