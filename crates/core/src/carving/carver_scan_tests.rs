use std::sync::Arc;

use super::*;
use crate::carving::carver::CarveScanConfig;

const MIB: usize = 1024 * 1024;

#[derive(Clone)]
struct MemorySource {
    bytes: Arc<Vec<u8>>,
    /// Reads touching this offset fail, to exercise error propagation.
    failing_offset: Option<u64>,
}

impl MemorySource {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(bytes),
            failing_offset: None,
        }
    }
}

impl CarveSource for MemorySource {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
        if let Some(failing) = self.failing_offset {
            if (offset..offset + destination.len() as u64).contains(&failing) {
                anyhow::bail!("synthetic read failure at {failing}");
            }
        }
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.bytes.len());
        let length = destination.len().min(self.bytes.len() - start);
        destination[..length].copy_from_slice(&self.bytes[start..start + length]);
        Ok(length)
    }

    fn worker_handle(&self) -> Self {
        self.clone()
    }
}

fn png(payload: usize) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    let data = (0..payload)
        .map(|index| (index * 7) as u8)
        .collect::<Vec<_>>();
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn jpeg(entropy: usize) -> Vec<u8> {
    let mut out = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
    out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    out.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 1, 2, 3, 4, 5, 6]);
    for index in 0..entropy {
        match index % 997 {
            0 => out.extend_from_slice(&[0xff, 0x00]),
            500 => out.extend_from_slice(&[0xff, 0xd3]),
            _ => out.push((index % 251) as u8),
        }
    }
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

fn pdf(body: usize, updates: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    out.extend(std::iter::repeat_n(b'a', body));
    out.extend_from_slice(b"\n%%EOF\n");
    for _ in 0..updates {
        out.extend(std::iter::repeat_n(b'u', body / 3));
        out.extend_from_slice(b"\n%%EOF\n");
    }
    out
}

fn zip(member: usize) -> Vec<u8> {
    let data = vec![b'z'; member];
    let mut out = b"PK\x03\x04".to_vec();
    out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(member as u32).to_le_bytes());
    out.extend_from_slice(&(member as u32).to_le_bytes());
    out.extend_from_slice(&5u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(b"a.txt");
    out.extend_from_slice(&data);
    out.extend_from_slice(b"PK\x01\x02");
    let mut central = [0u8; 42];
    central[24..26].copy_from_slice(&5u16.to_le_bytes());
    out.extend_from_slice(&central);
    out.extend_from_slice(b"a.txt");
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0u8; 18]);
    out
}

/// Deterministic evidence: noise full of weak signatures (PCX, BMP, MP3)
/// plus real files, several straddling the 4 MiB window boundaries.
fn evidence() -> Vec<u8> {
    let mut bytes = vec![0u8; 13 * MIB + 4096];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for (index, byte) in bytes.iter_mut().enumerate() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = if index % 3 == 0 { 0 } else { state as u8 };
    }
    for block in (0..bytes.len()).step_by(512).skip(3).step_by(29) {
        let weak: &[u8] = match block % 3 {
            0 => b"\x0a\x05\x01\x08",
            1 => b"BM",
            _ => b"\xff\xfb\x90\x64",
        };
        bytes[block..block + weak.len()].copy_from_slice(weak);
    }
    let files = [
        (512 * 10, png(3000)),
        (512 * 40, jpeg(20_000)),
        (4 * MIB - 1024, png(9000)),
        (4 * MIB + 512 * 30, pdf(50_000, 2)),
        (8 * MIB - 512 * 3, jpeg(300_000)),
        (8 * MIB + 512 * 700, zip(40_000)),
        (8 * MIB + 512 * 900, pdf(4096, 0)),
        (12 * MIB - 512, zip(2000)),
        (13 * MIB - 1024, png(100)),
    ];
    for (offset, file) in files {
        bytes[offset..offset + file.len()].copy_from_slice(&file);
    }
    bytes
}

fn regions_for(len: u64) -> Vec<Vec<CarveRegion>> {
    let region = |offset: u64, length: u64, label: &str| CarveRegion {
        offset,
        length,
        label: label.to_string(),
    };
    vec![
        vec![region(0, len, "whole")],
        vec![
            region(512 * 7, 5 * MIB as u64 + 123, "first"),
            region(len, 4096, "past end"),
            region(6 * MIB as u64, 0, "empty"),
            region(7 * MIB as u64 + 512, len, "tail"),
        ],
    ]
}

