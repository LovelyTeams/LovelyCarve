//! Compare full, streamed and ranged NTFS reads, optionally against oracle bytes.
use anyhow::{ensure, Context};
use lovelycarve_lib::core::{evidence::image::DiskImage, filesystem::volumes::ntfs::NtfsVolume};
use sha2::{Digest, Sha256};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        arguments.len() >= 3,
        "usage: probe_ntfs_stream <image> <partition-id> <record> [oracle-file]"
    );
    let image = DiskImage::open(&arguments[0])?;
    let partition_id: u8 = arguments[1].parse()?;
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.id == partition_id)
        .context("NTFS partition not found")?;
    let volume = NtfsVolume::open(&image, partition)?;
    let record: u64 = arguments[2].parse()?;
    let stat = volume.file_stat(record, "probe")?;
    if stat.size > 16 * 1024 * 1024 {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut spool = tempfile::NamedTempFile::new()?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let count = volume
            .stream_file(record, 1024 * 1024, |chunk| {
                spool.write_all(chunk)?;
                hash.update(chunk);
                total += chunk.len() as u64;
                Ok(())
            })
            .with_context(|| format!("stream failed after {total} bytes"))?;
        ensure!(count == stat.size, "short streamed read");
        let mut ranges = 0;
        for offset in [
            0,
            1,
            4090,
            65530,
            stat.size / 2,
            stat.size.saturating_sub(70001),
            stat.size,
        ] {
            let bytes = volume.read_range(record, offset, 70001)?;
            let mut expected = vec![0; (stat.size - offset).min(70001) as usize];
            spool.seek(SeekFrom::Start(offset))?;
            spool.read_exact(&mut expected)?;
            ensure!(bytes == expected, "range differs at {offset}");
            ranges += 1;
        }
        let prefix = volume.read_file(record, 16 * 1024 * 1024)?;
        spool.seek(SeekFrom::Start(0))?;
        let mut expected = vec![0; prefix.len()];
        spool.read_exact(&mut expected)?;
        ensure!(prefix == expected, "bounded full read differs");
        let hash = hash.finalize();
        if let Some(path) = arguments.get(3) {
            let mut oracle = std::fs::File::open(path)?;
            let mut oracle_hash = Sha256::new();
            let mut buffer = vec![0; 1024 * 1024];
            let mut length = 0u64;
            loop {
                let read = oracle.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                oracle_hash.update(&buffer[..read]);
                length += read as u64;
            }
            ensure!(
                length == stat.size && oracle_hash.finalize() == hash,
                "NTFS bytes differ from independent oracle"
            );
        }
        println!(
            "record={record} size={} sha256={} ranges={ranges} oracle={}",
            stat.size,
            hex::encode(hash),
            arguments.get(3).is_some()
        );
        return Ok(());
    }
    ensure!(stat.size <= 16 * 1024 * 1024, "probe file exceeds 16 MiB");
    let bytes = volume.read_file(record, stat.size as usize)?;
    ensure!(bytes.len() as u64 == stat.size, "short full read");
    let expected_hash = Sha256::digest(&bytes);
    let mut stream_hash = Sha256::new();
    let count = volume.stream_file(record, 77777, |chunk| {
        stream_hash.update(chunk);
        Ok(())
    })?;
    ensure!(
        count == stat.size && stream_hash.finalize() == expected_hash,
        "streamed bytes differ"
    );
    let mut ranges = 0;
    for offset in [
        0,
        1,
        15,
        4090,
        65530,
        bytes.len() / 2,
        bytes.len().saturating_sub(17),
        bytes.len(),
    ] {
        let offset = offset.min(bytes.len());
        let end = (offset + 70001).min(bytes.len());
        ensure!(
            volume.read_range(record, offset as u64, 70001)? == bytes[offset..end],
            "range differs at {offset}"
        );
        ranges += 1;
    }
    if let Some(path) = arguments.get(3) {
        let oracle = std::fs::read(path)?;
        ensure!(bytes == oracle, "NTFS bytes differ from independent oracle");
    }
    println!(
        "record={record} size={} sha256={} ranges={ranges} oracle={}",
        stat.size,
        hex::encode(expected_hash),
        arguments.get(3).is_some()
    );
    Ok(())
}
