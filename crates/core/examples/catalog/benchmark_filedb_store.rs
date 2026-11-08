//! Synthetic benchmark for the SQLite volume index (`core::catalog::filedb::filedb_store`).
//!
//! Usage:
//!
//! ```text
//! just run-example benchmark_filedb_store --entries 100000 --max-peak-rss-growth-mib 256
//! just bench-filedb-store 0 --reuse-snapshot --snapshot-path <volume.dirz> \
//!     --image-id <image id in the .dirz header> --volume-key <volume key>
//! ```
//!
//! By default this writes into a temporary directory and prints one JSON object
//! with timings, row counts, query sanity checks, and process RSS samples. The
//! generated snapshot puts every entry directly under the volume root, which
//! exercises neither directory fan-out nor the child-count cache; pass
//! `--reuse-snapshot` to import an existing `.dirz` (for example a case
//! volume's `evidence/<uid>/volumes/<volume>/tree-*.dirz`) instead.
//! `--entries` is ignored then. Every run imports into a fresh index file.

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(windows)]
use std::ffi::c_void;

use anyhow::Context;
use lovelycarve_core::catalog::filedb::filedb_store::IndexSource;
use lovelycarve_core::catalog::snapshot::volume_snapshot::{
    SnapshotEntry, SnapshotHeader, SNAPSHOT_SCHEMA,
};
use lovelycarve_core::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};
use lovelycarve_core::{
    casework::case_paths, catalog::filedb::filedb_query, catalog::filedb::filedb_store,
};
use serde::Serialize;

const DEFAULT_ENTRIES: usize = 1_000_000;
const DEFAULT_PAGE_SIZE: usize = 100;
const DEFAULT_IMAGE_ID: &str = "benchmark-img";
const DEFAULT_VOLUME_KEY: &str = "part:0";

