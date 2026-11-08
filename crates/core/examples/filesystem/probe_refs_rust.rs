//! Compare the current ReFS DLL with two pure Rust replacement candidates.
//!
//! `just probe-refs <image> [--backend all|dll|refs-core|librefs|ported]`
//! uses LovelyCarve's partition detection and byte reader. The refs-core candidate
//! additionally needs a stable RAW file because its volume API takes `&[u8]`.
//! Candidate failures remain in the JSON report; no DLL fallback hides them.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use librefs_rust::btree::{BPlusTableReader, KeyType, PageSource};
use librefs_rust::io::{DeviceSize, ReadAt};
use librefs_rust::{RefsError, RefsResult};
use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::tree::fs_tree::NodeKind;
use lovelycarve_core::filesystem::volumes::refs_dll_oracle::{open_refs_source, RefsSource};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod refs_dll_corpus;

#[derive(Clone, Copy, ValueEnum)]
enum Backend {
    All,
    Dll,
    RefsCore,
    Librefs,
    Ported,
}

#[derive(Parser)]
struct Arguments {
    image: PathBuf,
    #[arg(long, value_enum, default_value = "all")]
    backend: Backend,
    /// Directory to enumerate; both Rust candidates currently only probe the root.
    #[arg(long, default_value = "/")]
    directory: String,
    /// File to hash using the current DLL, without writing extracted evidence.
    #[arg(long, default_value = "/System Volume Information/WPSettings.dat")]
    file: String,
    /// Save a clean JSON report (create_new; an existing file is never replaced).
    #[arg(long)]
    output: Option<PathBuf>,
    /// Traverse and hash every file and named stream with the DLL / ported reader.
    #[arg(long)]
    walk: bool,
}

#[derive(Serialize)]
struct Observation {
    backend: &'static str,
    stage: &'static str,
    status: &'static str,
    detail: Value,
}

fn observe<T: Serialize>(
    observations: &mut Vec<Observation>,
    backend: &'static str,
    stage: &'static str,
    result: Result<T>,
) {
    match result {
        Ok(value) => {
            observations.push(Observation {
                backend,
                stage,
                status: "passed",
                detail: json!(value),
            });
        }
        Err(error) => {
            observations.push(Observation {
                backend,
                stage,
                status: "failed",
                detail: json!({ "error": format!("{error:#}") }),
            });
        }
    }
}

/// Bounded, partition-relative I/O; never materializes an E01 or reads another partition.
struct PartitionReader {
    image: Arc<DiskImage>,
    base: u64,
    length: u64,
}

impl ReadAt for PartitionReader {
    fn read_into(&self, offset: u64, destination: &mut [u8]) -> RefsResult<usize> {
        let remaining = self.length.checked_sub(offset).ok_or_else(|| {
            RefsError::Io(
                "partition read",
                Error::new(ErrorKind::InvalidInput, "offset outside partition"),
            )
        })?;
        let count = remaining.min(destination.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let absolute = self.base.checked_add(offset).ok_or_else(|| {
            RefsError::Io(
                "partition read",
                Error::new(ErrorKind::InvalidInput, "offset overflow"),
            )
        })?;
        let bytes = self
            .image
            .read_range(absolute, count)
            .map_err(|error| RefsError::Io("partition read", Error::other(error.to_string())))?;
        if bytes.len() != count {
            return Err(RefsError::Io(
                "partition read",
                Error::new(ErrorKind::UnexpectedEof, "short image read"),
            ));
        }
        destination[..count].copy_from_slice(&bytes);
        Ok(count)
    }
}

impl DeviceSize for PartitionReader {
    fn size(&self) -> RefsResult<u64> {
        Ok(self.length)
    }
}

impl refs_reader::ReadAt for PartitionReader {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
        self.read_exact(offset, buffer)
            .map_err(|error| Error::other(error.to_string()))
    }
}

