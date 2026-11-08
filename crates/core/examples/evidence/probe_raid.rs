//! Cross-image RAID auto-detection probe.
//!
//! Usage: just run-example probe_raid [--member-signatures] <image1> [<image2> ...]
//!
//! Loads each image through the same `DiskImage::open` path the GUI uses,
//! then runs `core::evidence::raid::detect` against the full set and pretty-
//! prints the resulting LVM/mdadm/btrfs stack.
use std::env;
use std::sync::Arc;

use lovelycarve_core::evidence::image::DiskImage;
use lovelycarve_core::evidence::raid;

fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let inspect = arguments
        .first()
        .is_some_and(|arg| arg == "--member-signatures");
    let mut args = arguments.iter().skip(usize::from(inspect)).peekable();
    if args.peek().is_none() {
        eprintln!("usage: probe_raid <image1> [<image2> ...]");
        std::process::exit(2);
    }

    let mut images = Vec::new();
    for path in args {
        println!("loading: {path}");
        let image = DiskImage::open(&path)?;
        let info = image.info();
        println!(
            "  format={:?} size={} sectors={} partitions={}",
            info.source_format,
            human(info.size_bytes),
            info.size_bytes / info.sector_size as u64,
            info.partitions.len()
        );
        for p in &info.partitions {
            println!(
                "    #{id:<2} kind={kind:<10} start_lba={slba:<12} length_lba={llba:<12}",
                id = p.id,
                kind = format!("{:?}", p.kind),
                slba = p.start_lba,
                llba = p.length_lba,
            );
        }
        if inspect {
            // Read-only diagnostics for erased md headers and large data reserves.
            for sector in [0, 8, 128, (image.len() / 512).saturating_sub(16)] {
                let bytes = image.read_range(sector * 512, 4096)?;
                println!(
                    "    sector {sector}: {:02x?}",
                    &bytes[..bytes.len().min(32)]
                );
            }
            for sector in (0..=512 * 1024).step_by(1024) {
                if sector * 512 + 2048 > image.len() {
                    break;
                }
                let bytes = image.read_range(sector * 512, 2048)?;
                if bytes.get(0x438..0x43a) == Some(&[0x53, 0xef]) {
                    println!(
                        "    ext superblock at data sector {sector}: uuid={:02x?} blocks={:02x?}",
                        bytes.get(0x468..0x478),
                        bytes.get(0x404..0x408)
                    );
                    println!("    ext geometry first_block={:02x?} block_log={:02x?} blocks_per_group={:02x?} features={:02x?} backup_groups={:02x?}",
                        &bytes[0x414..0x418], &bytes[0x418..0x41c], &bytes[0x420..0x424],
                        &bytes[0x45c..0x468], &bytes[0x64c..0x654]);
                }
            }
        }
        images.push(Arc::new(image));
    }

    if inspect {
        return Ok(());
    }

    println!();
    println!("== running RAID auto-detection ==");
    let stack = raid::detect(&images);

    println!();
    println!("images:");
    for img in &stack.images {
        println!(
            "  - {} ({} bytes, {} partitions, {} LVM PVs)",
            img.path, img.size_bytes, img.partition_count, img.lvm_pv_count
        );
    }

    println!();
    println!("volume groups: {}", stack.volume_groups.len());
    for vg in &stack.volume_groups {
        println!(
            "  VG {} (uuid {}, extent_size={} sectors)",
            vg.vg_name, vg.vg_uuid, vg.extent_size_sectors
        );
        for member in &vg.members {
            println!(
                "    PV {} = image {} part #{} (pe_start={} sectors, partition {} bytes)",
                member.pv_local_id,
                shorten(&member.image_id),
                member.partition_id,
                member.pe_start_sectors,
                member.partition_byte_length
            );
        }
        for lv in &vg.logical_volumes {
            println!(
                "    LV {}/{}: {} ({} segments)",
                vg.vg_name,
                lv.name,
                human(lv.size_bytes),
                lv.segments.len()
            );
            for seg in &lv.segments {
                println!(
                    "      seg pv={} image={} part={} offset=0x{:x} length={}",
                    seg.pv_local_id,
                    shorten(&seg.image_id),
                    seg.partition_id,
                    seg.byte_offset,
                    human(seg.byte_length)
                );
            }
        }
    }

    println!();
    println!("mdadm arrays: {}", stack.arrays.len());
    for arr in &stack.arrays {
        println!(
            "  set_uuid={} name={:?} level=RAID{} chunk={} sectors raid_disks={} data_offset={} sectors total={}",
            arr.set_uuid,
            arr.set_name,
            arr.level,
            arr.chunksize_sectors,
            arr.raid_disks,
            arr.data_offset_sectors,
            human(arr.assembled_bytes)
        );
        for m in &arr.members {
            let origin = match &m.origin {
                lovelycarve_core::evidence::raid::RaidMemberOrigin::Lv {
                    vg_name,
                    lv_name,
                    size_bytes,
                    ..
                } => format!("LV {vg_name}/{lv_name} ({})", human(*size_bytes)),
                lovelycarve_core::evidence::raid::RaidMemberOrigin::Whole {
                    image_id,
                    partition_id,
                    byte_offset,
                    byte_length,
                } => format!(
                    "image {} part {} @0x{:x} ({})",
                    shorten(image_id),
                    partition_id,
                    byte_offset,
                    human(*byte_length)
                ),
            };
            let slot = m
                .slot
                .map(|s| format!("slot {s}"))
                .unwrap_or_else(|| "slot ?".to_string());
            println!(
                "    dev{} ({slot}) uuid={} data_size={} sectors origin={}",
                m.dev_number, m.device_uuid, m.data_size_sectors, origin
            );
        }
    }

    println!();
    println!(
        "filesystems on assembled arrays: {}",
        stack.filesystems.len()
    );
    for fs in &stack.filesystems {
        println!(
            "  {} on set {} fsid={:?} total={:?} used={:?} num_devices={:?} sectorsize={:?} nodesize={:?}",
            fs.kind,
            fs.set_uuid,
            fs.fsid,
            fs.total_bytes.map(human),
            fs.bytes_used.map(human),
            fs.num_devices,
            fs.sector_size,
            fs.node_size,
        );
    }

    if !stack.diagnostics.is_empty() {
        println!();
        println!("diagnostics:");
        for line in &stack.diagnostics {
            println!("  ! {line}");
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

fn shorten(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…", &id[..12])
    }
}
