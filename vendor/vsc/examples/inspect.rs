//! Read-only vshadowinfo-style inspection of a raw NTFS volume.
//! Usage: just probe-vsc <volume.raw> [--blocks] [--hash]

use std::fs::File;
use std::io::Read;

use sha2::{Digest, Sha256};
use vsc::VssVolume;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: inspect <raw NTFS volume> [--blocks] [--hash]")?;
    let mut show_blocks = false;
    let mut hash_snapshots = false;
    for argument in arguments {
        match argument.to_str() {
            Some("--blocks") => show_blocks = true,
            Some("--hash") => hash_snapshots = true,
            _ => return Err("unknown option; use --blocks or --hash".into()),
        }
    }
    let mut volume = VssVolume::open(File::open(path)?)?;
    println!("source_size: {}", volume.volume_size());
    println!("volume_header: {:#?}", volume.header());
    if !volume.has_vss_header() {
        return Ok(());
    }
    println!("stores: {}", volume.store_count());
    if volume.store_count() == 0 {
        return Ok(());
    }
    println!("snapshot_size: {}", volume.snapshot_size()?);
    for index in 0..volume.store_count() {
        let descriptor = &volume.stores()[index];
        println!("store {index}: {}", descriptor.store_id_string());
        println!("  descriptor: {descriptor:#?}");
        println!("  in_volume_data: {}", descriptor.has_in_volume_data());
        if !descriptor.has_in_volume_data() {
            continue;
        }
        let info = volume.store_info(index)?;
        println!("  copy_identifier: {}", info.shadow_copy_id_string());
        println!(
            "  copy_set_identifier: {}",
            info.shadow_copy_set_id_string()
        );
        println!("  information: {info:#?}");
        if show_blocks {
            let blocks = volume.store_blocks(index)?;
            println!("  raw_block_count: {}", blocks.len());
            for (number, block) in blocks.iter().enumerate() {
                println!("  block {number}: {block:?}");
            }
            println!("  ranges: {:#?}", volume.store_ranges(index)?);
        }
        if hash_snapshots {
            let mut snapshot = volume.snapshot(index)?;
            let size = snapshot.layout().volume_size();
            let mut digest = Sha256::new();
            let mut buffer = vec![0; 1024 * 1024];
            let mut total = 0u64;
            loop {
                let count = snapshot.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
                total += count as u64;
            }
            if total != size {
                return Err("snapshot hash ended before its logical volume length".into());
            }
            println!("  snapshot_sha256: {:x}", digest.finalize());
        }
    }
    Ok(())
}
