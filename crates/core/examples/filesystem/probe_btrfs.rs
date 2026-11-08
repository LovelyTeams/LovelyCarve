//! Smoke-test the Btrfs walker. Usage: cargo run --example probe_btrfs -- <image>
use std::env;

use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::volumes::btrfs::{open_btrfs_source, BtrfsSource};

fn main() -> anyhow::Result<()> {
    let path = env::args().nth(1).expect("usage: probe_btrfs <image>");
    let image = DiskImage::open(&path)?;
    let info = image.info();
    println!("image: {} ({} bytes)", info.path, info.size_bytes);
    let Some(part) = info
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Btrfs)
    else {
        anyhow::bail!("no Btrfs partition discovered in {}", info.path);
    };
    println!(
        "btrfs partition: id={} start_lba={} length_lba={}",
        part.id, part.start_lba, part.length_lba
    );

    let volume = open_btrfs_source(&image, BtrfsSource::Partition(part.id))?;
    let (fs_root_bytenr, fs_root_level) = volume.debug_default_fs_tree_root()?;
    println!(
        "volume: label={:?} sector_size={} root_inode={} default_subvol={} fs_tree=0x{:x}@L{}",
        volume.label(),
        volume.sector_size_fs(),
        volume.root_inode(),
        volume.default_subvol(),
        fs_root_bytenr,
        fs_root_level
    );

    println!("subvolumes ({}):", volume.list_subvolumes().len());
    for (id, name) in volume.list_subvolumes() {
        println!("  id={id:>10}  name={:?}", name);
    }

    let sample = volume.debug_sample_fs_keys(40)?;
    println!("\nfirst {} default-subvol FS-tree keys:", sample.len());
    for (objid, ktype, off) in &sample {
        println!("  ({objid:>10}, type={ktype:>3}, off=0x{off:x})");
    }

    let raw = volume.debug_dir_index(volume.default_subvol(), volume.root_inode())?;
    println!(
        "\nraw DIR_INDEX entries under inode {} (default subvol):",
        volume.root_inode()
    );
    for (name, loc_obj, loc_type, ftype) in &raw {
        println!(
            "  name={name:<24} location_objectid={loc_obj:>10} location_type={loc_type:>3} ftype={ftype}"
        );
    }

    let children = volume.children(volume.default_subvol(), volume.root_inode())?;
    println!("\ndefault root has {} children:", children.len());
    for (i, child) in children.iter().take(40).enumerate() {
        println!(
            "  [{i:>2}] {kind:?} {size:>12}  {id}  {name}",
            kind = child.kind,
            size = child.size,
            id = child.id,
            name = child.name
        );
    }

    // If the default tree just points at a single subvolume, descend once
    // to demonstrate that subvolume entries are followable.
    if let Some(first) = children.first() {
        if let Some((_, subvol, inode)) =
            lovelycarve_core::filesystem::volumes::btrfs::parse_btrfs_id(&first.id)
        {
            let inner = volume.children(subvol, inode)?;
            println!(
                "\n@{}/{} ({}) has {} entries:",
                subvol,
                inode,
                first.name,
                inner.len()
            );
            for (i, c) in inner.iter().take(60).enumerate() {
                println!(
                    "  [{i:>2}] {kind:?} {size:>12}  {name}",
                    kind = c.kind,
                    size = c.size,
                    name = c.name
                );
            }

            // Read the first regular file we see to confirm extent reads work.
            if let Some(file) = inner.iter().find(|n| {
                n.kind == lovelycarve_core::filesystem::tree::fs_tree::NodeKind::File && n.size > 0
            }) {
                if let Some((_, fsubvol, finode)) =
                    lovelycarve_core::filesystem::volumes::btrfs::parse_btrfs_id(&file.id)
                {
                    let inode_meta = volume.read_inode(fsubvol, finode)?;
                    let data = volume.read_inode_data(fsubvol, &inode_meta, file.size as usize)?;
                    println!(
                        "\nfile {:?} ({} bytes): first 200 bytes = {:?}",
                        file.name,
                        data.len(),
                        String::from_utf8_lossy(&data[..data.len().min(200)])
                    );
                }
            }

            // Read a symlink target (stored as inline extent data).
            if let Some(link) = inner.iter().find(|n| {
                n.kind == lovelycarve_core::filesystem::tree::fs_tree::NodeKind::Symlink
                    && n.size > 0
            }) {
                if let Some((_, lsubvol, linode)) =
                    lovelycarve_core::filesystem::volumes::btrfs::parse_btrfs_id(&link.id)
                {
                    let inode_meta = volume.read_inode(lsubvol, linode)?;
                    let data = volume.read_inode_data(lsubvol, &inode_meta, link.size as usize)?;
                    println!(
                        "symlink {:?} -> {:?}",
                        link.name,
                        String::from_utf8_lossy(&data)
                    );
                }
            }
        }
    }
    Ok(())
}
