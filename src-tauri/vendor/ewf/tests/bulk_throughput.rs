//! Opt-in full logical-stream benchmark; never writes to the evidence.
#![cfg(feature = "verify")]
use std::sync::{mpsc, Arc};
use std::time::Instant;

use ewf::EwfReader;
use md5::{Digest, Md5};

#[test]
#[ignore = "set EWF_BENCH_IMAGE and run in release mode"]
fn bulk_throughput() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var_os("EWF_BENCH_IMAGE").ok_or("set EWF_BENCH_IMAGE")?;
    let paths = ewf::discover_segments(std::path::Path::new(&path))?;
    let reader = EwfReader::open_segments_lazy(&paths, 128, 8)?;
    let passes: usize = std::env::var("EWF_BENCH_PASSES")
        .unwrap_or_else(|_| "1".into())
        .parse()?;
    let mode = std::env::var("EWF_BENCH_MODE").unwrap_or_else(|_| "md5".into());
    if !matches!(mode.as_str(), "md5" | "sha1" | "read") {
        return Err("unknown benchmark mode".into());
    }
    let readers = std::env::var("EWF_BENCH_READERS").unwrap_or_else(|_| "serial,bulk".into());
    eprintln!(
        "logical_bytes={} chunk_bytes={} segments={} threads={} mode={mode}",
        reader.total_size(),
        reader.chunk_size(),
        paths.len(),
        rayon::current_num_threads()
    );
    for pass in 0..passes {
        for strategy in readers.split(',') {
            if !matches!(strategy, "serial" | "bulk") {
                return Err("unknown reader strategy".into());
            }
            let started = Instant::now();
            let digest = std::thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
                let (sender, receiver) = mpsc::sync_channel::<Arc<Vec<u8>>>(2);
                let mode = &mode;
                let worker = scope.spawn(move || {
                    let mut digest = Md5::new();
                    let mut sha1 = sha1::Sha1::new();
                    while let Ok(bytes) = receiver.recv() {
                        match mode.as_str() {
                            "md5" => digest.update(bytes.as_slice()),
                            "sha1" => sha1.update(bytes.as_slice()),
                            _ => {}
                        }
                    }
                    match mode.as_str() {
                        "md5" => digest.finalize().to_vec(),
                        "sha1" => sha1.finalize().to_vec(),
                        _ => Vec::new(),
                    }
                });
                let mut offset = 0;
                while offset < reader.total_size() {
                    let want = (reader.total_size() - offset).min(16 * 1024 * 1024) as usize;
                    let mut bytes = vec![0; want];
                    let count = if strategy == "serial" {
                        let mut count = 0;
                        // Same cached per-chunk decode/copy loop as the old bulk
                        // read, kept below the automatic parallel threshold.
                        for destination in bytes.chunks_mut(reader.chunk_size().min(65536) as usize)
                        {
                            count += reader.read_at(destination, offset + count as u64)?;
                        }
                        count
                    } else {
                        reader.read_at(&mut bytes, offset)?
                    };
                    if count != want {
                        return Err("short full-image read".into());
                    }
                    offset += count as u64;
                    sender.send(Arc::new(bytes))?;
                }
                drop(sender);
                worker.join().map_err(|_| "hasher panicked".into())
            })?;
            let stored = match mode.as_str() {
                "md5" => reader.stored_hashes().md5.map(|value| value.to_vec()),
                "sha1" => reader.stored_hashes().sha1.map(|value| value.to_vec()),
                _ => None,
            };
            if let Some(stored) = stored {
                assert_eq!(
                    digest, stored,
                    "full logical stream must match acquisition digest"
                );
            }
            let seconds = started.elapsed().as_secs_f64();
            eprintln!("pass={} reader={strategy} seconds={seconds:.3} MiB_per_second={:.1} digest={digest:02x?}",
            pass + 1, reader.total_size() as f64 / 1048576.0 / seconds);
        }
    }
    Ok(())
}
