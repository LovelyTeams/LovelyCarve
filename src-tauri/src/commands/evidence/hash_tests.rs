use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;
use std::io::Write;

#[test]
fn image_hash_pipeline_matches_independent_digests() -> anyhow::Result<()> {
    let data: Vec<u8> = (0..1024 * 1024 + 137)
        .map(|index| (index * 131 + 17) as u8)
        .collect();
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(&data)?;
    let image = EvidenceSession::open(file.path())?;
    let output = run_parallel_hash(
        &image,
        &[
            HashAlgo::Md5,
            HashAlgo::Sha1,
            HashAlgo::Sha256,
            HashAlgo::Sm3,
        ],
        &|_, _| {},
        &CancellationToken::new(),
        ReadFaultPolicy::Strict,
    )
    .map_err(|error| anyhow::anyhow!(error.message))?;
    assert_eq!(
        (output.md5, output.sha1, output.sha256, output.sm3),
        (
            Some(hex::encode(md5::Md5::digest(&data))),
            Some(hex::encode(Sha1::digest(&data))),
            Some(hex::encode(Sha256::digest(&data))),
            Some(hex::encode(Sm3::digest(&data))),
        )
    );
    let token = CancellationToken::new();
    token.cancel();
    let error = run_parallel_hash(
        &image,
        &[HashAlgo::Md5],
        &|_, _| {},
        &token,
        ReadFaultPolicy::Strict,
    )
    .expect_err("cancelled hash must not publish a digest");
    assert_eq!(error.code, ErrorCode::Cancelled);
    Ok(())
}

/// Same 16 MiB/bounded-channel pipeline with the previous independent backend.
fn reference_image_md5(image: &EvidenceSession) -> anyhow::Result<String> {
    thread::scope(|scope| -> anyhow::Result<String> {
        let (sender, receiver) = mpsc::sync_channel::<Arc<Vec<u8>>>(CHANNEL_DEPTH);
        let hasher = scope.spawn(move || {
            let mut hasher = md5::Md5::new();
            while let Ok(bytes) = receiver.recv() {
                hasher.update(bytes.as_slice());
            }
            hex::encode(hasher.finalize())
        });
        let mut offset = 0;
        while offset < image.len() {
            let want = (image.len() - offset).min(HASH_CHUNK_BYTES as u64) as usize;
            let mut buffer = vec![0; want];
            let count = image.read_into(offset, &mut buffer)?;
            anyhow::ensure!(count == want, "short reference hash read at {offset}");
            anyhow::ensure!(
                !image.has_bad_region_in_range(offset, count as u64),
                "reference read contains synthetic bytes"
            );
            sender.send(Arc::new(buffer))?;
            offset += count as u64;
        }
        drop(sender);
        hasher
            .join()
            .map_err(|_| anyhow::anyhow!("reference hasher panicked"))
    })
}

#[test]
#[ignore = "set EWF_HASH_IMAGE; full-image release benchmark"]
fn image_hash_external_benchmark() -> anyhow::Result<()> {
    let path =
        std::env::var_os("EWF_HASH_IMAGE").ok_or_else(|| anyhow::anyhow!("set EWF_HASH_IMAGE"))?;
    let image = EvidenceSession::open(std::path::Path::new(&path))?;
    let acquisition = image
        .ewf_acquiry
        .as_ref()
        .and_then(|info| info.stored_md5.as_deref());
    anyhow::ensure!(
        acquisition.is_some(),
        "benchmark requires an acquisition MD5"
    );
    for pass in 0..2 {
        // Alternate order; do not evict or claim to evict the OS file cache.
        for fast in if pass == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            image.validate_source_file_stable()?;
            let started = Instant::now();
            let digest = if fast {
                run_parallel_hash(
                    &image,
                    &[HashAlgo::Md5],
                    &|_, _| {},
                    &CancellationToken::new(),
                    ReadFaultPolicy::Strict,
                )
                .map_err(|error| anyhow::anyhow!(error.message))?
                .md5
                .ok_or_else(|| anyhow::anyhow!("MD5 result missing"))?
            } else {
                reference_image_md5(&image)?
            };
            let seconds = started.elapsed().as_secs_f64();
            image.validate_source_file_stable()?;
            assert_eq!(Some(digest.as_str()), acquisition);
            eprintln!("pass={} backend={} logical_bytes={} seconds={seconds:.3} MiB_per_second={:.1} md5={digest}", pass + 1, if fast { "fast-md5" } else { "md-5" }, image.len(), image.len() as f64 / 1048576.0 / seconds);
        }
    }
    Ok(())
}
