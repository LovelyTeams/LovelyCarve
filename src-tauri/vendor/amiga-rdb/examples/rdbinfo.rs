//! Print an image's RDB: `cargo run --example rdbinfo -- disk.hdf [block-size]`
//!
//! `block-size` is the *device* block size the image was taken from
//! (default 512); it must match the RDB's `rdb_BlockBytes`.

use amiga_rdb::{rdb_flags, Rdb, SeekBlockSource, CHAIN_END};
use std::fs::File;

/// A dostype the conventional way: three printable characters and the
/// version byte as a number, e.g. `DOS\3`. Shared by partitions and
/// filesystem headers, which is the whole point — the two are matched on
/// this value.
fn dostype(v: u32) -> String {
    let b = v.to_be_bytes();
    format!("{}{}{}\\{}", b[0] as char, b[1] as char, b[2] as char, b[3])
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: rdbinfo <image> [block-size]");
    let block_size: usize = std::env::args()
        .nth(2)
        .map(|s| s.parse().expect("block-size must be a number"))
        .unwrap_or(512);
    let file = File::open(&path).expect("open image");
    let mut disk = SeekBlockSource::with_block_size(file, block_size).expect("stat image");
    let rdb = match Rdb::parse(&mut disk) {
        Ok(rdb) => rdb,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "RDSK at block {}  {} B/block  geometry {}/{}/{}  rdb blocks {}..={}",
        rdb.rdsk_block,
        rdb.block_bytes,
        rdb.cylinders,
        rdb.heads,
        rdb.sectors,
        rdb.rdb_blocks_lo,
        rdb.rdb_blocks_hi
    );
    // Only printed when the flag says the bytes mean anything: without
    // DISKID/CTRLRID these fields are uninitialised, and showing them
    // would be inventing a drive identity.
    if rdb.flags & rdb_flags::DISK_ID != 0 {
        println!(
            "disk: {} {} rev {}",
            rdb.disk_vendor, rdb.disk_product, rdb.disk_revision
        );
    }
    if rdb.flags & rdb_flags::CTRLR_ID != 0 {
        println!(
            "controller: {} {} rev {}",
            rdb.controller_vendor, rdb.controller_product, rdb.controller_revision
        );
    }
    // The loadable filesystems the image carries — this is how a
    // partition with a dostype the ROM never heard of still mounts.
    for f in &rdb.filesystems {
        println!(
            "FSHD at block {}  {}  version {}.{}  {}",
            f.fshd_block,
            dostype(f.dos_type),
            f.version_major(),
            f.version_minor(),
            if f.seg_list_blocks == CHAIN_END {
                String::from("no LSEG chain")
            } else {
                format!("LSEG chain head block {}", f.seg_list_blocks)
            },
        );
    }
    if !rdb.bad_blocks.is_empty() {
        println!("bad blocks: {} remapped", rdb.bad_blocks.len());
    }
    for p in &rdb.partitions {
        let dos = dostype(p.dos_type);
        println!(
            "  {:<8} {}  cyls {:>5}..={:<5}  lba {:>8} +{:<8} ({} MB)  bootpri {}{}{}",
            p.name,
            dos,
            p.low_cyl,
            p.high_cyl,
            p.start_lba,
            p.block_len,
            // Saturating: both factors come off the image, and a hostile
            // `de_HighCyl` makes the product overflow — a debug panic,
            // and in release a wrapped size printed as fact.
            p.block_len.saturating_mul(rdb.block_bytes as u64) / (1024 * 1024),
            p.boot_pri,
            if p.bootable { "  bootable" } else { "" },
            if p.no_automount { "  noautomount" } else { "" },
        );
    }

    // Layout validation last, so it reads as a verdict on everything
    // printed above. Both halves: `validate` covers the chains held in
    // memory, `validate_seg_lists` needs the disk back for the lazy LSEG
    // blocks. Reported, never fatal — an image whose RDB has spilled
    // into a partition is exactly the one someone is trying to recover.
    let mut issues = rdb.validate();
    match rdb.validate_seg_lists(&mut disk) {
        Ok(more) => issues.extend(more),
        Err(e) => eprintln!("{path}: walking LSEG chains: {e}"),
    }
    if !issues.is_empty() {
        println!("layout issues ({}):", issues.len());
        for issue in &issues {
            println!("  ! {issue}");
        }
    }
}