fn probe_ported(
    reader: PartitionReader,
    arguments: &Arguments,
    observations: &mut Vec<Observation>,
) {
    let volume = match refs_reader::Volume::open(Arc::new(reader)) {
        Ok(volume) => {
            observe(
                observations,
                "refs-reader-port",
                "mount",
                Ok(json!({
                    "version": format!("{}.{}", volume.boot().major, volume.boot().minor),
                    "cluster_size": volume.boot().cluster_size,
                    "checkpoint_block": volume.checkpoint().block,
                    "checkpoint_sequence": volume.checkpoint().sequence,
                })),
            );
            volume
        }
        Err(error) => {
            observe::<Value>(observations, "refs-reader-port", "mount", Err(error.into()));
            return;
        }
    };
    let records = (|| -> Result<_> {
        let object_id = if arguments.directory == "/" {
            0x600
        } else {
            volume
                .lookup(&arguments.directory)?
                .directory_id
                .context("directory id")?
        };
        Ok(volume.list_directory(object_id)?)
    })()
    .map(|records| {
        json!(records
            .into_iter()
            .map(|record| json!({
                "name": record.name, "directory": record.is_directory(), "size": record.size,
            }))
            .collect::<Vec<_>>())
    });
    observe(
        observations,
        "refs-reader-port",
        "root_directory",
        records.map_err(Into::into),
    );
    let result = (|| -> Result<Value> {
        let entry = volume.lookup(&arguments.file)?;
        let file = volume.open_file(entry)?;
        let extents = file
            .extents()
            .iter()
            .map(|extent| {
                json!({"logical":extent.logical,
            "physical":extent.physical,"length":extent.length})
            })
            .collect::<Vec<_>>();
        let mut digest = Sha256::new();
        let mut total = 0;
        let mut buffer = vec![0; 1024 * 1024];
        while total < file.entry().size {
            let count = file.read_range(total, &mut buffer)?;
            anyhow::ensure!(count != 0, "short ReFS file read");
            digest.update(&buffer[..count]);
            total += count as u64;
        }
        anyhow::ensure!(total == file.entry().size, "ReFS file size mismatch");
        Ok(
            json!({ "path": arguments.file, "bytes": total, "sha256": hex::encode(digest.finalize()), "extents":extents }),
        )
    })();
    observe(observations, "refs-reader-port", "file_content", result);
    if arguments.walk {
        observe(
            observations,
            "refs-reader-port",
            "corpus",
            probe_ported_corpus(&volume),
        );
    }
}

fn probe_ported_corpus(volume: &refs_reader::Volume) -> Result<Value> {
    let mut pending = vec![(String::new(), 0x600)];
    let mut visited = std::collections::HashSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    let mut buffer = vec![0; 1024 * 1024];
    while let Some((path, object_id)) = pending.pop() {
        anyhow::ensure!(visited.insert(object_id), "directory cycle at {path}");
        for entry in volume
            .list_directory(object_id)
            .with_context(|| format!("list {path}"))?
        {
            let path = format!("{path}/{}", entry.name);
            let mut observation = json!({"path": path, "directory": entry.is_directory(),
                "size": entry.size, "created": entry.created, "accessed": entry.accessed,
                "modified": entry.modified, "changed": entry.changed,
                "symlink": entry.symlink_target});
            let stream_result = (|| -> Result<Vec<Value>> {
                let mut output = Vec::new();
                for stream in volume.list_streams(&entry)? {
                    let file = volume.open_named_stream(entry.clone(), &stream.name)?;
                    let mut total = 0;
                    let mut digest = Sha256::new();
                    while total < stream.size {
                        let count = file.read_range(total, &mut buffer)?;
                        anyhow::ensure!(count > 0, "short named stream read");
                        digest.update(&buffer[..count]);
                        total += count as u64;
                    }
                    anyhow::ensure!(total == stream.size, "stream size mismatch");
                    output.push(json!({"name": stream.name, "size": stream.size,
                        "snapshot": stream.is_snapshot, "sha256": hex::encode(digest.finalize())}));
                }
                Ok(output)
            })();
            match stream_result {
                Ok(streams) => observation["streams"] = json!(streams),
                Err(error) => {
                    observation["stream_error"] = json!(format!("{error:#}"));
                    failures.push(format!("{path}:streams"));
                }
            }
            if let Some(directory_id) = entry.directory_id {
                pending.push((path.clone(), directory_id));
            } else {
                let result = (|| -> Result<String> {
                    let file = volume.open_file(entry)?;
                    let mut total = 0;
                    let mut digest = Sha256::new();
                    while total < file.entry().size {
                        let count = file.read_range(total, &mut buffer)?;
                        anyhow::ensure!(count > 0, "short file read");
                        digest.update(&buffer[..count]);
                        total += count as u64;
                    }
                    anyhow::ensure!(total == file.entry().size, "size mismatch");
                    Ok(hex::encode(digest.finalize()))
                })();
                match result {
                    Ok(hash) => observation["sha256"] = json!(hash),
                    Err(error) => {
                        observation["error"] = json!(format!("{error:#}"));
                        failures.push(path);
                    }
                }
            }
            entries.push(observation);
        }
    }
    Ok(json!({"directories": visited.len(), "entries": entries,
        "failed_files": failures.len(), "failures": failures}))
}

struct VolumePages<'a>(&'a librefs_rust::Volume<PartitionReader>);