fn main() -> anyhow::Result<()> {
    let config = BenchmarkConfig::parse(env::args().skip(1))?;
    let tempdir = if config.case_dir.is_none() {
        Some(tempfile::tempdir().context("create temporary benchmark case dir")?)
    } else {
        None
    };
    let case_dir = config
        .case_dir
        .as_deref()
        .or_else(|| tempdir.as_ref().map(|dir| dir.path()))
        .context("missing benchmark case dir")?;
    std::fs::create_dir_all(case_dir)
        .with_context(|| format!("create benchmark case dir {}", case_dir.display()))?;
    let snapshot_path = config
        .snapshot_path
        .clone()
        .unwrap_or_else(|| case_dir.join("benchmark_part_0.dirz"));

    let memory_start = process_memory_sample();

    let started = Instant::now();
    if !config.reuse_snapshot {
        write_generated_snapshot_file(
            &snapshot_path,
            config.entries,
            &config.image_id,
            &config.volume_key,
        )?;
    }
    let write_ms = started.elapsed().as_secs_f64() * 1000.0;
    let snapshot_bytes = std::fs::metadata(&snapshot_path)
        .with_context(|| format!("stat generated snapshot {}", snapshot_path.display()))?
        .len();
    let memory_after_write = process_memory_sample();

    let index_path = case_dir.join(case_paths::index_file_name(&case_paths::new_token()));
    let started = Instant::now();
    let rows = filedb_store::build_index(
        &snapshot_path,
        &index_path,
        &IndexSource {
            image_id: &config.image_id,
            volume_key: &config.volume_key,
            vol_id: 0,
            built_at: "2026-07-27T00:00:00.000Z",
            fingerprint: "benchmark:unknown",
            truncated: false,
        },
        None,
        None,
    )?;
    let rows = usize::try_from(rows).context("indexed row count exceeds usize")?;
    let index_ms = started.elapsed().as_secs_f64() * 1000.0;
    let memory_after_index = process_memory_sample();

    let started = Instant::now();
    let first_page = filedb_query::children_page(
        &index_path,
        &config.volume_key,
        None,
        Some(config.page_size),
    )?
    .context("root should be covered by generated snapshot index")?;
    let first_page_ms = started.elapsed().as_secs_f64() * 1000.0;

    let started = Instant::now();
    let second_page_items = if let Some(cursor) = first_page.next_cursor.as_deref() {
        filedb_query::children_page(
            &index_path,
            &config.volume_key,
            Some(cursor),
            Some(config.page_size),
        )?
        .map(|page| page.items.len())
        .unwrap_or_default()
    } else {
        0
    };
    let second_page_ms = started.elapsed().as_secs_f64() * 1000.0;

    let started = Instant::now();
    let counts = filedb_query::child_counts(&index_path, std::slice::from_ref(&config.volume_key))?;
    let child_count_ms = started.elapsed().as_secs_f64() * 1000.0;
    let child_count = counts
        .get(&config.volume_key)
        .map(|counts| counts.count)
        .unwrap_or_default();
    let memory_after_query = process_memory_sample();

    // The generated layout is flat, so every entry is a root child. A reused
    // snapshot has its own shape; its root total must agree with the cache.
    let expected_root_children = if config.reuse_snapshot {
        first_page.total.unwrap_or_default()
    } else {
        anyhow::ensure!(
            rows == config.entries,
            "indexed row mismatch: got {rows}, expected {}",
            config.entries
        );
        config.entries
    };
    anyhow::ensure!(
        first_page.total == Some(expected_root_children),
        "root total mismatch: got {:?}, expected {expected_root_children}",
        first_page.total
    );
    anyhow::ensure!(
        child_count == expected_root_children as u64,
        "child-count mismatch: got {child_count}, expected {expected_root_children}"
    );

    let memory_samples = [
        memory_start,
        memory_after_write,
        memory_after_index,
        memory_after_query,
    ];
    let peak_growth_bytes = peak_rss_growth_bytes(&memory_samples);
    if let Some(limit) = config.max_peak_rss_growth_mib {
        let growth = peak_growth_bytes
            .context("peak RSS growth limit was set, but peak RSS is unavailable")?;
        let limit_bytes = limit
            .checked_mul(1024 * 1024)
            .context("peak RSS growth limit is too large")?;
        anyhow::ensure!(
            growth <= limit_bytes,
            "peak RSS growth {} exceeded limit {}",
            format_mib(growth),
            format_mib(limit_bytes)
        );
    }

    if !config.keep_dirz && config.snapshot_path.is_none() {
        std::fs::remove_file(&snapshot_path)
            .with_context(|| format!("remove generated snapshot {}", snapshot_path.display()))?;
    }
    case_paths::remove_database_files(&index_path)
        .with_context(|| format!("remove benchmark index {}", index_path.display()))?;

    let report = BenchmarkReport {
        entries: if config.reuse_snapshot {
            rows
        } else {
            config.entries
        },
        rows_indexed: rows,
        case_dir: case_dir.display().to_string(),
        snapshot_path: snapshot_path.display().to_string(),
        snapshot_bytes,
        write_ms,
        index_ms,
        first_page_ms,
        second_page_ms,
        child_count_ms,
        page_size: config.page_size,
        first_page_items: first_page.items.len(),
        second_page_items,
        first_page_total: first_page.total,
        child_count,
        memory: MemoryReport {
            start: memory_samples[0],
            after_write: memory_samples[1],
            after_index: memory_samples[2],
            after_query: memory_samples[3],
            peak_growth_bytes,
            peak_growth_human: peak_growth_bytes.map(format_mib),
            max_peak_rss_growth_mib: config.max_peak_rss_growth_mib,
        },
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(Debug)]
struct BenchmarkConfig {
    entries: usize,
    page_size: usize,
    case_dir: Option<PathBuf>,
    snapshot_path: Option<PathBuf>,
    keep_dirz: bool,
    /// Import the existing `--snapshot-path` instead of generating one.
    reuse_snapshot: bool,
    image_id: String,
    volume_key: String,
    max_peak_rss_growth_mib: Option<u64>,
}

impl BenchmarkConfig {
    fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let mut config = Self {
            entries: DEFAULT_ENTRIES,
            page_size: DEFAULT_PAGE_SIZE,
            case_dir: None,
            snapshot_path: None,
            keep_dirz: false,
            reuse_snapshot: false,
            image_id: DEFAULT_IMAGE_ID.to_string(),
            volume_key: DEFAULT_VOLUME_KEY.to_string(),
            max_peak_rss_growth_mib: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--entries" => {
                    config.entries = parse_next(&mut args, "--entries")?;
                }
                "--page-size" => {
                    config.page_size = parse_next(&mut args, "--page-size")?;
                }
                "--case-dir" => {
                    config.case_dir = Some(PathBuf::from(next_arg(&mut args, "--case-dir")?));
                }
                "--snapshot-path" => {
                    config.snapshot_path =
                        Some(PathBuf::from(next_arg(&mut args, "--snapshot-path")?));
                }
                "--keep-dirz" => {
                    config.keep_dirz = true;
                }
                "--reuse-snapshot" => {
                    config.reuse_snapshot = true;
                }
                "--image-id" => {
                    config.image_id = next_arg(&mut args, "--image-id")?;
                }
                "--volume-key" => {
                    config.volume_key = next_arg(&mut args, "--volume-key")?;
                }
                "--max-peak-rss-growth-mib" => {
                    config.max_peak_rss_growth_mib =
                        Some(parse_next(&mut args, "--max-peak-rss-growth-mib")?);
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument {other}; use --help for usage"),
            }
        }
        anyhow::ensure!(
            config.reuse_snapshot || config.entries > 0,
            "--entries must be greater than zero"
        );
        anyhow::ensure!(
            config.page_size > 0,
            "--page-size must be greater than zero"
        );
        if config.reuse_snapshot {
            let path = config
                .snapshot_path
                .as_deref()
                .context("--reuse-snapshot requires --snapshot-path")?;
            anyhow::ensure!(
                path.is_file(),
                "--reuse-snapshot: {} is not a file",
                path.display()
            );
        }
        Ok(config)
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<String> {
    args.next()
        .with_context(|| format!("{name} requires a value"))
}

fn parse_next<T>(args: &mut impl Iterator<Item = String>, name: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    next_arg(args, name)?
        .parse::<T>()
        .map_err(|err| anyhow::anyhow!("{name} has invalid value: {err}"))
}

fn print_usage() {
    println!(
        "usage: benchmark_filedb_store [--entries N] [--page-size N] \
         [--case-dir PATH] [--snapshot-path PATH [--reuse-snapshot]] [--keep-dirz] \
         [--image-id ID] [--volume-key KEY] [--max-peak-rss-growth-mib N]"
    );
}

fn write_generated_snapshot_file(
    path: &Path,
    entry_count: usize,
    image_id: &str,
    volume_key: &str,
) -> anyhow::Result<()> {
    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image_id.to_string(),
        source_volume_size: 1024,
        built_at: "2026-07-27T00:00:00Z".to_string(),
    };
    let file =
        File::create(path).with_context(|| format!("create snapshot file {}", path.display()))?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    writeln!(encoder, "{}", serde_json::to_string(&header)?)?;
    let mut scratch = Vec::new();
    for index in 0..entry_count {
        let name = format!("file_{index:09}");
        let generated = SnapshotEntry {
            parent: volume_key.to_string(),
            node: FsNode {
                id: format!("file-{index:09}"),
                name,
                parent_id: None,
                kind: NodeKind::File,
                size: index as u64,
                deleted: false,
                inode: Some(index as u64),
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            },
            path: format!("/file_{index:09}"),
        };
        write_snapshot_entry_frame(&mut encoder, &generated, &mut scratch)?;
    }
    encoder.finish()?;
    Ok(())
}

