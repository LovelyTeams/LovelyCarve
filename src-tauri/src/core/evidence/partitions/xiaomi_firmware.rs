//! Xiaomi `HDR1` firmware-container recognition.
//!
//! The container header stores up to 32 absolute segment-header offsets.
//! Each 48-byte segment header then names a byte payload and declares its
//! length. Payloads are not required to start on a 512-byte boundary, so the
//! enclosing [`DiskImage`](crate::core::evidence::image::DiskImage) uses one-byte
//! addressing for this source format. Only segments that directly contain a
//! supported filesystem are surfaced as partitions; metadata, signatures,
//! boot loaders, and other opaque blobs remain available through the raw
//! image view without masquerading as expandable volumes.

use std::collections::HashSet;

use anyhow::{bail, Context};

use crate::core::evidence::partitions::volume_probe::probe_volume;
use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionSlice, TableScheme,
};

pub const XIAOMI_FIRMWARE_MAGIC: &[u8; 4] = b"HDR1";

const CONTAINER_HEADER_LEN: usize = 48;
const SIGNATURE_OFFSET_OFFSET: usize = 4;
const SEGMENT_OFFSETS_OFFSET: usize = 16;
const LEGACY_SEGMENT_OFFSET_COUNT: usize = 8;
const SEGMENT_OFFSET_COUNT: usize = 32;
const SEGMENT_HEADER_LEN: usize = 48;
const SEGMENT_MAGIC: [u8; 4] = [0xBE, 0xBA, 0x00, 0x00];
const SEGMENT_LENGTH_OFFSET: usize = 8;
const SEGMENT_NAME_OFFSET: usize = 16;
const SEGMENT_NAME_LEN: usize = 32;
const EMBEDDED_VOLUME_SECTOR_SIZE: usize = 512;

pub struct XiaomiFirmwareParser;

impl PartParser for XiaomiFirmwareParser {
    fn name(&self) -> &'static str {
        "xiaomi_firmware"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Firmware
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        if ctx.head.get(..XIAOMI_FIRMWARE_MAGIC.len()) != Some(XIAOMI_FIRMWARE_MAGIC) {
            return Ok(None);
        }
        if ctx.head.len() < CONTAINER_HEADER_LEN {
            bail!("Xiaomi HDR1 header is truncated");
        }
        if ctx.sector_size != 1 {
            bail!("Xiaomi HDR1 container requires byte-addressed source geometry");
        }

        let signature_offset = u64::from(read_u32(ctx.head, SIGNATURE_OFFSET_OFFSET)?);
        if signature_offset < CONTAINER_HEADER_LEN as u64 || signature_offset > ctx.size {
            bail!(
                "Xiaomi HDR1 signature offset 0x{signature_offset:x} is outside the {:#x}-byte source",
                ctx.size
            );
        }

