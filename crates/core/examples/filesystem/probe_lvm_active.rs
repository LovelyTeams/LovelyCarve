//! Inspect the verified current LVM2 metadata, including wrapped and backup copies.
//! Usage: just run-example probe_lvm_active <image>
use lovelycarve_core::evidence::image::DiskImage;
use lovelycarve_core::evidence::partitions::{lvm2::header::read_metadata, PartitionKind};

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: probe_lvm_active <image>"))?;
    let image = DiskImage::open(&path)?;
    for partition in &image.partitions {
        if partition.kind != PartitionKind::Lvm {
            continue;
        }
        let offset = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| anyhow::anyhow!("partition offset overflow"))?;
        let size = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .ok_or_else(|| anyhow::anyhow!("partition size overflow"))?;
        let metadata = read_metadata(&image, offset, size)?;
        let group = &metadata.volume_group;
        println!(
            "partition {}: PV={} VG={} UUID={} seqno={} extent_sectors={}",
            partition.id,
            metadata.header.identifier,
            group.name,
            group.identifier,
            group.sequence_number,
            group.extent_size_sectors
        );
        for range in &metadata.text_ranges {
            println!(
                "  active text: PV+{:#x} size={:#x}",
                range.offset, range.size
            );
        }
        for pv in &group.physical_volumes {
            println!(
                "  PV {} UUID={} device={:?} sectors={} pe_start={} pe_count={}",
                pv.name,
                pv.identifier,
                pv.device,
                pv.size_sectors,
                pv.pe_start_sectors,
                pv.extent_count
            );
        }
        for volume in &group.logical_volumes {
            println!(
                "  LV {} UUID={} segments={}",
                volume.name,
                volume.identifier,
                volume.segments.len()
            );
            for segment in &volume.segments {
                println!(
                    "    LE={} count={} type={} stripes={:?}",
                    segment.start_extent, segment.extent_count, segment.kind, segment.stripes
                );
            }
        }
        for diagnostic in &metadata.diagnostics {
            println!("  warning: {diagnostic}");
        }
    }
    Ok(())
}
