//! Apple Partition Map detection.
//!
//! APM is registered in the parser table because its
//! `ER` driver-descriptor signature at offset 0 cannot collide with the
//! other parsers' magics, and the resulting Apple partition map describes
//! a real partitioning scheme worth surfacing in the same flow.

use std::io::Cursor;

use binrw::BinRead;

use crate::core::evidence::partitions::{
    DetectCtx, PartParser, Partition, PartitionKind, TableScheme,
};

const APM_ENTRY_MAGIC: &[u8; 2] = b"PM";
const APM_ENTRY_MAGIC_ALT: &[u8; 2] = b"TS";
const MAX_APM_ENTRIES: u32 = 1024;

pub struct ApmParser;

#[derive(Debug, BinRead)]
#[br(big, magic = b"ER")]
struct ApmDriverDescriptor {
    block_size: u16,
}

#[derive(Debug, BinRead)]
#[br(big)]
struct ApmEntry {
    signature: [u8; 2],
    _reserved: u16,
    entry_count: u32,
    start_block: u32,
    block_count: u32,
    name: [u8; 32],
    type_name: [u8; 32],
    _data_start_block: u32,
}

impl ApmEntry {
    fn has_valid_signature(&self) -> bool {
        self.signature == *APM_ENTRY_MAGIC || self.signature == *APM_ENTRY_MAGIC_ALT
    }
}

impl PartParser for ApmParser {
    fn name(&self) -> &'static str {
        "apm"
    }

    fn scheme(&self) -> TableScheme {
        TableScheme::Apm
    }

    fn try_parse(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<Vec<Partition>>> {
        let block0 = ctx.source.read(0, ctx.sector_size)?;
        let Some(driver) = ApmDriverDescriptor::read(&mut Cursor::new(&block0)).ok() else {
            return Ok(None);
        };
        let block_size = usize::from(driver.block_size);
        if block_size == 0 || !block_size.is_multiple_of(512) {
            return Ok(None);
        }

        let first_entry = ctx.source.read(block_size as u64, block_size)?;
        let Some(first_entry) = ApmEntry::read(&mut Cursor::new(&first_entry)).ok() else {
            return Ok(None);
        };
        if !first_entry.has_valid_signature() {
            return Ok(None);
        }
        let entry_count = first_entry.entry_count;
        if entry_count == 0 || entry_count > MAX_APM_ENTRIES {
            return Ok(None);
        }

        let scale = (block_size / ctx.sector_size).max(1) as u64;
        let mut out = Vec::with_capacity(entry_count as usize);
        for index in 0..entry_count {
            let offset = (index as u64 + 1) * block_size as u64;
            let entry = ctx.source.read(offset, block_size)?;
            let Some(entry) = ApmEntry::read(&mut Cursor::new(&entry)).ok() else {
                break;
            };
            if !entry.has_valid_signature() {
                continue;
            }
            let start_block = u64::from(entry.start_block);
            let block_count = u64::from(entry.block_count);
            if block_count == 0 {
                continue;
            }
            let name = read_apm_string(&entry.name);
            let type_name = read_apm_string(&entry.type_name);
            out.push(Partition {
                id: index as u8,
                kind: apm_type_to_kind(&type_name),
                start_lba: start_block * scale,
                length_lba: block_count * scale,
                label: if name.is_empty() {
                    Some(type_name)
                } else {
                    Some(format!("{name} ({type_name})"))
                },
                unique_guid: None,
                amiga_geometry: None,
            });
        }
        Ok(Some(out))
    }
}

fn read_apm_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn apm_type_to_kind(type_name: &str) -> PartitionKind {
    match type_name {
        "Apple_HFS" | "Apple_HFSX" | "Apple_MFS" | "Apple_APFS" => PartitionKind::ApfsContainer,
        "Apple_UFS" | "Linux" => PartitionKind::Unknown,
        _ => PartitionKind::Apm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{detect_partitions, test_support::BufSource};

    #[test]
    fn detects_apm_partitions() {
        let sector = 512usize;
        let block = 512usize;
        let mut buf = vec![0u8; block * 8];
        buf[0..2].copy_from_slice(b"ER");
        buf[2..4].copy_from_slice(&(block as u16).to_be_bytes());
        buf[block..block + 2].copy_from_slice(APM_ENTRY_MAGIC);
        buf[block + 4..block + 8].copy_from_slice(&2u32.to_be_bytes());
        buf[block + 8..block + 12].copy_from_slice(&1u32.to_be_bytes());
        buf[block + 12..block + 16].copy_from_slice(&3u32.to_be_bytes());
        buf[block + 16..block + 20].copy_from_slice(b"Map\0");
        buf[block + 48..block + 66].copy_from_slice(b"Apple_partition_ma");
        buf[2 * block..2 * block + 2].copy_from_slice(APM_ENTRY_MAGIC);
        buf[2 * block + 4..2 * block + 8].copy_from_slice(&2u32.to_be_bytes());
        buf[2 * block + 8..2 * block + 12].copy_from_slice(&4u32.to_be_bytes());
        buf[2 * block + 12..2 * block + 16].copy_from_slice(&4u32.to_be_bytes());
        buf[2 * block + 16..2 * block + 20].copy_from_slice(b"HFS\0");
        buf[2 * block + 48..2 * block + 57].copy_from_slice(b"Apple_HFS");

        let src = BufSource(buf);
        let parts = detect_partitions(&src, sector).expect("detect");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].start_lba, 4);
        assert_eq!(parts[1].length_lba, 4);
        assert_eq!(parts[1].kind, PartitionKind::ApfsContainer);
    }
}
