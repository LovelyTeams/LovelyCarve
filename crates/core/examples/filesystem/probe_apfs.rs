//! APFS container end-to-end probe. Verifies both the raw vendored `apfs`
//! crate path (open container → list root) and the lovelycarve dispatcher
//! path (`fs_tree::children_for_image` / `read_file_limited` / `file_stat`)
//! against the same image. Use to sanity-check APFS support after touching
//! either layer.
//!
//! Usage: cargo run --example probe_apfs -- <image>
use std::env;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use apfs::ApfsVolume as ContainerVolume;
use lovelycarve_core::evidence::image::PartitionKind;
use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{
    children_for_image, file_stat, read_file_limited, NodeKind,
};

fn main() -> anyhow::Result<()> {
    let path = env::args().nth(1).expect("usage: probe_apfs <image>");
    let image = Arc::new(EvidenceSession::open(&path)?);
    let info = image.info();
    println!("image: {}", info.path);
    println!("sector_size: {}", info.sector_size);

    let apfs_part = info
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::ApfsContainer)
        .expect("no APFS partition found");
    let base = apfs_part.start_lba * info.sector_size as u64;
    let length = apfs_part.length_lba * info.sector_size as u64;
    println!(
        "apfs partition #{}: offset=0x{:x} length={}",
        apfs_part.id, base, length
    );

    // 1. Raw apfs-crate path — confirms the vendored parser is reachable.
    let cursor = SliceCursor {
        image: Arc::clone(&image),
        base,
        length,
        position: 0,
    };
    let mut vol = ContainerVolume::open(cursor)?;
    let vinfo = vol.volume_info().clone();
    println!(
        "volume: {:?} block_size={} files={} dirs={} symlinks={}",
        vinfo.name, vinfo.block_size, vinfo.num_files, vinfo.num_directories, vinfo.num_symlinks
    );
    let raw_root = vol.list_directory("/")?;
    println!("raw root: {} entries", raw_root.len());
    for e in raw_root.iter().take(8) {
        println!(
            "  {:?} oid={} size={} name={:?}",
            e.kind, e.oid, e.size, e.name
        );
    }

    // 2. Dispatcher path — same flow the UI takes.
    println!();
    let part_id = format!("part:{}", apfs_part.id);
    let dispatched = children_for_image(&image, &part_id)?;
    println!(
        "dispatcher children for {part_id}: {} entries",
        dispatched.len()
    );
    for node in dispatched.iter().take(8) {
        println!(
            "  {:?} {} {} bytes id={}",
            node.kind, node.name, node.size, node.id
        );
    }

    // 3. Walk into the first directory and read the first file we can find,
    //    then call file_stat on it. This exercises parse_apfs_id end-to-end.
    let first_dir = dispatched.iter().find(|node| node.kind == NodeKind::Dir);
    if let Some(dir) = first_dir {
        let dir_children = children_for_image(&image, &dir.id)?;
        println!(
            "\nchildren of {} ({} entries):",
            dir.name,
            dir_children.len()
        );
        for node in dir_children.iter().take(6) {
            println!("  {:?} {} {} bytes", node.kind, node.name, node.size);
        }
        if let Some(file) = dir_children
            .iter()
            .find(|node| node.kind == NodeKind::File && node.size > 0 && node.size < 64 * 1024)
        {
            let bytes = read_file_limited(&image, &file.id, 64 * 1024)?;
            println!(
                "\nread {} ({} bytes requested, got {} bytes); first 16 bytes: {}",
                file.name,
                file.size,
                bytes.len(),
                bytes
                    .iter()
                    .take(16)
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let stat = file_stat(&image, &file.id)?;
            println!(
                "stat: size={} mime={} entropy={:?} mode={:?}",
                stat.size, stat.mime, stat.entropy, stat.mode
            );
        } else {
            println!(
                "\n(no small file found under {} for read/stat probe)",
                dir.name
            );
        }
    }

    // 4. Probe every root-level directory — surface ones that come back empty
    //    so we can tell apart real "no children" from listing bugs. Time each
    //    call so we can see how slow per-call B-tree walks are.
    println!("\nper-directory child counts (dispatcher, with timing):");
    for node in dispatched.iter().filter(|n| n.kind == NodeKind::Dir) {
        let t0 = std::time::Instant::now();
        match children_for_image(&image, &node.id) {
            Ok(children) => println!(
                "  {:>4} entries  {:>8.2?}  {}",
                children.len(),
                t0.elapsed(),
                node.name
            ),
            Err(err) => println!("  ERROR        {}: {err}", node.name),
        }
    }

    // 5. Dump the .Spotlight-V100 listing in detail — backend-reported children
    //    aren't appearing in the UI, so surface every field the IPC returns.
    if let Some(spotlight) = dispatched.iter().find(|n| n.name == ".Spotlight-V100") {
        println!("\n.Spotlight-V100 detailed children:");
        let children = children_for_image(&image, &spotlight.id)?;
        for node in &children {
            println!("  {node:#?}");
        }
    }

    // 6. Re-time the same dirs but reusing ONE open ContainerVolume so we can
    //    tell what fraction of per-call latency is the open dance vs. the
    //    actual B-tree walk.
    println!("\nre-timing with a single shared ContainerVolume:");
    let cursor = SliceCursor {
        image: Arc::clone(&image),
        base,
        length,
        position: 0,
    };
    let mut shared = ContainerVolume::open(cursor)?;
    for name in [
        ".fseventsd",
        ".PKInstallSandboxManager-SystemSoftware",
        ".Spotlight-V100",
        ".vol",
        "Applications",
        "bin",
        "Library",
        "Users",
        "usr",
    ] {
        let path = format!("/{name}");
        let t0 = std::time::Instant::now();
        let entries = shared.list_directory(&path)?;
        println!(
            "  {:>4} entries  {:>8.2?}  {name}",
            entries.len(),
            t0.elapsed()
        );
    }

    // 7. Reproduce the user's symptom: spawn a tight read_range loop in a
    //    background thread (simulates fs_digest's hash loop hammering the
    //    EWF Mutex) and time fs_children calls against it. If contention is
    //    the cause, listings will balloon vs. the uncontended baseline above.
    println!("\nfs_children under simulated hash contention:");
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_image = Arc::clone(&image);
    let bg_stop = Arc::clone(&stop_flag);
    let bg = std::thread::spawn(move || {
        let mut offset = 0u64;
        let total = bg_image.len();
        let mut iters = 0u64;
        while !bg_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let want = (1024 * 1024usize).min((total - offset) as usize);
            if want == 0 {
                offset = 0;
                continue;
            }
            let _ = bg_image.read_range(offset, want);
            offset += want as u64;
            iters += 1;
        }
        iters
    });
    // Give the background loop a beat to ramp up.
    std::thread::sleep(std::time::Duration::from_millis(50));
    for name in [".Spotlight-V100", "Applications", "Library", "Users"] {
        let id = format!(
            "apfs:part:{}:{}",
            apfs_part.id,
            hex::encode(format!("/{name}").as_bytes())
        );
        let t0 = std::time::Instant::now();
        let children = children_for_image(&image, &id)?;
        println!(
            "  {:>4} entries  {:>8.2?}  {name}",
            children.len(),
            t0.elapsed()
        );
    }
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let bg_iters = bg.join().expect("bg thread panicked");
    println!("(background read_range iterations: {bg_iters})");

    // 8. Same contention, but the BG thread yields after each chunk —
    //    distinguishes mutex/lock-convoy issues from OS-scheduler
    //    monopolization. If fs_children speeds up dramatically here, the
    //    real fix is to yield in the hash loop, not to swap mutex types.
    println!("\nfs_children under contention WITH yield_now per chunk:");
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_image = Arc::clone(&image);
    let bg_stop = Arc::clone(&stop_flag);
    let bg = std::thread::spawn(move || {
        let mut offset = 0u64;
        let total = bg_image.len();
        let mut iters = 0u64;
        while !bg_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let want = (1024 * 1024usize).min((total - offset) as usize);
            if want == 0 {
                offset = 0;
                continue;
            }
            let _ = bg_image.read_range(offset, want);
            offset += want as u64;
            iters += 1;
            std::thread::yield_now();
        }
        iters
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    for name in [".Spotlight-V100", "Applications", "Library", "Users"] {
        let id = format!(
            "apfs:part:{}:{}",
            apfs_part.id,
            hex::encode(format!("/{name}").as_bytes())
        );
        let t0 = std::time::Instant::now();
        let children = children_for_image(&image, &id)?;
        println!(
            "  {:>4} entries  {:>8.2?}  {name}",
            children.len(),
            t0.elapsed()
        );
    }
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let bg_iters = bg.join().expect("bg thread panicked");
    println!("(background read_range iterations: {bg_iters})");

    // 9. Realistic hash simulation: each BG iteration also does a real
    //    md5+sha256 over the chunk, so the lock release between iterations
    //    is held open for as long as fs_digest would actually hold it.
    println!("\nfs_children under contention WITH md5+sha256 work per chunk:");
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_image = Arc::clone(&image);
    let bg_stop = Arc::clone(&stop_flag);
    let bg = std::thread::spawn(move || {
        use md5::Md5;
        use sha2::{Digest, Sha256};
        let mut offset = 0u64;
        let total = bg_image.len();
        let mut iters = 0u64;
        let mut md5 = Md5::new();
        let mut sha256 = Sha256::new();
        while !bg_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let want = (1024 * 1024usize).min((total - offset) as usize);
            if want == 0 {
                offset = 0;
                continue;
            }
            if let Ok(buf) = bg_image.read_range(offset, want) {
                md5.update(&buf);
                sha256.update(&buf);
            }
            offset += want as u64;
            iters += 1;
        }
        let _ = md5.finalize();
        let _ = sha256.finalize();
        iters
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    for name in [".Spotlight-V100", "Applications", "Library", "Users"] {
        let id = format!(
            "apfs:part:{}:{}",
            apfs_part.id,
            hex::encode(format!("/{name}").as_bytes())
        );
        let t0 = std::time::Instant::now();
        let children = children_for_image(&image, &id)?;
        println!(
            "  {:>4} entries  {:>8.2?}  {name}",
            children.len(),
            t0.elapsed()
        );
    }
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let bg_iters = bg.join().expect("bg thread panicked");
    println!("(background read_range iterations: {bg_iters})");

    Ok(())
}

struct SliceCursor {
    image: Arc<EvidenceSession>,
    base: u64,
    length: u64,
    position: u64,
}

impl Read for SliceCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.position >= self.length || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let bytes = self
            .image
            .read_range(self.base + self.position, want)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let copied = bytes.len();
        buf[..copied].copy_from_slice(&bytes);
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for SliceCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(o) => o as i128,
            SeekFrom::Current(d) => self.position as i128 + d as i128,
            SeekFrom::End(d) => self.length as i128 + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}