        // Older HDR1 headers have only eight slots; extended headers end at
        // the first segment (usually 0x90). Never interpret payload as slots.
        let mut table_end = SEGMENT_OFFSETS_OFFSET + SEGMENT_OFFSET_COUNT * 4;
        for index in 0..LEGACY_SEGMENT_OFFSET_COUNT {
            let offset = read_u32(ctx.head, SEGMENT_OFFSETS_OFFSET + index * 4)? as usize;
            if offset != 0 {
                table_end = table_end.min(offset);
            }
        }
        if table_end < CONTAINER_HEADER_LEN || table_end > ctx.head.len() {
            bail!("Xiaomi HDR1 segment table is truncated or overlaps its header");
        }
        let slot_count = (table_end - SEGMENT_OFFSETS_OFFSET) / 4;
        let mut seen_offsets = HashSet::new();
        let mut segments = Vec::new();
        let mut partitions = Vec::new();
        for index in 0..slot_count {
            let field_offset = SEGMENT_OFFSETS_OFFSET + index * 4;
            let header_offset = u64::from(read_u32(ctx.head, field_offset)?);
            if header_offset == 0 {
                continue;
            }
            if !seen_offsets.insert(header_offset) {
                bail!("Xiaomi HDR1 segment header offset 0x{header_offset:x} is duplicated");
            }
            let payload_offset = header_offset
                .checked_add(SEGMENT_HEADER_LEN as u64)
                .context("Xiaomi HDR1 segment payload offset overflow")?;
            if header_offset < table_end as u64 || payload_offset > signature_offset {
                bail!(
                    "Xiaomi HDR1 segment {index} header at 0x{header_offset:x} is outside the payload area"
                );
            }

            let header = ctx
                .source
                .read(header_offset, SEGMENT_HEADER_LEN)
                .with_context(|| {
                    format!("reading Xiaomi HDR1 segment {index} header at 0x{header_offset:x}")
                })?;
            if header.len() != SEGMENT_HEADER_LEN {
                bail!("Xiaomi HDR1 segment {index} header is truncated");
            }
            if header.get(..SEGMENT_MAGIC.len()) != Some(&SEGMENT_MAGIC) {
                bail!("Xiaomi HDR1 segment {index} has invalid BABE magic");
            }

            let payload_len = u64::from(read_u32(&header, SEGMENT_LENGTH_OFFSET)?);
            let payload_end = payload_offset
                .checked_add(payload_len)
                .context("Xiaomi HDR1 segment payload length overflow")?;
            if payload_end > signature_offset {
                bail!(
                    "Xiaomi HDR1 segment {index} payload ends at 0x{payload_end:x}, beyond signature offset 0x{signature_offset:x}"
                );
            }
            if segments
                .iter()
                .any(|&(start, end)| header_offset < end && payload_end > start)
            {
                bail!("Xiaomi HDR1 segment {index} overlaps another segment");
            }
            segments.push((header_offset, payload_end));
            if payload_len == 0 {
                continue;
            }

            let slice = PartitionSlice::new(ctx.source, payload_offset, payload_len);
            let Some(hit) = probe_volume(&slice, EMBEDDED_VOLUME_SECTOR_SIZE)? else {
                continue;
            };
            let name = segment_name(&header);
            let id = u8::try_from(partitions.len()).context("too many Xiaomi HDR1 volumes")?;
            partitions.push(Partition {
                id,
                kind: hit.kind,
                start_lba: payload_offset,
                length_lba: payload_len,
                label: Some(if name.is_empty() {
                    format!("固件卷{}", id + 1)
                } else {
                    name
                }),
                unique_guid: None,
            });
        }

        Ok(Some(partitions))
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .with_context(|| format!("Xiaomi HDR1 field at 0x{offset:x} is truncated"))?;
    Ok(u32::from_le_bytes(
        value
            .try_into()
            .context("Xiaomi HDR1 field has invalid width")?,
    ))
}