fn write_snapshot_entry_frame<W: Write>(
    writer: &mut W,
    entry: &SnapshotEntry,
    scratch: &mut Vec<u8>,
) -> anyhow::Result<()> {
    scratch.clear();
    bincode::serde::encode_into_std_write(entry, scratch, bincode::config::standard())?;
    let len: u32 = scratch
        .len()
        .try_into()
        .context("snapshot entry frame exceeds u32 length")?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(scratch.as_slice())?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessMemorySample {
    current_rss_bytes: u64,
    peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    entries: usize,
    rows_indexed: usize,
    case_dir: String,
    snapshot_path: String,
    snapshot_bytes: u64,
    write_ms: f64,
    index_ms: f64,
    first_page_ms: f64,
    second_page_ms: f64,
    child_count_ms: f64,
    page_size: usize,
    first_page_items: usize,
    second_page_items: usize,
    first_page_total: Option<usize>,
    child_count: u64,
    memory: MemoryReport,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryReport {
    start: Option<ProcessMemorySample>,
    after_write: Option<ProcessMemorySample>,
    after_index: Option<ProcessMemorySample>,
    after_query: Option<ProcessMemorySample>,
    peak_growth_bytes: Option<u64>,
    peak_growth_human: Option<String>,
    max_peak_rss_growth_mib: Option<u64>,
}

fn process_memory_sample() -> Option<ProcessMemorySample> {
    #[cfg(windows)]
    {
        return windows_process_memory_sample();
    }

    #[cfg(target_os = "linux")]
    {
        return linux_process_memory_sample();
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_process_memory_sample() -> Option<ProcessMemorySample> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let process = unsafe { GetCurrentProcess() };
    let ok = unsafe { GetProcessMemoryInfo(process, &mut counters, counters.cb) };
    if ok == 0 {
        return None;
    }
    Some(ProcessMemorySample {
        current_rss_bytes: counters.working_set_size as u64,
        peak_rss_bytes: Some(counters.peak_working_set_size as u64),
    })
}

#[cfg(target_os = "linux")]
fn linux_process_memory_sample() -> Option<ProcessMemorySample> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let current_rss_bytes = parse_proc_status_kib(&status, "VmRSS:")?;
    let peak_rss_bytes = parse_proc_status_kib(&status, "VmHWM:");
    Some(ProcessMemorySample {
        current_rss_bytes,
        peak_rss_bytes,
    })
}

#[cfg(target_os = "linux")]
fn parse_proc_status_kib(status: &str, key: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix(key)?;
        let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
        kib.checked_mul(1024)
    })
}

fn peak_rss_growth_bytes(samples: &[Option<ProcessMemorySample>]) -> Option<u64> {
    let mut peaks = samples
        .iter()
        .filter_map(|sample| sample.as_ref()?.peak_rss_bytes);
    let start = peaks.next()?;
    let max_peak = peaks.max().unwrap_or(start);
    Some(max_peak.saturating_sub(start))
}

fn format_mib(bytes: u64) -> String {
    format!("{:.1}MiB", bytes as f64 / 1024.0 / 1024.0)
}
