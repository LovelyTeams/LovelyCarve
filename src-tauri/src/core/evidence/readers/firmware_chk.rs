//! Netgear CHK bounds and wrapping two-accumulator checksums.
use super::{be32, segments};
use crate::core::evidence::partitions::Partition;
use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

fn checksum(bytes: &[u8], cancel: &CancellationToken) -> Result<u32> {
    let mut first = 0u32;
    let mut second = 0u32;
    for chunk in bytes.chunks(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "CHK checksum cancelled");
        for byte in chunk {
            first = first.wrapping_add(u32::from(*byte));
            second = second.wrapping_add(first);
        }
    }
    let fold = |value: u32| {
        let sum = (value & 0xffff) + (value >> 16);
        (sum + (sum >> 16)) & 0xffff
    };
    Ok((fold(second) << 16) | fold(first))
}

pub(super) fn parse(
    bytes: &[u8],
    base: u64,
    cancel: &CancellationToken,
    depth: usize,
) -> Result<Vec<Partition>> {
    let header_length = be32(bytes, 4)? as usize;
    ensure!(
        (41..=104).contains(&header_length),
        "invalid CHK header/board ID length"
    );
    let mut header = bytes
        .get(..header_length)
        .context("truncated CHK header")?
        .to_vec();
    let expected = be32(&header, 36)?;
    header[36..40].fill(0);
    ensure!(
        checksum(&header, cancel)? == expected,
        "CHK header checksum mismatch"
    );
    ensure!(
        header[40..].iter().all(|byte| byte.is_ascii_graphic()),
        "invalid CHK board ID"
    );
    let kernel_length = be32(bytes, 24)? as usize;
    let root_length = be32(bytes, 28)? as usize;
    ensure!(kernel_length != 0, "empty CHK kernel");
    let kernel_end = header_length
        .checked_add(kernel_length)
        .context("CHK kernel length overflow")?;
    let image_end = kernel_end
        .checked_add(root_length)
        .context("CHK rootfs length overflow")?;
    ensure!(image_end == bytes.len(), "CHK image length mismatch");
    let kernel = bytes
        .get(header_length..kernel_end)
        .context("CHK kernel exceeds source")?;
    let root = bytes
        .get(kernel_end..image_end)
        .context("CHK rootfs exceeds source")?;
    ensure!(
        checksum(kernel, cancel)? == be32(bytes, 16)?,
        "CHK kernel checksum mismatch"
    );
    ensure!(
        checksum(root, cancel)? == be32(bytes, 20)?,
        "CHK rootfs checksum mismatch"
    );
    ensure!(
        checksum(&bytes[header_length..], cancel)? == be32(bytes, 32)?,
        "CHK image checksum mismatch"
    );
    let mut partitions = segments::parse_payload(
        kernel,
        segments::source_offset(base, header_length)?,
        cancel,
        depth + 1,
    )?;
    if !root.is_empty() {
        segments::append_partitions(
            &mut partitions,
            segments::parse_payload(
                root,
                segments::source_offset(base, kernel_end)?,
                cancel,
                depth + 1,
            )?,
        )?;
    }
    for partition in &mut partitions {
        partition.label = Some("Netgear CHK rootfs".into());
    }
    Ok(partitions)
}
