//! Parallel window scheduler for the signature carver.
//!
//! The scan stays Digler's block walk: each 4 MiB window is examined at
//! 512-byte block boundaries and an accepted artifact skips blocks only inside
//! its own window, so windows are independent. Workers claim windows in order,
//! scan them with a private evidence handle, and the calling thread merges the
//! results back in window order. Sequence numbers, progress and the result cap
//! are applied during that ordered merge, so the output is identical to a
//! single-threaded scan.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context};
use tokio_util::sync::CancellationToken;

use crate::carving::carver::{
    CarveArtifact, CarveRegion, CarveScanError, CarveScanEvent, CarveScanOutput, CarveScanProgress,
    CarveSource, Carver, FormatKind, BLOCK_BYTES, SCAN_BUFFER_BYTES,
};
use crate::carving::carver_formats::{self, Candidate};

/// How many windows each worker may run ahead of the ordered merge. Bounds
/// memory and lets a blocked observer (a paused scan) stall the workers.
const WINDOWS_AHEAD_PER_WORKER: usize = 4;
const MERGE_POLL: Duration = Duration::from_millis(50);

/// Regions clamped to the source, with the index of their first window.
struct WindowPlan {
    regions: Vec<PlannedRegion>,
    window_count: usize,
}

struct PlannedRegion {
    index: usize,
    start: u64,
    end: u64,
    first_window: usize,
}

#[derive(Clone, Copy)]
struct ScanWindow {
    region: usize,
    offset: u64,
    length: usize,
    region_end: u64,
}

impl WindowPlan {
    fn new(regions: &[CarveRegion], source_len: u64) -> anyhow::Result<Self> {
        let mut planned = Vec::with_capacity(regions.len());
        let mut window_count = 0usize;
        for (index, region) in regions.iter().enumerate() {
            let end = region
                .offset
                .checked_add(region.length)
                .context("carve region range overflow")?
                .min(source_len);
            let start = region.offset.min(end);
            let windows = usize::try_from((end - start).div_ceil(SCAN_BUFFER_BYTES as u64))
                .context("carve region has too many windows")?;
            planned.push(PlannedRegion {
                index,
                start,
                end,
                first_window: window_count,
            });
            window_count = window_count
                .checked_add(windows)
                .context("carve plan has too many windows")?;
        }
        Ok(Self {
            regions: planned,
            window_count,
        })
    }

    fn window(&self, window: usize) -> ScanWindow {
        let position = self
            .regions
            .partition_point(|region| region.first_window <= window);
        let region = &self.regions[position.saturating_sub(1)];
        let offset =
            region.start + (window - region.first_window) as u64 * SCAN_BUFFER_BYTES as u64;
        ScanWindow {
            region: region.index,
            offset,
            length: (region.end - offset).min(SCAN_BUFFER_BYTES as u64) as usize,
            region_end: region.end,
        }
    }
}

/// An artifact accepted inside one window, before merge-time numbering.
struct WindowArtifact {
    offset: u64,
    length: u64,
    format: FormatKind,
    extension: String,
    name: Option<String>,
    read_fault: bool,
    /// Signature hits in this window up to and including this artifact.
    signatures_through: u64,
}

struct WindowScan {
    bytes_read: usize,
    signatures: u64,
    artifacts: Vec<WindowArtifact>,
}

/// Per-worker buffers reused across windows.
struct WorkerBuffers {
    window: Vec<u8>,
    lookahead: Vec<u8>,
}

#[derive(Default)]
struct GateState {
    next: usize,
    merged: usize,
    stopped: bool,
}

/// Hands out windows in order while keeping workers a bounded distance ahead
/// of the merge.
struct WindowGate {
    state: Mutex<GateState>,
    changed: Condvar,
    window_count: usize,
    ahead: usize,
}

impl WindowGate {
    fn claim(&self) -> Option<usize> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if state.stopped || state.next >= self.window_count {
                return None;
            }
            if state.next < state.merged + self.ahead {
                state.next += 1;
                return Some(state.next - 1);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn merged(&self, merged: usize) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .merged = merged;
        self.changed.notify_all();
    }

    fn stop(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped = true;
        self.changed.notify_all();
    }
}

type WindowMessage = (usize, Result<WindowScan, CarveScanError>);

