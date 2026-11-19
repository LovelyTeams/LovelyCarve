//! Bounded fallback for images whose partition table is missing.

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

use super::volume_probe::{apfs_size, has_ext_magic, has_ntfs_oem};
use super::{DetectCtx, Partition, PartitionKind, PartitionSlice, SectorSource};
use crate::core::common::error::Failure;

const MIB: usize = 1024 * 1024;
const PREFIX_BYTES: u64 = 16 * MIB as u64;
const WINDOW_BYTES: usize = 2 * MIB;
const HEADER_BYTES: usize = 2048;
const LATER_OFFSETS_MIB: [u64; 7] = [32, 64, 100, 128, 256, 512, 1024];
const MAX_VOLUMES: usize = 16;

pub(in crate::core::evidence) fn scan(
    source: &dyn SectorSource,
    sector_size: usize,
    cancel: &CancellationToken,
) -> Result<Vec<Partition>> {
    if !matches!(sector_size, 512 | 1024 | 2048 | 4096) {
        return Ok(Vec::new());
    }
    let sector = sector_size as u64;
    let size = source.size();
    let prefix_end = size.min(PREFIX_BYTES);
    let prefix = (0..prefix_end).step_by(WINDOW_BYTES).map(|offset| {
        let span = (prefix_end - offset).min(WINDOW_BYTES as u64) as usize;
        (offset, span)
    });
    let later = LATER_OFFSETS_MIB
        .into_iter()
        .map(|mib| (mib * MIB as u64, sector_size));
    let mut partitions: Vec<Partition> = Vec::new();
    let mut claimed_end = sector; // Offset zero already went through whole-volume detection.
    for (base, span) in prefix.chain(later) {
        ensure!(
            !cancel.is_cancelled(),
            Failure::cancelled("image load cancelled")
        );
        if base + span as u64 <= claimed_end || size.saturating_sub(base) < sector {
            continue;
        }
        // Include enough lookahead for a superblock crossing the window boundary.
        let length =
            (size - base).min((span + HEADER_BYTES.saturating_sub(sector_size)) as u64) as usize;
        let bytes = read_exact(source, base, length)?;
        for relative in (0..span.min(bytes.len())).step_by(sector_size) {
            let start = base + relative as u64;
            if start < claimed_end {
                continue;
            }
            let end = (relative + HEADER_BYTES).min(bytes.len());
            let head = &bytes[relative..end];
            let slice = PartitionSlice::new(source, start, size - start);
            let ctx = DetectCtx::for_source(&slice, head, sector_size);
            let Some((kind, length)) = volume_size(&ctx)
                .with_context(|| format!("validating filesystem signature at byte {start}"))?
            else {
                continue;
            };
            if length == 0 || length > slice.size() || !length.is_multiple_of(sector) {
                continue;
            }
            partitions.push(Partition {
                id: u8::try_from(partitions.len())?,
                kind,
                start_lba: start / sector,
                length_lba: length / sector,
                label: None,
                unique_guid: None,
                amiga_geometry: None,
            });
            claimed_end = start + length;
            if partitions.len() == MAX_VOLUMES {
                return Ok(partitions);
            }
        }
    }
    Ok(partitions)
}

fn volume_size(ctx: &DetectCtx<'_>) -> Result<Option<(PartitionKind, u64)>> {
    if has_ntfs_oem(ctx.head) {
        return Ok(ntfs_size(ctx)?.map(|length| (PartitionKind::Ntfs, length)));
    }
    if ctx.head.get(32..36) == Some(b"NXSB") {
        return Ok(apfs_size(ctx)?.map(|length| (PartitionKind::ApfsContainer, length)));
    }
    Ok(has_ext_magic(ctx.head)
        .then(|| ext4_view::Ext4::size_from_superblock(&ctx.head[1024..]))
        .flatten()
        .map(|length| (PartitionKind::Ext4, length)))
}

fn ntfs_size(ctx: &DetectCtx<'_>) -> Result<Option<u64>> {
    let Ok(ntfs) = ntfs::Ntfs::new(&mut Cursor::new(ctx.head)) else {
        return Ok(None);
    };
    // The boot-sector count excludes the final backup boot sector.
    let Some(length) = ntfs.size().checked_add(u64::from(ntfs.sector_size())) else {
        return Ok(None);
    };
    let Some(mft) = ntfs.mft_position().value() else {
        return Ok(None);
    };
    let mft = mft.get();
    if length > ctx.size
        || mft
            .checked_add(u64::from(ntfs.file_record_size()))
            .is_none_or(|end| end > ntfs.size())
    {
        return Ok(None);
    }
    let signature = read_exact(ctx.source, mft, 4)?;
    Ok((signature == b"FILE").then_some(length))
}

fn read_exact(source: &dyn SectorSource, offset: u64, length: usize) -> Result<Vec<u8>> {
    let bytes = source.read(offset, length)?;
    ensure!(
        bytes.len() == length,
        Failure::corrupt(format!(
            "short read during filesystem discovery at byte {offset}"
        ))
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::test_support::BufSource;

    const EXT: &[u8] = include_bytes!("../../../../vendor/ext4-view/test_data/audit_ext.bin");

    #[test]
    fn recovers_ext_across_window_boundary_and_at_later_alignment() {
        let starts = [WINDOW_BYTES - 512, 32 * MIB];
        let mut bytes = vec![0; starts[1] + EXT.len()];
        for start in starts {
            bytes[start..start + EXT.len()].copy_from_slice(EXT);
        }
        let recovered = scan(&BufSource(bytes), 512, &CancellationToken::new()).unwrap();
        assert_eq!(
            recovered
                .iter()
                .map(|part| part.start_lba)
                .collect::<Vec<_>>(),
            starts.map(|start| (start / 512) as u64)
        );
    }

    #[test]
    fn ignores_unaligned_and_truncated_ext_candidates() {
        let mut bytes = vec![0; 2 * MIB + EXT.len() - 4096];
        let unaligned = MIB + 512;
        bytes[unaligned..unaligned + EXT.len()].copy_from_slice(EXT);
        bytes[2 * MIB..].copy_from_slice(&EXT[..EXT.len() - 4096]);
        assert!(scan(&BufSource(bytes), 4096, &CancellationToken::new())
            .unwrap()
            .is_empty());
    }
}