fn segment_name(header: &[u8]) -> String {
    let bytes = &header[SEGMENT_NAME_OFFSET..SEGMENT_NAME_OFFSET + SEGMENT_NAME_LEN];
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{
        detect_partitions_with_meta, test_support::BufSource, PartitionKind,
    };

    #[test]
    fn detects_byte_aligned_fat12_and_squashfs_segments() {
        let source = BufSource(firmware_fixture());

        let detected = detect_partitions_with_meta(&source, 1).expect("detect HDR1 firmware");

        assert_eq!(detected.scheme, Some(TableScheme::Firmware));
        assert_eq!(detected.disk_id, None);
        assert_eq!(detected.partitions.len(), 2);
        assert_eq!(
            detected.partitions[0],
            Partition {
                id: 0,
                kind: PartitionKind::Fat12,
                start_lba: 176,
                length_lba: 8192,
                label: Some("kernel.img".into()),
                unique_guid: None,
            }
        );
        assert_eq!(
            detected.partitions[1],
            Partition {
                id: 1,
                kind: PartitionKind::Squashfs,
                start_lba: 8432,
                length_lba: 4096,
                label: Some("rootfs.img".into()),
                unique_guid: None,
            }
        );
    }

    #[test]
    fn rejects_a_segment_that_runs_into_the_signature() {
        let mut bytes = firmware_fixture();
        bytes[128 + SEGMENT_LENGTH_OFFSET..128 + SEGMENT_LENGTH_OFFSET + 4]
            .copy_from_slice(&20_000u32.to_le_bytes());
        let source = BufSource(bytes);

        let error = detect_partitions_with_meta(&source, 1).expect_err("invalid segment must fail");

        assert!(error.to_string().contains("beyond signature offset"));
    }

    #[test]
    fn detects_ubi_after_the_first_eight_slots_and_in_the_last_slot() {
        for slot in [8, 31] {
            let mut payload = vec![0xff; 8192];
            let ec = &mut payload[..64];
            ec.fill(0);
            ec[..4].copy_from_slice(b"UBI#");
            ec[4] = 1;
            ec[16..20].copy_from_slice(&2048u32.to_be_bytes());
            ec[20..24].copy_from_slice(&4096u32.to_be_bytes());
            let crc = ubidump::utils::crc::crc32_jamcrc(&ec[..60]);
            ec[60..64].copy_from_slice(&crc.to_be_bytes());
            let signature = 256 + 48 + payload.len();
            let mut bytes = vec![0; signature + 32];
            bytes[..4].copy_from_slice(XIAOMI_FIRMWARE_MAGIC);
            bytes[4..8].copy_from_slice(&(signature as u32).to_le_bytes());
            bytes[16..20].copy_from_slice(&144u32.to_le_bytes());
            bytes[16 + slot * 4..20 + slot * 4].copy_from_slice(&256u32.to_le_bytes());
            write_segment(&mut bytes, 144, "version", b"v1");
            write_segment(&mut bytes, 256, "root.ubi", &payload);
            let result = detect_partitions_with_meta(&BufSource(bytes), 1).unwrap();
            assert_eq!(result.partitions.len(), 1);
            assert_eq!(result.partitions[0].kind, PartitionKind::Ubi);
            assert_eq!(result.partitions[0].start_lba, 304);
            assert_eq!(result.partitions[0].label.as_deref(), Some("root.ubi"));
        }
    }

    #[test]
    fn rejects_overlapping_segments_even_when_one_is_opaque() {
        let mut bytes = firmware_fixture();
        // Extend the opaque version segment into the FAT segment header.
        bytes[56 + SEGMENT_LENGTH_OFFSET..60 + SEGMENT_LENGTH_OFFSET]
            .copy_from_slice(&100u32.to_le_bytes());
        let error = detect_partitions_with_meta(&BufSource(bytes), 1).unwrap_err();
        assert!(error.to_string().contains("overlaps"));
    }

    fn firmware_fixture() -> Vec<u8> {
        const VERSION_HEADER: usize = 56;
        const FAT_HEADER: usize = 128;
        const SQUASHFS_HEADER: usize = 8384;
        const SIGNATURE_OFFSET: usize = 12_544;

        let mut bytes = vec![0u8; SIGNATURE_OFFSET + 256];
        bytes[..4].copy_from_slice(XIAOMI_FIRMWARE_MAGIC);
        bytes[4..8].copy_from_slice(&(SIGNATURE_OFFSET as u32).to_le_bytes());
        for (index, offset) in [VERSION_HEADER, FAT_HEADER, SQUASHFS_HEADER]
            .into_iter()
            .enumerate()
        {
            let field = SEGMENT_OFFSETS_OFFSET + index * 4;
            bytes[field..field + 4].copy_from_slice(&(offset as u32).to_le_bytes());
        }

        write_segment(&mut bytes, VERSION_HEADER, "version", b"version");

        let mut fat = vec![0u8; 8192];
        fat[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
        fat[3..11].copy_from_slice(b"mkfs.fat");
        fat[11..13].copy_from_slice(&512u16.to_le_bytes());
        fat[13] = 1;
        fat[14..16].copy_from_slice(&1u16.to_le_bytes());
        fat[16] = 2;
        fat[17..19].copy_from_slice(&16u16.to_le_bytes());
        fat[19..21].copy_from_slice(&16u16.to_le_bytes());
        fat[21] = 0xF8;
        fat[22..24].copy_from_slice(&1u16.to_le_bytes());
        fat[54..62].copy_from_slice(b"FAT12   ");
        fat[510..512].copy_from_slice(&[0x55, 0xAA]);
        write_segment(&mut bytes, FAT_HEADER, "kernel.img", &fat);

        let mut squashfs = vec![0u8; 4096];
        squashfs[..4].copy_from_slice(b"hsqs");
        write_segment(&mut bytes, SQUASHFS_HEADER, "rootfs.img", &squashfs);
        bytes
    }

    fn write_segment(bytes: &mut [u8], header_offset: usize, name: &str, payload: &[u8]) {
        bytes[header_offset..header_offset + 4].copy_from_slice(&SEGMENT_MAGIC);
        bytes[header_offset + 4..header_offset + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[header_offset + 8..header_offset + 12]
            .copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes[header_offset + 12..header_offset + 16]
            .copy_from_slice(&u32::from(u16::MAX).to_le_bytes());
        let name_end = header_offset + SEGMENT_NAME_OFFSET + name.len();
        bytes[header_offset + SEGMENT_NAME_OFFSET..name_end].copy_from_slice(name.as_bytes());
        let payload_offset = header_offset + SEGMENT_HEADER_LEN;
        bytes[payload_offset..payload_offset + payload.len()].copy_from_slice(payload);
    }
}
