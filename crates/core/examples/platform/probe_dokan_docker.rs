//! Direct Dokan-mount probe for the Docker / OCI tar fast-path.
//!
//! Usage: cargo run --example probe_dokan_docker -- <archive.tar> <drive_letter>
//!
//! Example: cargo run --example probe_dokan_docker -- C:\path\faka.tar M
//!
//! Mounts read-only on `<drive_letter>:\`, sleeps 30 seconds (so you can
//! click around in Explorer), then unmounts. Dokan DEBUG / STDERR flags
//! are forced on so kernel-side complaints land on the console.

#![cfg(target_os = "windows")]

use std::env;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::platform::dokan_mount;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,lovelycarve=debug")),
        )
        .with_writer(std::io::stderr)
        .init();

    let archive = env::args()
        .nth(1)
        .expect("usage: probe_dokan_docker <archive.tar> <drive_letter>");
    let drive = env::args()
        .nth(2)
        .expect("usage: probe_dokan_docker <archive.tar> <drive_letter>");

    println!("opening {archive}");
    let image = Arc::new(EvidenceSession::open(&archive)?);
    let info = image.info();
    println!(
        "format: {:?}, size: {} bytes",
        info.source_format, info.size_bytes
    );
    println!("docker.is_some: {}", image.docker.is_some());
    if let Some(docker) = image.docker.as_ref() {
        println!("layers: {}", docker.layers().len());
        println!("root entries: {}", docker.children("").len());
        println!("repo tags: {:?}", docker.repo_tags());
    }

    println!("\nmounting on {drive}:\\ ...");
    let session = dokan_mount::start_readonly_mount(
        "probe-id".to_string(),
        info.id.clone(),
        Arc::clone(&image),
        "docker:".to_string(),
        format!("{drive}:\\"),
    )?;
    println!("mount ready: {:?}", session.view());
    println!("sleeping 30s — open {drive}:\\ in Explorer to test");
    thread::sleep(Duration::from_secs(30));
    println!("unmounting...");
    session.stop()?;
    println!("done");
    Ok(())
}