#[derive(Debug, PartialEq)]
enum Event {
    Artifact(u64, String, u64, u64, String, String),
    Progress(u64, u64, u64, u64, u64),
}

fn record(events: &mut Vec<Event>, event: CarveScanEvent) {
    events.push(match event {
        CarveScanEvent::Artifact(artifact) => Event::Artifact(
            artifact.sequence,
            artifact.id,
            artifact.offset,
            artifact.length,
            artifact.extension,
            artifact.region,
        ),
        CarveScanEvent::Progress(progress) => Event::Progress(
            progress.bytes_scanned,
            progress.bytes_total,
            progress.signatures_found,
            progress.artifacts_found,
            progress.current_offset,
        ),
    });
}

/// The single-threaded scan loop the scheduler replaced, kept as the oracle.
fn sequential_scan(
    carver: &Carver,
    source: &MemorySource,
    regions: &[CarveRegion],
    events: &mut Vec<Event>,
) -> Result<CarveScanOutput, String> {
    let cancellation = CancellationToken::new();
    let bytes_total = regions.iter().map(|region| region.length).sum();
    let (mut bytes_scanned, mut signatures_found, mut artifacts_found) = (0u64, 0u64, 0u64);
    let mut buffer = vec![0u8; SCAN_BUFFER_BYTES];
    let mut lookahead = Vec::new();
    for region in regions {
        let region_end = (region.offset + region.length).min(source.len());
        let mut chunk_offset = region.offset.min(region_end);
        while chunk_offset < region_end {
            let wanted = (region_end - chunk_offset).min(SCAN_BUFFER_BYTES as u64) as usize;
            let read = source
                .read_into(chunk_offset, &mut buffer[..wanted])
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            let mut block_index = 0usize;
            while block_index < read.div_ceil(BLOCK_BYTES as usize) {
                let local = block_index * BLOCK_BYTES as usize;
                let block_data = &buffer[local..read];
                let mut accepted_size = 0u64;
                for format in &carver.signature_candidates[usize::from(block_data[0])] {
                    if !format
                        .signatures()
                        .iter()
                        .any(|signature| block_data.starts_with(signature))
                    {
                        continue;
                    }
                    signatures_found += 1;
                    let candidate = Candidate {
                        offset: chunk_offset + local as u64,
                        window: block_data,
                        region_end,
                        max_file_size: carver.max_file_size,
                    };
                    let Ok(Some(detected)) = carver_formats::identify(
                        *format,
                        source,
                        &candidate,
                        &mut lookahead,
                        &cancellation,
                    ) else {
                        continue;
                    };
                    artifacts_found += 1;
                    record(
                        events,
                        CarveScanEvent::Artifact(CarveArtifact {
                            id: format!("carve-{artifacts_found:08}"),
                            name: String::new(),
                            format_id: format.id().to_string(),
                            extension: detected.extension,
                            category: format.category().to_string(),
                            offset: candidate.offset,
                            length: detected.length,
                            confidence: 100,
                            region: region.label.clone(),
                            read_fault: false,
                            sequence: artifacts_found,
                        }),
                    );
                    accepted_size = detected.length;
                    break;
                }
                if artifacts_found as usize >= carver.max_results {
                    bytes_scanned += read as u64;
                    let progress = (
                        bytes_scanned,
                        bytes_total,
                        signatures_found,
                        artifacts_found,
                        chunk_offset + read as u64,
                    );
                    events.push(Event::Progress(
                        progress.0, progress.1, progress.2, progress.3, progress.4,
                    ));
                    return Ok(CarveScanOutput {
                        bytes_scanned,
                        bytes_total,
                        signatures_found,
                        artifacts_found,
                        truncated: true,
                    });
                }
                block_index += if accepted_size > 0 {
                    accepted_size.div_ceil(BLOCK_BYTES).max(1) as usize
                } else {
                    1
                };
            }
            chunk_offset += read as u64;
            bytes_scanned += read as u64;
            events.push(Event::Progress(
                bytes_scanned,
                bytes_total,
                signatures_found,
                artifacts_found,
                chunk_offset,
            ));
        }
    }
    Ok(CarveScanOutput {
        bytes_scanned,
        bytes_total,
        signatures_found,
        artifacts_found,
        truncated: false,
    })
}