impl PageSource for VolumePages<'_> {
    fn read_page(&self, cluster: u64, buffer: &mut [u8]) -> RefsResult<()> {
        let offset = cluster.checked_mul(self.0.cluster_size()).ok_or_else(|| {
            RefsError::Io(
                "metadata page",
                Error::new(ErrorKind::InvalidInput, "cluster offset overflow"),
            )
        })?;
        self.0.io().read_exact(offset, buffer)
    }
}

fn probe_librefs(reader: PartitionReader, observations: &mut Vec<Observation>) {
    let volume = match librefs_rust::Volume::open(reader) {
        Ok(volume) => {
            observe(
                observations,
                "librefs-0.1.0",
                "mount",
                Ok(json!({
                    "cluster_size": volume.cluster_size(),
                    "version": format!("{}.{}", volume.boot().version_major(), volume.boot().version_minor()),
                })),
            );
            volume
        }
        Err(error) => {
            observe::<Value>(observations, "librefs-0.1.0", "mount", Err(error.into()));
            return;
        }
    };
    let result = (|| -> Result<Value> {
        let table_cluster = volume
            .object_table_root_lcn()
            .context("object table root unavailable")?;
        let pages = VolumePages(&volume);
        let table = BPlusTableReader::new(&pages, table_cluster.get(), KeyType::BigIdentifier);
        // Use the candidate's documented root identifier, rather than silently fixing its model.
        let root_id = librefs_rust::object::ObjectId::VolumeRoot.raw();
        let root = librefs_rust::object::lookup_object(table_cluster, root_id, &table)?;
        let directory = BPlusTableReader::new(&pages, root.root_lcn.get(), KeyType::UnicodeString);
        let entries = librefs_rust::dir::enumerate_directory(root.root_lcn, &directory)?;
        Ok(json!(entries
            .into_iter()
            .map(|entry| json!({
                "name": entry.name, "directory": entry.is_directory(), "size": entry.size,
            }))
            .collect::<Vec<_>>()))
    })();
    observe(observations, "librefs-0.1.0", "root_directory", result);
}

