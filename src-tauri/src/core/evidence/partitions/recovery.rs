//! Bounded fallback for images whose partition table is missing.

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use apfs::object::{ObjectHeader, OBJECT_TYPE_OMAP};
use ntfs_core::{apply_fixup, parse_attributes, FileName, MftRecordHeader};
use tokio_util::sync::CancellationToken;

use super::volume_probe::{apfs_superblock, has_ext_magic, has_ntfs_oem};
use super::{DetectCtx, Partition, PartitionKind, PartitionSlice, SectorSource};
use crate::core::common::error::Failure;

const MIB: usize = 1024 * 1024;
const PREFIX_BYTES: u64 = 16 * MIB as u64;
const WINDOW_BYTES: usize = 2 * MIB;
const HEADER_BYTES: usize = 2048;
const LATER_OFFSETS_MIB: [u64; 7] = [32, 64, 100, 128, 256, 512, 1024];
const MAX_VOLUMES: usize = 16;
const MAX_MFT_RECORD_BYTES: usize = 64 * 1024;

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
            ensure!(
                !cancel.is_cancelled(),
                Failure::cancelled("image load cancelled")
            );
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
    if let Some(length) = ntfs_size(ctx)? {
        return Ok(Some((PartitionKind::Ntfs, length)));
    }
    if let Some(length) = apfs_size(ctx)? {
        return Ok(Some((PartitionKind::ApfsContainer, length)));
    }
    Ok(has_ext_magic(ctx.head)
        .then(|| ext4_view::Ext4::size_from_superblock(&ctx.head[1024..]))
        .flatten()
        .map(|length| (PartitionKind::Ext4, length)))
}

fn apfs_size(ctx: &DetectCtx<'_>) -> Result<Option<u64>> {
    let Some(superblock) = apfs_superblock(ctx)? else {
        return Ok(None);
    };
    if !(1..superblock.block_count).contains(&superblock.omap_oid) {
        return Ok(None);
    }
    // A checkpoint NXSB has the same magic and geometry as block zero. Its
    // physical object map must still resolve relative to the proposed start.
    let block_size = u64::from(superblock.block_size);
    let omap = read_exact(
        ctx.source,
        superblock.omap_oid * block_size,
        block_size as usize,
    )?;
    let valid = ObjectHeader::parse(&omap).is_ok_and(|header| {
        header.is_physical()
            && header.object_type() == OBJECT_TYPE_OMAP
            && header.oid == superblock.omap_oid
    }) && apfs::fletcher::verify_object(&omap);
    Ok(valid.then_some(superblock.block_count * block_size))
}

fn ntfs_size(ctx: &DetectCtx<'_>) -> Result<Option<u64>> {
    if !has_ntfs_oem(ctx.head) {
        return Ok(None);
    }
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
    let record_size = ntfs.file_record_size() as usize;
    if length > ctx.size
        || record_size > MAX_MFT_RECORD_BYTES
        || mft
            .checked_add(record_size as u64)
            .is_none_or(|end| end > ntfs.size())
    {
        return Ok(None);
    }
    let mut record = read_exact(ctx.source, mft, record_size)?;
    let Ok(header) = MftRecordHeader::parse(&record) else {
        return Ok(None);
    };
    if header.is_corrupt()
        || !header.is_in_use()
        || header.base_record != 0
        || header.record_number != 0
        || header.allocated_size as usize != record_size
        || apply_fixup(&mut record, usize::from(ntfs.sector_size())).is_err()
    {
        return Ok(None);
    }
    let Some(used) = record.get(..header.used_size as usize) else {
        return Ok(None);
    };
    let Ok(attributes) = parse_attributes(used, usize::from(header.first_attribute_offset)) else {
        return Ok(None);
    };
    let named_mft = attributes
        .iter()
        .filter(|attribute| attribute.type_code == 0x30)
        .filter_map(|attribute| attribute.resident_content(used))
        .any(|value| FileName::parse(value).is_ok_and(|name| name.name == "$MFT"));
    Ok(named_mft.then_some(length))
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
    const APFS: &[u8] = include_bytes!("../../../../vendor/apfs/tests/data/basic.apfs");

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

    #[test]
    fn finds_ext_under_stale_boot_signatures() {
        let mut bytes = vec![0; MIB + EXT.len()];
        let volume = &mut bytes[MIB..];
        volume.copy_from_slice(EXT);
        volume[3..11].copy_from_slice(b"NTFS    ");
        volume[32..36].copy_from_slice(b"NXSB");
        volume[36..40].copy_from_slice(&4096u32.to_le_bytes());

        let recovered = scan(&BufSource(bytes), 512, &CancellationToken::new()).unwrap();
        assert_eq!(
            recovered
                .iter()
                .map(|part| (part.kind, part.start_lba, part.length_lba))
                .collect::<Vec<_>>(),
            [(PartitionKind::Ext4, 2048, 128)]
        );
    }

    #[test]
    fn rejects_apfs_checkpoint_as_a_shifted_container_start() {
        // Block zero is wiped, but the checkpoint, its real OMAP, and padding
        // after the container are still present. The shifted OMAP is invalid.
        let mut bytes = vec![0; MIB + APFS.len() + 4096];
        bytes[MIB + 4096..MIB + APFS.len()].copy_from_slice(&APFS[4096..]);
        assert!(scan(&BufSource(bytes), 512, &CancellationToken::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_ntfs_when_the_mft_record_is_only_a_signature() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("vendor/vhd/tests/data/ntfs_fixed.vhd");
        let fixture = std::fs::read(path).unwrap();
        let mut bytes = vec![0; MIB + 6016 * 512];
        bytes[MIB..MIB + 512].copy_from_slice(&fixture[128 * 512..129 * 512]);
        let mft_offset = MIB + 250 * 4096;
        bytes[mft_offset..mft_offset + 4].copy_from_slice(b"FILE");
        assert!(scan(&BufSource(bytes), 512, &CancellationToken::new())
            .unwrap()
            .is_empty());
    }
}