impl Carver {
    pub(crate) fn scan_parallel<S: CarveSource>(
        &self,
        source: &S,
        regions: &[CarveRegion],
        cancellation: &CancellationToken,
        mut observer: impl FnMut(CarveScanEvent),
    ) -> Result<CarveScanOutput, CarveScanError> {
        let plan = WindowPlan::new(regions, source.len())?;
        let bytes_total = regions.iter().map(|region| region.length).sum();
        let workers = self.worker_threads.clamp(1, plan.window_count.max(1));
        let gate = WindowGate {
            state: Mutex::new(GateState::default()),
            changed: Condvar::new(),
            window_count: plan.window_count,
            ahead: workers * WINDOWS_AHEAD_PER_WORKER,
        };
        // Workers stop on user cancellation and when the merge is done early.
        let worker_stop = cancellation.child_token();
        let (sender, receiver) = mpsc::channel::<WindowMessage>();

        thread::scope(|scope| {
            let mut spawned = Ok(());
            for worker in 0..workers {
                let sender = sender.clone();
                let (gate, plan, worker_stop) = (&gate, &plan, &worker_stop);
                if let Err(error) = thread::Builder::new()
                    .name(format!("carve-worker-{worker}"))
                    .spawn_scoped(scope, move || {
                        self.run_worker(source, plan, gate, worker_stop, &sender);
                    })
                {
                    spawned = Err(CarveScanError::Failed(anyhow!(
                        "spawn carve worker: {error}"
                    )));
                    break;
                }
            }
            drop(sender);
            let merged = spawned.and_then(|()| {
                self.merge_windows(
                    &plan,
                    regions,
                    bytes_total,
                    &gate,
                    &receiver,
                    cancellation,
                    &mut observer,
                )
            });
            // Release workers blocked on the gate before the scope joins them.
            gate.stop();
            worker_stop.cancel();
            merged
        })
    }

