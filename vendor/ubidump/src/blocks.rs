//! UBI PEB scan and volume-table selection, adapted from rust_ubidump's
//! `UbiBlocks`. The source is bounded and all persistent metadata is validated.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};

use crate::ubifs::common::VTBL_VOLID;
use crate::ubifs::ec_header::UbiEcHeader;
use crate::ubifs::vid_header::UbiVidHead;
use crate::ubifs::vtbl_record::UbiVtblRecord;
use crate::utils::crc::crc32_jamcrc;
use crate::volume::{Block, ReadAt, UbiVolume, read_exact};

pub struct UbiImage {
    pub peb_size: u32,
    pub volumes: BTreeMap<u32, Arc<UbiVolume>>,
}

impl UbiImage {
    pub fn open(source: &dyn ReadAt) -> Result<Self> {
        let first = UbiEcHeader::parse(&read_exact(source, 0, 64)?)?;
        ensure!(
            first.version == 1,
            "unsupported UBI version {}",
            first.version
        );
        let mut peb_size = None;
        for exponent in 12..=24 {
            let size = 1u32 << exponent;
            if u64::from(size) + 64 > source.size() || first.data_ofs >= size {
                continue;
            }
            let bytes = read_exact(source, u64::from(size), 64)?;
            if let Ok(next) = UbiEcHeader::parse(&bytes)
                && next.vid_hdr_ofs == first.vid_hdr_ofs
                && next.data_ofs == first.data_ofs
            {
                peb_size = Some(size);
                break;
            }
        }
        let peb_size = peb_size.context("cannot determine UBI eraseblock size (4 KiB..16 MiB)")?;
        ensure!(
            source.size().is_multiple_of(u64::from(peb_size)),
            "truncated UBI eraseblock"
        );
        ensure!(
            first.vid_hdr_ofs >= 64
                && first
                    .vid_hdr_ofs
                    .checked_add(64)
                    .is_some_and(|end| end <= first.data_ofs),
            "invalid UBI header offsets"
        );
        let payload_size = peb_size - first.data_ofs;
        let mut image_sequence = None;
        let mut maps: BTreeMap<u32, BTreeMap<u32, (Block, UbiVidHead)>> = BTreeMap::new();
        for physical in 0..source.size() / u64::from(peb_size) {
            let base = physical * u64::from(peb_size);
            let bytes = read_exact(source, base, 64)?;
            if bytes.iter().all(|byte| *byte == 0xff) {
                // An erased header alone does not prove the rest is erased.
                ensure!(
                    read_exact(source, base, peb_size as usize)?
                        .iter()
                        .all(|byte| *byte == 0xff),
                    "UBI PEB {physical} has data without an EC header"
                );
                continue;
            }
            let ec =
                UbiEcHeader::parse(&bytes).with_context(|| format!("UBI EC at PEB {physical}"))?;
            ensure!(
                ec.version == 1
                    && ec.vid_hdr_ofs == first.vid_hdr_ofs
                    && ec.data_ofs == first.data_ofs,
                "inconsistent UBI geometry at PEB {physical}"
            );
            if ec.image_seq != 0 {
                if let Some(sequence) = image_sequence {
                    ensure!(ec.image_seq == sequence, "mixed UBI image sequences");
                } else {
                    image_sequence = Some(ec.image_seq);
                }
            }
            let bytes = read_exact(source, base + u64::from(ec.vid_hdr_ofs), 64)?;
            if bytes.iter().all(|byte| *byte == 0xff) {
                continue;
            }
            let vid =
                UbiVidHead::parse(&bytes).with_context(|| format!("UBI VID at PEB {physical}"))?;
            ensure!(
                vid.version == 1 && matches!(vid.vol_type, 1 | 2) && vid.copy_flag <= 1,
                "unsupported UBI VID attributes"
            );
            ensure!(
                vid.vol_id < 128 || vid.vol_id == VTBL_VOLID,
                "unsupported UBI internal volume {}",
                vid.vol_id
            );
            ensure!(
                vid.data_pad < payload_size && vid.data_size <= payload_size - vid.data_pad,
                "invalid UBI LEB data bounds"
            );
            if vid.vol_type == 2 || vid.copy_flag == 1 {
                let data = read_exact(
                    source,
                    base + u64::from(ec.data_ofs),
                    vid.data_size as usize,
                )?;
                ensure!(
                    crc32_jamcrc(&data) == vid.data_crc,
                    "UBI data CRC mismatch at PEB {physical}"
                );
            }
            let block = Block {
                offset: base + u64::from(ec.data_ofs),
                sequence: vid.sqnum,
                data_size: vid.data_size,
                used_ebs: vid.used_ebs,
            };
            let map = maps.entry(vid.vol_id).or_default();
            if let Some((old, _)) = map.get(&vid.lnum) {
                ensure!(
                    old.sequence != block.sequence,
                    "ambiguous UBI duplicate LEB {}:{}",
                    vid.vol_id,
                    vid.lnum
                );
                if old.sequence > block.sequence {
                    continue;
                }
            }
            map.insert(vid.lnum, (block, vid));
        }
        let layout = maps
            .remove(&VTBL_VOLID)
            .context("UBI layout volume is missing")?;
        let mut tables = Vec::new();
        for (number, (block, header)) in layout {
            ensure!(
                number <= 1 && header.vol_type == 1,
                "unsupported UBI layout volume"
            );
            let count = (payload_size as usize / UbiVtblRecord::RECORD_SIZE).min(128);
            let bytes = read_exact(source, block.offset, count * UbiVtblRecord::RECORD_SIZE)?;
            let records = bytes
                .as_chunks::<{ UbiVtblRecord::RECORD_SIZE }>()
                .0
                .iter()
                .map(|record| UbiVtblRecord::parse(record))
                .collect::<std::result::Result<Vec<_>, _>>();
            if let Ok(records) = records {
                tables.push((bytes, records));
            }
        }
        let (table_bytes, records) = tables
            .first()
            .context("both UBI volume tables are invalid")?;
        ensure!(
            tables.iter().all(|(bytes, _)| bytes == table_bytes),
            "UBI volume tables disagree; interrupted update is unsupported"
        );
        let mut volumes = BTreeMap::new();
        let mut names = BTreeSet::new();
        for (id, record) in records.iter().enumerate() {
            if record.is_empty() {
                continue;
            }
            let id = id as u32;
            ensure!(
                record.reserved_pebs > 0
                    && record.alignment > 0
                    && record.alignment <= payload_size,
                "invalid UBI volume {id} reservation/alignment"
            );
            ensure!(
                record.data_pad == payload_size % record.alignment
                    && matches!(record.vol_type, 1 | 2),
                "invalid UBI volume {id} type/padding"
            );
            ensure!(
                record.upd_marker == 0,
                "UBI volume {id} has an incomplete update"
            );
            ensure!(record.flags <= 1, "unsupported UBI volume flags");
            let name =
                String::from_utf8(record.name.clone()).context("UBI volume name is not UTF-8")?;
            ensure!(
                !name.is_empty() && !name.contains(['\0', '/', '\\']),
                "invalid UBI volume name"
            );
            ensure!(names.insert(name.clone()), "duplicate UBI volume name");
            let leb_size = payload_size - record.data_pad;
            let mut blocks = BTreeMap::new();
            for (number, (block, header)) in maps.remove(&id).unwrap_or_default() {
                ensure!(
                    number < record.reserved_pebs
                        && header.vol_type == record.vol_type
                        && header.data_pad == record.data_pad,
                    "UBI volume {id} LEB disagrees with volume table"
                );
                blocks.insert(number, block);
            }
            let static_volume = record.vol_type == 2;
            let length = if static_volume {
                let first_block = blocks
                    .get(&0)
                    .context("UBI static volume is missing LEB 0")?;
                let count = first_block.used_ebs;
                ensure!(
                    count > 0
                        && count <= record.reserved_pebs
                        && blocks.len() as u64 == u64::from(count),
                    "UBI static volume has missing LEBs"
                );
                let mut length = 0u64;
                for number in 0..count {
                    let block = blocks.get(&number).context("UBI static volume has a gap")?;
                    ensure!(
                        block.used_ebs == count
                            && block.data_size > 0
                            && (number == count - 1 || block.data_size == leb_size),
                        "UBI static LEB length is inconsistent"
                    );
                    length += u64::from(block.data_size);
                }
                length
            } else {
                u64::from(record.reserved_pebs) * u64::from(leb_size)
            };
            volumes.insert(
                id,
                Arc::new(UbiVolume {
                    id,
                    name,
                    length,
                    leb_size,
                    static_volume,
                    blocks,
                }),
            );
        }
        if !maps.is_empty() {
            bail!("UBI blocks reference volumes absent from the volume table");
        }
        Ok(Self { peb_size, volumes })
    }
}
