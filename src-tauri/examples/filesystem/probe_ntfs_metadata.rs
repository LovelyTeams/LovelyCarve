//! Dump NTFS volume or standalone exported MFT metadata as requested JSON.
use anyhow::{ensure, Context};
use lovelycarve_lib::core::{
    evidence::image::DiskImage,
    filesystem::volumes::ntfs::{NtfsMftMetadataFile, NtfsVolume},
};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    ensure!(arguments.len() >= 3, "usage: probe_ntfs_metadata <image> <partition-id> <record> | --mft <exported-MFT> <record>");
    let result = if arguments[0] == "--mft" {
        let record: u64 = arguments[2].parse()?;
        let mut file = NtfsMftMetadataFile::open(std::path::Path::new(&arguments[1]))?;
        serde_json::json!({ "mft_records": file.record_count(), "volume": file.volume_metadata()?, "record": file.record_metadata(record)? })
    } else {
        let image = DiskImage::open(&arguments[0])?;
        let id: u8 = arguments[1].parse()?;
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == id)
            .context("NTFS partition not found")?;
        let volume = NtfsVolume::open(&image, partition)?;
        let record = arguments[2]
            .parse::<u64>()
            .or_else(|_| volume.find_record_by_path(&arguments[2]))?;
        serde_json::json!({ "volume": volume.volume_metadata()?, "record": volume.record_metadata(record)?, "security_descriptor_hex": volume.security_descriptor(record, 1024 * 1024)?.map(hex::encode) })
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
