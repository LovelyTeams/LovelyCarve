//! Raw file-carving probe.
//!
//! Usage: `just run-example probe_carve <image> [--max-results N]
//! [--max-size-mib N] [--threads N] [--dump <tsv>] [format-id ...]`
//!
//! `--dump` writes every artifact as `offset<TAB>length<TAB>extension` in scan
//! order, so two builds can be compared with a plain diff; `probe_carve_truth`
//! writes the matching ground truth for images with an intact ext4 volume.
//! `--threads` overrides the worker count (default: cores, at most 8).

use std::collections::BTreeMap;
use std::env;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context};
use lovelycarve_core::carving::carver::{
    CarveArtifact, CarveRegion, CarveScanConfig, CarveScanEvent, Carver, DEFAULT_MAX_FILE_SIZE,
    DEFAULT_MAX_RESULTS,
};
use lovelycarve_core::evidence::image::DiskImage;
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let Some(image_path) = arguments.first() else {
        bail!(
            "usage: probe_carve <image> [--max-results N] [--max-size-mib N] [--threads N] [--dump <tsv>] [format-id ...]"
        );
    };
    let ProbeOptions {
        max_results,
        max_file_size,
        dump_path,
        worker_threads,
        format_ids,
    } = parse_options(&arguments[1..])?;
    let mut dump = dump_path
        .as_ref()
        .map(|path| {
            std::fs::File::create(path)
                .map(BufWriter::new)
                .with_context(|| format!("create {}", path.display()))
        })
        .transpose()?;
    let mut dump_error = None;
    let image = Arc::new(
        DiskImage::open(image_path).with_context(|| format!("open disk image {image_path}"))?,
    );
    let image_info = image.info();
    let region = CarveRegion {
        offset: 0,
        length: image_info.size_bytes,
        label: "whole-image".to_string(),
    };
    let config = CarveScanConfig {
        format_ids,
        max_file_size,
        max_results,
        worker_threads,
    };
    let carver = Carver::try_new(&config)?;
    let started = Instant::now();
    let mut next_percentage = 0u64;
    let mut counts = BTreeMap::<String, u64>::new();
    let mut samples = Vec::<CarveArtifact>::new();

    println!("image:       {}", image_info.path);
    println!("format:      {:?}", image_info.source_format);
    println!("logical size: {}", human(image_info.size_bytes));
    println!("max artifact: {}", human(max_file_size));
    println!("result cap:   {max_results}");

    let output =
        carver.scan_regions(
            &image,
            &[region],
            &CancellationToken::new(),
            |event| match event {
                CarveScanEvent::Artifact(artifact) => {
                    if let Some(dump) = dump.as_mut() {
                        if let Err(error) = writeln!(
                            dump,
                            "{}\t{}\t{}",
                            artifact.offset, artifact.length, artifact.extension
                        ) {
                            dump_error.get_or_insert(error);
                        }
                    }
                    *counts.entry(artifact.extension.clone()).or_default() += 1;
                    if samples.len() < 50 {
                        samples.push(artifact);
                    }
                }
                CarveScanEvent::Progress(progress) => {
                    let percentage = progress
                        .bytes_scanned
                        .saturating_mul(100)
                        .checked_div(progress.bytes_total.max(1))
                        .unwrap_or(100)
                        .min(100);
                    if percentage >= next_percentage {
                        println!(
                            "progress: {percentage:>3}%  scanned={}  found={}",
                            human(progress.bytes_scanned),
                            progress.artifacts_found
                        );
                        next_percentage = percentage.saturating_add(5);
                    }
                }
            },
        )?;

    if let Some(error) = dump_error {
        return Err(error).context("write artifact dump");
    }
    if let Some(mut dump) = dump {
        dump.flush().context("flush artifact dump")?;
    }
    println!("elapsed:      {:.2?}", started.elapsed());
    println!("scanned:      {}", human(output.bytes_scanned));
    println!("signatures:   {}", output.signatures_found);
    println!("found:        {}", output.artifacts_found);
    println!("truncated:    {}", output.truncated);
    println!("by format:    {counts:?}");
    println!("first {} findings:", samples.len());
    for artifact in samples {
        println!(
            "  {:<8} offset=0x{:012X} length={:<12} confidence={} fault={}",
            artifact.extension,
            artifact.offset,
            human(artifact.length),
            artifact.confidence,
            artifact.read_fault
        );
    }
    Ok(())
}

struct ProbeOptions {
    max_results: usize,
    max_file_size: u64,
    dump_path: Option<PathBuf>,
    worker_threads: Option<usize>,
    format_ids: Vec<String>,
}

fn parse_options(arguments: &[String]) -> anyhow::Result<ProbeOptions> {
    let mut max_results = DEFAULT_MAX_RESULTS;
    let mut max_file_size = DEFAULT_MAX_FILE_SIZE;
    let mut dump_path = None;
    let mut worker_threads = None;
    let mut format_ids = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--threads" => {
                let value = arguments
                    .get(index + 1)
                    .context("--threads needs a value")?;
                worker_threads = Some(value.parse().context("parse --threads")?);
                index += 2;
            }
            "--dump" => {
                let value = arguments.get(index + 1).context("--dump needs a path")?;
                dump_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--max-results" => {
                let value = arguments
                    .get(index + 1)
                    .context("--max-results needs a value")?;
                max_results = value.parse().context("parse --max-results")?;
                index += 2;
            }
            "--max-size-mib" => {
                let value = arguments
                    .get(index + 1)
                    .context("--max-size-mib needs a value")?;
                let mebibytes = value.parse::<u64>().context("parse --max-size-mib")?;
                max_file_size = mebibytes
                    .checked_mul(1024 * 1024)
                    .context("--max-size-mib is too large")?;
                index += 2;
            }
            format_id if format_id.starts_with('-') => bail!("unknown option {format_id}"),
            format_id => {
                format_ids.push(format_id.to_string());
                index += 1;
            }
        }
    }
    Ok(ProbeOptions {
        max_results,
        max_file_size,
        dump_path,
        worker_threads,
        format_ids,
    })
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}
