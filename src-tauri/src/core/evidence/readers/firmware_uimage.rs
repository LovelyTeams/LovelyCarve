//! U-Boot legacy images. Validate both CRCs before interpreting filesystem spans.
use super::{be32, segments};
use crate::core::evidence::partitions::Partition;
use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

const HEADER: usize = 64;

pub(super) fn parse(
    bytes: &[u8],
    base: u64,
    cancel: &CancellationToken,
    depth: usize,
) -> Result<Vec<Partition>> {
    let mut header: [u8; HEADER] = bytes
        .get(..HEADER)
        .context("truncated uImage header")?
        .try_into()?;
    let expected = be32(&header, 4)?;
    header[4..8].fill(0);
    ensure!(
        segments::crc32(&header, cancel)? == expected,
        "uImage header CRC mismatch"
    );
    let length = be32(bytes, 12)? as usize;
    let end = HEADER
        .checked_add(length)
        .context("uImage length overflow")?;
    let data = bytes
        .get(HEADER..end)
        .context("uImage data exceeds source")?;
    ensure!(
        segments::crc32(data, cancel)? == be32(bytes, 24)?,
        "uImage data CRC mismatch"
    );
    let mut partitions = Vec::new();
    match bytes[30] {
        4 => {
            ensure!(bytes[31] == 0, "unsupported compressed uImage multi-image");
            let mut lengths = Vec::new();
            let mut table_end = 0usize;
            loop {
                let size = be32(data, table_end)? as usize;
                table_end = table_end
                    .checked_add(4)
                    .context("uImage component table overflow")?;
                if size == 0 {
                    break;
                }
                ensure!(
                    lengths.len() < 32,
                    "uImage component limit or missing terminator"
                );
                lengths.push(size);
            }
            ensure!(!lengths.is_empty(), "empty uImage multi-image");
            let mut start = table_end;
            for (index, size) in lengths.iter().enumerate() {
                let component_end = start
                    .checked_add(*size)
                    .context("uImage component length overflow")?;
                let span = data
                    .get(start..component_end)
                    .context("uImage component exceeds data")?;
                segments::append_partitions(
                    &mut partitions,
                    segments::parse_payload(
                        span,
                        segments::source_offset(base, HEADER + start)?,
                        cancel,
                        depth + 1,
                    )?,
                )?;
                start = if index + 1 == lengths.len() {
                    component_end
                } else {
                    let aligned = component_end
                        .checked_add(3)
                        .context("uImage component alignment overflow")?
                        & !3;
                    ensure!(
                        data.get(component_end..aligned)
                            .is_some_and(|pad| pad.iter().all(|byte| *byte == 0)),
                        "invalid uImage component padding"
                    );
                    aligned
                };
            }
            ensure!(
                start == data.len(),
                "uImage component lengths leave unclaimed data"
            );
        }
        7 => {
            ensure!(bytes[31] == 0, "unsupported compressed uImage filesystem");
            segments::append_partitions(
                &mut partitions,
                segments::parse_payload(
                    data,
                    segments::source_offset(base, HEADER)?,
                    cancel,
                    depth + 1,
                )?,
            )?;
        }
        // Kernels / RAM disks are opaque; compression here is intentionally not
        // interpreted as a filesystem. A rootfs may follow the verified payload.
        2 | 3 | 14 => {}
        kind => anyhow::bail!("unsupported uImage payload type {kind}"),
    }
    if end < bytes.len() {
        segments::append_partitions(
            &mut partitions,
            segments::parse_payload(
                &bytes[end..],
                segments::source_offset(base, end)?,
                cancel,
                depth + 1,
            )?,
        )?;
    }
    Ok(partitions)
}
