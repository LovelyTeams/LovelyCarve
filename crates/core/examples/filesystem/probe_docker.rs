//! Quick probe over a Docker / OCI image archive.
//!
//! Usage: cargo run --example probe_docker -- <archive.tar> [path-to-read]

use std::env;

use lovelycarve_core::evidence::readers::docker_image::{DockerImage, EntryKind};

fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let archive = args
        .next()
        .expect("usage: probe_docker <archive.tar> [path-to-read]");
    let read_path = args.next();

    let image = DockerImage::open(&archive)?;

    println!("archive: {}", image.path().display());
    if !image.repo_tags().is_empty() {
        println!("tags:    {}", image.repo_tags().join(", "));
    }
    let cfg = image.config();
    if let Some(arch) = &cfg.architecture {
        println!("arch:    {arch}");
    }
    if let Some(os) = &cfg.os {
        println!("os:      {os}");
    }
    if let Some(rt) = &cfg.config {
        if let Some(user) = &rt.user {
            println!("user:    {user}");
        }
        if let Some(wd) = &rt.working_dir {
            println!("workdir: {wd}");
        }
        if let Some(ep) = &rt.entrypoint {
            println!("entry:   {}", ep.join(" "));
        }
        if let Some(cmd) = &rt.cmd {
            println!("cmd:     {}", cmd.join(" "));
        }
        if !rt.env.is_empty() {
            println!("env:");
            for var in &rt.env {
                println!("  {var}");
            }
        }
        if !rt.exposed_ports.is_empty() {
            println!(
                "ports:   {}",
                rt.exposed_ports
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    println!("layers:  {}", image.layers().len());
    for (i, layer) in image.layers().iter().enumerate() {
        println!(
            "  #{i:<2} {} compressed={} uncompressed={} media={}",
            layer.digest,
            human(layer.compressed_size),
            human(layer.uncompressed_size),
            layer.media_type
        );
    }

    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut symlinks = 0u64;
    let mut total_bytes = 0u64;
    for entry in image.walk() {
        match entry.kind {
            EntryKind::File | EntryKind::Hardlink => {
                files += 1;
                total_bytes += entry.size;
            }
            EntryKind::Dir => dirs += 1,
            EntryKind::Symlink => symlinks += 1,
            _ => {}
        }
    }
    println!(
        "merged:  {files} files ({}), {dirs} dirs, {symlinks} symlinks",
        human(total_bytes)
    );

    println!("root:");
    for child in image.children("") {
        let marker = match child.kind {
            EntryKind::Dir => "d",
            EntryKind::Symlink => "l",
            EntryKind::Hardlink => "h",
            EntryKind::File => "-",
            EntryKind::CharDevice => "c",
            EntryKind::BlockDevice => "b",
            EntryKind::Fifo => "p",
        };
        println!(
            "  {marker} {:>10}  {}{}",
            human(child.size),
            child.path,
            child
                .link_target
                .as_deref()
                .map(|t| format!(" -> {t}"))
                .unwrap_or_default()
        );
    }

    if let Some(path) = read_path {
        match image.read(&path, 4096) {
            Ok(bytes) => {
                println!("--- read {path} ({} bytes shown) ---", bytes.len());
                println!("{}", String::from_utf8_lossy(&bytes));
            }
            Err(err) => println!("read {path} failed: {err}"),
        }
    }

    Ok(())
}

fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}