fn probe_refs_core(
    arguments: &Arguments,
    reader: &PartitionReader,
    observations: &mut Vec<Observation>,
) {
    let result = (|| -> Result<Value> {
        let source = File::open(&arguments.image)?;
        // The offline fixture is not modified while this read-only mapping is alive.
        let mapped = unsafe { memmap2::Mmap::map(&source)? };
        let base = usize::try_from(reader.base)?;
        let length = usize::try_from(reader.length)?;
        let end = base
            .checked_add(length)
            .context("partition mapping overflow")?;
        let partition = mapped
            .get(base..end)
            .context("refs-core requires a complete RAW partition mapping")?;
        let mut logical_boot = [0u8; 512];
        reader.read_exact(0, &mut logical_boot)?;
        anyhow::ensure!(
            partition.get(..512) == Some(logical_boot.as_slice()),
            "refs-core needs RAW bytes, not the outer image container"
        );
        let boot = refs_core::BootSector::parse(partition)?;
        boot.require_v3()?;
        observe(
            observations,
            "refs-core-0.1.0",
            "boot",
            Ok(json!({
                "cluster_size": boot.cluster_size(), "version": format!("{}.{}", boot.major_version, boot.minor_version),
            })),
        );
        let entries = refs_core::list_dir(partition, refs_core::REFS_ROOT_DIRECTORY_ID)?;
        Ok(json!(entries
            .into_iter()
            .map(|entry| json!({
                "name": entry.name, "directory": entry.is_directory,
                "size": entry.metadata.map(|metadata| metadata.size),
            }))
            .collect::<Vec<_>>()))
    })();
    observe(observations, "refs-core-0.1.0", "root_directory", result);
    observations.push(Observation {
        backend: "refs-core-0.1.0",
        stage: "file_content",
        status: "unsupported",
        detail: json!({ "reason": "upstream has no file-content extraction API" }),
    });
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    anyhow::ensure!(matches!(arguments.backend, Backend::Dll | Backend::Ported) || arguments.directory == "/", "Rust candidate comparison currently probes the root; use --backend dll or ported for another directory");
    let image = Arc::new(DiskImage::open(&arguments.image)?);
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Refs)
        .context("no ReFS partition detected")?;
    let base = partition
        .start_lba
        .checked_mul(image.sector_size as u64)
        .context("partition offset overflow")?;
    let length = partition
        .length_lba
        .checked_mul(image.sector_size as u64)
        .context("partition length overflow")?;
    let partition_id = partition.id;
    let mut observations = Vec::new();

    if matches!(arguments.backend, Backend::All | Backend::Dll) {
        match open_refs_source(&image, RefsSource::Partition(partition_id)) {
            Ok(volume) => {
                observe(
                    &mut observations,
                    "librefs-dll",
                    "mount",
                    Ok(json!({ "length": volume.partition_length() })),
                );
                let listing = volume.children(&arguments.directory).map(|entries| json!(entries.into_iter().map(|entry| json!({
                    "name": entry.name, "directory": entry.kind == NodeKind::Dir, "size": entry.size,
                })).collect::<Vec<_>>()));
                observe(&mut observations, "librefs-dll", "directory", listing);
                let read = (|| -> Result<Value> {
                    let mut digest = Sha256::new();
                    let mut total = 0u64;
                    let count = volume.stream_file(&arguments.file, 1024 * 1024, |bytes| {
                        digest.update(bytes);
                        total += bytes.len() as u64;
                        Ok(())
                    })?;
                    let expected = volume.file_stat(&arguments.file, "probe")?.size;
                    anyhow::ensure!(
                        count == expected && total == expected,
                        "short file read: {total} of {expected} bytes"
                    );
                    Ok(
                        json!({ "path": arguments.file, "bytes": total, "sha256": hex::encode(digest.finalize()) }),
                    )
                })();
                observe(&mut observations, "librefs-dll", "file_content", read);
                if arguments.walk {
                    observe(
                        &mut observations,
                        "librefs-dll",
                        "corpus",
                        refs_dll_corpus::probe(&volume),
                    );
                }
            }
            Err(error) => {
                observe::<Value>(&mut observations, "librefs-dll", "mount", Err(error));
            }
        }
    }
    if matches!(arguments.backend, Backend::All | Backend::RefsCore) {
        probe_refs_core(
            &arguments,
            &PartitionReader {
                image: Arc::clone(&image),
                base,
                length,
            },
            &mut observations,
        );
    }
    if matches!(arguments.backend, Backend::All | Backend::Librefs) {
        probe_librefs(
            PartitionReader {
                image: Arc::clone(&image),
                base,
                length,
            },
            &mut observations,
        );
    }
    if matches!(arguments.backend, Backend::All | Backend::Ported) {
        probe_ported(
            PartitionReader {
                image,
                base,
                length,
            },
            &arguments,
            &mut observations,
        );
    }
    let report = serde_json::to_string_pretty(&json!({
        "image": arguments.image, "partition_id": partition_id, "partition_offset": base,
        "partition_length": length, "observations": observations,
    }))?;
    if let Some(path) = &arguments.output {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(report.as_bytes())?;
        file.write_all(b"\n")?;
    } else {
        println!("{report}");
    }
    if observations.iter().any(|observation| {
        observation.status != "passed"
            || observation
                .detail
                .get("failed_files")
                .and_then(Value::as_u64)
                .is_some_and(|count| count != 0)
    }) {
        return Err(anyhow!(
            "ReFS replacement probe found failed or unsupported operations; see JSON report"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader() -> Result<(tempfile::TempDir, PartitionReader)> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("source.raw");
        let mut bytes = vec![0; 4096];
        bytes[512..520].copy_from_slice(b"evidence");
        std::fs::write(&path, bytes)?;
        let image = Arc::new(DiskImage::open(path)?);
        Ok((
            directory,
            PartitionReader {
                image,
                base: 512,
                length: 8,
            },
        ))
    }

    #[test]
    fn partition_read_is_relative_and_cannot_cross_its_end() -> Result<()> {
        let (_directory, reader) = reader()?;
        let mut bytes = [0xAA; 4];
        let count = reader.read_into(6, &mut bytes)?;
        assert_eq!((count, bytes), (2, [b'c', b'e', 0xAA, 0xAA]));
        assert_eq!(reader.read_into(8, &mut bytes)?, 0);
        assert!(reader.read_into(9, &mut bytes).is_err());
        Ok(())
    }

    #[test]
    fn exact_read_rejects_truncation_and_absolute_offset_overflow() -> Result<()> {
        let (_directory, mut reader) = reader()?;
        assert!(reader.read_exact(6, &mut [0; 4]).is_err());
        reader.base = u64::MAX;
        assert!(reader.read_into(1, &mut [0; 1]).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires REFS_ORACLE_IMAGE pointing to the documented 4 KiB RAW fixture and the current DLL"]
    fn ported_reader_preserves_dll_coverage_and_corrects_unwritten_runs() -> Result<()> {
        let path = std::env::var("REFS_ORACLE_IMAGE").context("set REFS_ORACLE_IMAGE")?;
        let image = Arc::new(DiskImage::open(path)?);
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::Refs)
            .context("ReFS partition")?;
        let partition_id = partition.id;
        let reader = PartitionReader {
            base: partition
                .start_lba
                .checked_mul(image.sector_size as u64)
                .context("partition offset")?,
            length: partition
                .length_lba
                .checked_mul(image.sector_size as u64)
                .context("partition size")?,
            image: Arc::clone(&image),
        };
        let rust = refs_reader::Volume::open(Arc::new(reader))?;
        assert_eq!(
            (
                rust.boot().major,
                rust.boot().minor,
                rust.boot().cluster_size
            ),
            (3, 14, 4096)
        );
        let dll = open_refs_source(&image, RefsSource::Partition(partition_id))?;
        for path in [
            "/",
            "/System Volume Information",
            "/test",
            "/test/elvis_dir_victor_783323",
        ] {
            let object_id = if path == "/" {
                0x600
            } else {
                rust.lookup(path)?.directory_id.context("directory id")?
            };
            let mut actual = rust
                .list_directory(object_id)?
                .into_iter()
                .map(|entry| (entry.name.clone(), entry.is_directory(), entry.size))
                .collect::<Vec<_>>();
            let mut expected = dll
                .children(path)?
                .into_iter()
                .map(|entry| (entry.name, entry.kind == NodeKind::Dir, entry.size))
                .collect::<Vec<_>>();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "directory {path}");
        }
        for (path, size, dll_expected, rust_expected) in [
            (
                "/System Volume Information/WPSettings.dat",
                12u64,
                "d65866983685530595dcdfcb6fed3345517dcb3513efb883fd1fb53fb9260994",
                "d65866983685530595dcdfcb6fed3345517dcb3513efb883fd1fb53fb9260994",
            ),
            (
                "/test/elvis_dir_victor_783323/xbpt_large_149mb_alpha_610845.dat",
                156237824,
                "8c63cbc67d8ff5481e01ea2f42ec770669239811932e294a8b286a879845a269",
                "c7d96f97bfab0380aa99d8c0c8ffdec69715d637febcc74e5e83cc8292ecb664",
            ),
        ] {
            let file = rust.open_file(rust.lookup(path)?)?;
            assert_eq!(file.entry().size, size);
            let ranges = [(0, 8), (size / 2, 1027), (size - 5, 99)];
            let mut expected_ranges = vec![Vec::new(); ranges.len()];
            let mut dll_hash = Sha256::new();
            let mut total = 0u64;
            let streamed = dll.stream_file(path, 1024 * 1024, |bytes| {
                dll_hash.update(bytes);
                for ((start, length), output) in ranges.iter().zip(&mut expected_ranges) {
                    let overlap_start = total.max(*start);
                    let overlap_end = (total + bytes.len() as u64).min(start + length);
                    if overlap_start < overlap_end {
                        output.extend_from_slice(
                            &bytes
                                [(overlap_start - total) as usize..(overlap_end - total) as usize],
                        );
                    }
                }
                total += bytes.len() as u64;
                Ok(())
            })?;
            assert_eq!(
                (streamed, total, hex::encode(dll_hash.finalize())),
                (size, size, dll_expected.to_string())
            );
            let mut rust_hash = Sha256::new();
            let mut offset = 0;
            let mut buffer = vec![0; 1024 * 1024];
            while offset < size {
                let count = file.read_range(offset, &mut buffer)?;
                anyhow::ensure!(count > 0, "short file read");
                rust_hash.update(&buffer[..count]);
                offset += count as u64;
            }
            assert_eq!(
                (offset, hex::encode(rust_hash.finalize())),
                (size, rust_expected.to_string())
            );
            for ((offset, length), expected) in ranges.iter().zip(expected_ranges) {
                let mut buffer = vec![0xAA; *length as usize];
                let count = file.read_range(*offset, &mut buffer)?;
                let unwritten = file.extents().iter().any(|extent| {
                    extent.physical.is_none()
                        && extent.logical <= *offset
                        && extent.logical + extent.length >= *offset + count as u64
                });
                if !unwritten {
                    assert_eq!(
                        &buffer[..count],
                        expected.as_slice(),
                        "range {path} at {offset}"
                    );
                } else {
                    // The corpus's reserved but unwritten tail must read as zeros,
                    // rather than the foreign allocated bytes returned by the DLL.
                    assert!(buffer[..count].iter().all(|byte| *byte == 0));
                }
                assert!(buffer[count..].iter().all(|byte| *byte == 0xAA));
            }
        }
        Ok(())
    }
}