fn carver(max_results: usize, worker_threads: usize) -> Carver {
    Carver::try_new(&CarveScanConfig {
        max_results,
        worker_threads: Some(worker_threads),
        ..CarveScanConfig::default()
    })
    .expect("carver")
}

fn parallel_scan(
    carver: &Carver,
    source: &MemorySource,
    regions: &[CarveRegion],
) -> (Result<CarveScanOutput, String>, Vec<Event>) {
    let mut events = Vec::new();
    let output = carver
        .scan_regions(source, regions, &CancellationToken::new(), |event| {
            record(&mut events, event)
        })
        .map_err(|error| error.to_string());
    (output, events)
}

#[test]
fn window_plan_skips_empty_regions_and_clamps_to_the_source() {
    let regions = regions_for(10 * MIB as u64).pop().expect("regions");
    let plan = WindowPlan::new(&regions, 10 * MIB as u64).expect("plan");
    let windows = (0..plan.window_count)
        .map(|index| {
            let window = plan.window(index);
            (
                window.region,
                window.offset,
                window.length,
                window.region_end,
            )
        })
        .collect::<Vec<_>>();
    let first_end = 512 * 7 + 5 * MIB as u64 + 123;
    let tail = 7 * MIB as u64 + 512;
    assert_eq!(
        windows,
        vec![
            (0, 512 * 7, 4 * MIB, first_end),
            (0, 512 * 7 + 4 * MIB as u64, MIB + 123, first_end),
            (3, tail, 3 * MIB - 512, 10 * MIB as u64),
        ]
    );
}

#[test]
fn parallel_scan_matches_the_sequential_block_walk() {
    let source = MemorySource::new(evidence());
    for regions in regions_for(source.len()) {
        let mut expected = Vec::new();
        let expected_output = sequential_scan(
            &carver(MAX_RESULTS_FOR_TESTS, 1),
            &source,
            &regions,
            &mut expected,
        );
        let artifacts = expected
            .iter()
            .filter(|event| matches!(event, Event::Artifact(..)))
            .count();
        assert!(
            artifacts >= 8,
            "fixture should carve its files: {expected:?}"
        );
        for threads in [1, 2, 3, 8] {
            let (output, events) =
                parallel_scan(&carver(MAX_RESULTS_FOR_TESTS, threads), &source, &regions);
            assert_eq!(output, expected_output, "threads={threads}");
            assert_eq!(events, expected, "threads={threads}");
        }
    }
}

const MAX_RESULTS_FOR_TESTS: usize = 100_000;

#[test]
fn result_cap_truncates_at_the_same_artifact_as_the_sequential_scan() {
    let source = MemorySource::new(evidence());
    let regions = regions_for(source.len()).remove(0);
    for max_results in 1..=10 {
        let mut expected = Vec::new();
        let expected_output =
            sequential_scan(&carver(max_results, 1), &source, &regions, &mut expected);
        for threads in [1, 4] {
            let (output, events) = parallel_scan(&carver(max_results, threads), &source, &regions);
            assert_eq!(
                output, expected_output,
                "max={max_results} threads={threads}"
            );
            assert_eq!(events, expected, "max={max_results} threads={threads}");
        }
    }
}

#[test]
fn read_failure_surfaces_after_earlier_windows_are_reported() {
    let mut source = MemorySource::new(evidence());
    source.failing_offset = Some(9 * MIB as u64);
    let regions = regions_for(source.len()).remove(0);
    let mut expected = Vec::new();
    let expected_output = sequential_scan(&carver(100, 1), &source, &regions, &mut expected);
    assert!(expected_output.is_err());
    for threads in [1, 3] {
        let (output, events) = parallel_scan(&carver(100, threads), &source, &regions);
        assert!(output.is_err(), "threads={threads}");
        assert_eq!(events, expected, "threads={threads}");
    }
}

#[test]
fn cancellation_stops_before_the_next_window() {
    let source = MemorySource::new(evidence());
    let regions = regions_for(source.len()).remove(0);
    let cancellation = CancellationToken::new();
    let mut progress_events = 0;
    let output = carver(100, 4).scan_regions(&source, &regions, &cancellation, |event| {
        if matches!(event, CarveScanEvent::Progress(_)) {
            progress_events += 1;
            cancellation.cancel();
        }
    });
    assert!(matches!(output, Err(CarveScanError::Cancelled)));
    assert_eq!(progress_events, 1);
}