    fn run_worker<S: CarveSource>(
        &self,
        source: &S,
        plan: &WindowPlan,
        gate: &WindowGate,
        stop: &CancellationToken,
        sender: &mpsc::Sender<WindowMessage>,
    ) {
        let handle = source.worker_handle();
        let mut buffers = WorkerBuffers {
            window: vec![0u8; SCAN_BUFFER_BYTES],
            lookahead: Vec::new(),
        };
        while let Some(index) = gate.claim() {
            let window = plan.window(index);
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                self.scan_window(&handle, &window, &mut buffers, stop)
            }))
            .unwrap_or_else(|_| {
                Err(CarveScanError::Failed(anyhow!(
                    "carve worker panicked while scanning offset {}",
                    window.offset
                )))
            });
            let failed = result.is_err();
            if sender.send((index, result)).is_err() || failed {
                return;
            }
        }
    }

    /// Digler's block walk over one window.
    fn scan_window<S: CarveSource>(
        &self,
        source: &S,
        window: &ScanWindow,
        buffers: &mut WorkerBuffers,
        cancellation: &CancellationToken,
    ) -> Result<WindowScan, CarveScanError> {
        if cancellation.is_cancelled() {
            return Err(CarveScanError::Cancelled);
        }
        let buffer = &mut buffers.window[..window.length];
        let read = source
            .read_into(window.offset, buffer)
            .with_context(|| format!("read carve scan buffer at offset {}", window.offset))?;
        let data = &buffers.window[..read];
        let mut scan = WindowScan {
            bytes_read: read,
            signatures: 0,
            artifacts: Vec::new(),
        };
        let block_count = read.div_ceil(BLOCK_BYTES as usize);
        let mut block_index = 0usize;
        while block_index < block_count {
            let local = block_index * BLOCK_BYTES as usize;
            let block_data = &data[local..];
            let candidates = self.signature_candidates[usize::from(block_data[0])].as_slice();
            let mut accepted_size = 0u64;
            for format in candidates {
                if !format
                    .signatures()
                    .iter()
                    .any(|signature| block_data.starts_with(signature))
                {
                    continue;
                }
                scan.signatures += 1;
                let candidate = Candidate {
                    offset: window.offset + local as u64,
                    window: block_data,
                    region_end: window.region_end,
                    max_file_size: self.max_file_size,
                };
                let detected = match carver_formats::identify(
                    *format,
                    source,
                    &candidate,
                    &mut buffers.lookahead,
                    cancellation,
                ) {
                    Ok(detected) => detected,
                    Err(CarveScanError::Cancelled) => return Err(CarveScanError::Cancelled),
                    Err(CarveScanError::Failed(_)) => None,
                };
                let Some(detected) = detected else {
                    continue;
                };
                scan.artifacts.push(WindowArtifact {
                    offset: candidate.offset,
                    length: detected.length,
                    format: *format,
                    extension: detected.extension,
                    name: detected.name,
                    read_fault: source.has_read_fault(candidate.offset, detected.length),
                    signatures_through: scan.signatures,
                });
                accepted_size = detected.length;
                break;
            }
            block_index += if accepted_size > 0 {
                accepted_size.div_ceil(BLOCK_BYTES).max(1) as usize
            } else {
                1
            };
        }
        Ok(scan)
    }

    /// Consumes window results in window order, numbering artifacts and
    /// reporting progress exactly as the sequential scan did.
    #[allow(clippy::too_many_arguments)]
    fn merge_windows(
        &self,
        plan: &WindowPlan,
        regions: &[CarveRegion],
        bytes_total: u64,
        gate: &WindowGate,
        receiver: &mpsc::Receiver<WindowMessage>,
        cancellation: &CancellationToken,
        observer: &mut impl FnMut(CarveScanEvent),
    ) -> Result<CarveScanOutput, CarveScanError> {
        // A failed window surfaces only when the merge reaches it, after every
        // earlier window's artifacts, as it would have sequentially.
        let mut pending = BTreeMap::<usize, Result<WindowScan, CarveScanError>>::new();
        let mut bytes_scanned = 0u64;
        let mut signatures_found = 0u64;
        let mut artifacts_found = 0u64;
        // A region whose window read nothing ends there, as the sequential
        // scan's `break` did.
        let mut exhausted_region = None;
        let mut next = 0usize;
        while next < plan.window_count {
            if cancellation.is_cancelled() {
                return Err(CarveScanError::Cancelled);
            }
            let Some(scan) = pending.remove(&next) else {
                match receiver.recv_timeout(MERGE_POLL) {
                    Ok((_, Err(CarveScanError::Cancelled))) => {
                        return Err(CarveScanError::Cancelled);
                    }
                    Ok((index, result)) => {
                        pending.insert(index, result);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(CarveScanError::Failed(anyhow!(
                            "carve workers stopped before the scan finished"
                        )));
                    }
                }
                continue;
            };
            let scan = scan?;
            let window = plan.window(next);
            next += 1;
            if exhausted_region == Some(window.region) {
                gate.merged(next);
                continue;
            }
            if scan.bytes_read == 0 {
                exhausted_region = Some(window.region);
                gate.merged(next);
                continue;
            }
            let window_end = window.offset + scan.bytes_read as u64;
            for found in scan.artifacts {
                artifacts_found += 1;
                let sequence = artifacts_found;
                let block = found.offset / BLOCK_BYTES;
                observer(CarveScanEvent::Artifact(CarveArtifact {
                    id: format!("carve-{sequence:08}"),
                    name: found
                        .name
                        .unwrap_or_else(|| format!("f{block}.{}", found.extension)),
                    format_id: found.format.id().to_string(),
                    extension: found.extension,
                    category: found.format.category().to_string(),
                    offset: found.offset,
                    length: found.length,
                    confidence: 100,
                    region: regions[window.region].label.clone(),
                    read_fault: found.read_fault,
                    sequence,
                }));
                if artifacts_found as usize >= self.max_results {
                    signatures_found += found.signatures_through;
                    bytes_scanned += scan.bytes_read as u64;
                    observer(CarveScanEvent::Progress(CarveScanProgress {
                        bytes_scanned,
                        bytes_total,
                        signatures_found,
                        artifacts_found,
                        current_offset: window_end,
                    }));
                    return Ok(CarveScanOutput {
                        bytes_scanned,
                        bytes_total,
                        signatures_found,
                        artifacts_found,
                        truncated: true,
                    });
                }
            }
            signatures_found += scan.signatures;
            bytes_scanned += scan.bytes_read as u64;
            observer(CarveScanEvent::Progress(CarveScanProgress {
                bytes_scanned,
                bytes_total,
                signatures_found,
                artifacts_found,
                current_offset: window_end,
            }));
            gate.merged(next);
        }
        Ok(CarveScanOutput {
            bytes_scanned,
            bytes_total,
            signatures_found,
            artifacts_found,
            truncated: false,
        })
    }
}

#[cfg(test)]
#[path = "carver_scan_tests.rs"]
mod tests;
