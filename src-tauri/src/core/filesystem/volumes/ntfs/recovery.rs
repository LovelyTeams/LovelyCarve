//! Reconstruct a read-only NTFS reader from a surviving self-describing $MFT.
//! Geometry is inferred only when allocation size, run count, physical location
//! and neighboring system-record identities agree. The source boot is untouched.
use super::*;
use crate::core::evidence::partitions::PartitionKind;
use anyhow::{ensure, Context};
use ntfs_core::{apply_fixup, decode_runlist, parse_attributes, AttributeBody, MftRecordHeader};

#[derive(Debug, Clone, Serialize)]
pub struct RecoveredNtfsGeometry {
    pub mft_offset: u64,
    pub cluster_size: u32,
    pub record_size: u32,
    pub sector_size: u16,
}

pub(crate) fn discover_mft(
    image: &DiskImage,
    offset: u64,
) -> Result<(Partition, RecoveredNtfsGeometry)> {
    let head = image.read_range(offset, 48)?;
    let header = MftRecordHeader::parse(&head)?;
    ensure!(
        header.record_number == 0 && header.is_in_use() && header.base_record == 0,
        "not a base $MFT record"
    );
    let size = header.allocated_size as usize;
    ensure!(
        (1024..=4096).contains(&size) && size.is_power_of_two() && header.usa_count > 1,
        "invalid MFT record size"
    );
    let sectors = usize::from(header.usa_count - 1);
    ensure!(size.is_multiple_of(sectors), "invalid MFT sector count");
    let sector_size = size / sectors;
    ensure!(
        (512..=4096).contains(&sector_size) && sector_size.is_power_of_two(),
        "invalid MFT sector size"
    );
    let record = read_record(image, offset, size, sector_size, 0)?;
    let attrs = parse_attributes(
        &record[..header.used_size as usize],
        usize::from(header.first_attribute_offset),
    )?;
    ensure!(
        attrs
            .iter()
            .filter_map(|attr| attr
                .resident_content(&record)
                .filter(|_| attr.type_code == 0x30))
            .any(|value| ntfs_core::FileName::parse(value).is_ok_and(|name| name.name == "$MFT")),
        "record does not name $MFT"
    );
    ensure!(
        !attrs.iter().any(|attr| attr.type_code == 0x20),
        "fragmented MFT attribute-list geometry requires a surviving boot"
    );
    let data = attrs
        .iter()
        .find(|attr| attr.type_code == 0x80 && attr.name.is_none())
        .context("$MFT has no default data")?;
    ensure!(
        data.flags == 0,
        "encoded $MFT is unsupported for geometry recovery"
    );
    let AttributeBody::NonResident {
        start_vcn: 0,
        last_vcn,
        runs_offset,
        allocated_size,
        real_size,
        initialized_size,
        ..
    } = data.body
    else {
        anyhow::bail!("MFT data has no complete initial mapping");
    };
    let runs_start = data
        .offset
        .checked_add(usize::from(runs_offset))
        .context("MFT run offset overflow")?;
    let runs_end = data
        .offset
        .checked_add(data.length as usize)
        .context("MFT attribute end overflow")?;
    let runs = decode_runlist(
        record
            .get(runs_start..runs_end)
            .context("MFT mapping bounds")?,
    )?;
    ensure!(
        runs.iter().all(|run| run.lcn.is_some()),
        "sparse MFT geometry cannot be inferred"
    );
    let clusters = runs.iter().try_fold(0u64, |total, run| {
        total
            .checked_add(run.length)
            .context("MFT cluster overflow")
    })?;
    ensure!(
        clusters > 0
            && clusters == last_vcn.checked_add(1).context("MFT last VCN overflow")?
            && allocated_size.is_multiple_of(clusters),
        "inconsistent MFT allocation"
    );
    let cluster_size = allocated_size / clusters;
    ensure!(
        (sector_size as u64..=65536).contains(&cluster_size)
            && cluster_size.is_power_of_two()
            && real_size <= allocated_size
            && initialized_size <= real_size,
        "ambiguous MFT geometry"
    );
    let first = runs
        .first()
        .and_then(|run| run.lcn)
        .context("empty MFT runlist")?;
    let base = offset
        .checked_sub(
            first
                .checked_mul(cluster_size)
                .context("MFT LCN overflow")?,
        )
        .context("MFT mapping precedes source")?;
    ensure!(
        base.is_multiple_of(image.sector_size as u64),
        "MFT base is not source-sector aligned"
    );
    ensure!(
        runs[0]
            .length
            .checked_mul(cluster_size)
            .is_some_and(|length| length >= 6 * size as u64),
        "too little contiguous system metadata to validate geometry"
    );
    for run in &runs {
        let end = run
            .lcn
            .context("sparse MFT run")?
            .checked_add(run.length)
            .and_then(|end| end.checked_mul(cluster_size))
            .and_then(|end| base.checked_add(end))
            .context("MFT run bound overflow")?;
        ensure!(end <= image.len(), "MFT allocation exceeds evidence");
    }
    for (number, name) in [(1, "$MFTMirr"), (5, ".")] {
        let record_offset = offset
            .checked_add(number * size as u64)
            .context("MFT record offset overflow")?;
        let bytes = read_record(image, record_offset, size, sector_size, number as u32)?;
        let other = MftRecordHeader::parse(&bytes)?;
        let attrs = parse_attributes(
            &bytes[..other.used_size as usize],
            usize::from(other.first_attribute_offset),
        )?;
        ensure!(
            attrs
                .iter()
                .filter(|attr| attr.type_code == 0x30)
                .filter_map(|attr| attr.resident_content(&bytes))
                .any(
                    |value| ntfs_core::FileName::parse(value).is_ok_and(|entry| entry.name == name)
                ),
            "MFT neighboring system record mismatch"
        );
    }
    let volume_record = read_record(image, offset + 3 * size as u64, size, sector_size, 3)?;
    let volume_header = MftRecordHeader::parse(&volume_record)?;
    let label = parse_attributes(
        &volume_record[..volume_header.used_size as usize],
        usize::from(volume_header.first_attribute_offset),
    )?
    .into_iter()
    .find(|attr| attr.type_code == 0x60)
    .and_then(|attr| {
        attr.resident_content(&volume_record)
            .filter(|value| value.len().is_multiple_of(2))
            .map(|value| {
                String::from_utf16_lossy(
                    &value
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>(),
                )
            })
    });
    let partition = Partition {
        id: 0,
        kind: PartitionKind::Ntfs,
        start_lba: base / image.sector_size as u64,
        // The original volume end is unknown. Bound reads by the available source.
        length_lba: (image.len() - base) / image.sector_size as u64,
        label,
        unique_guid: None,
    };
    Ok((
        partition,
        RecoveredNtfsGeometry {
            mft_offset: offset,
            cluster_size: cluster_size as u32,
            record_size: size as u32,
            sector_size: sector_size as u16,
        },
    ))
}

fn read_record(
    image: &DiskImage,
    offset: u64,
    size: usize,
    sector: usize,
    number: u32,
) -> Result<Vec<u8>> {
    let mut bytes = image.read_range(offset, size)?;
    ensure!(bytes.len() == size, "short MFT record");
    let header = MftRecordHeader::parse(&bytes)?;
    ensure!(
        header.record_number == number
            && header.allocated_size as usize == size
            && header.used_size <= header.allocated_size
            && usize::from(header.first_attribute_offset) >= 48
            && !header.is_corrupt(),
        "invalid MFT record geometry or identity"
    );
    apply_fixup(&mut bytes, sector)?;
    Ok(bytes)
}

impl<'a> NtfsVolume<'a> {
    pub(crate) fn open_recovered(
        image: &'a DiskImage,
        partition: &Partition,
        geometry: &RecoveredNtfsGeometry,
    ) -> Result<Self> {
        let base = partition
            .start_lba
            .checked_mul(image.sector_size as u64)
            .context("Recovered NTFS base overflow")?;
        let length = partition
            .length_lba
            .checked_mul(image.sector_size as u64)
            .context("Recovered NTFS length overflow")?;
        ensure!(
            base.checked_add(length)
                .is_some_and(|end| end <= image.len()),
            "Recovered NTFS outside source"
        );
        let backing = PartitionBacking::Image {
            image,
            base_offset: base,
            length_bytes: length,
            layout: None,
        };
        let mut boot = [0; 512];
        boot[0..3].copy_from_slice(&[0xeb, 0x52, 0x90]);
        boot[3..11].copy_from_slice(b"NTFS    ");
        boot[11..13].copy_from_slice(&geometry.sector_size.to_le_bytes());
        boot[13] = u8::try_from(geometry.cluster_size / u32::from(geometry.sector_size))?;
        boot[40..48].copy_from_slice(&(length / u64::from(geometry.sector_size)).to_le_bytes());
        let mft_lcn = geometry
            .mft_offset
            .checked_sub(base)
            .context("MFT before recovered volume")?
            / u64::from(geometry.cluster_size);
        boot[48..56].copy_from_slice(&mft_lcn.to_le_bytes());
        boot[64] = (-(geometry.record_size.ilog2() as i8)) as u8;
        boot[68] = (-12i8) as u8;
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        let mut bootstrap = BootstrapReader {
            reader: backing.reader(),
            boot,
        };
        let mut ntfs = Ntfs::new(&mut bootstrap)?;
        // The inferred boot is used only to initialize geometry. File reads,
        // including $Boot, always return authentic source bytes afterwards.
        if let Err(error) = ntfs.read_upcase_table(&mut backing.reader()) {
            tracing::debug!("Recovered NTFS upcase: {error}");
        }
        Ok(Self {
            backing,
            source: NtfsSource::Partition(partition.id),
            ntfs,
        })
    }
}
struct BootstrapReader<'a> {
    reader: PartitionReader<'a>,
    boot: [u8; 512],
}
impl Read for BootstrapReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let position = self.reader.position;
        let count = self.reader.read(buffer)?;
        if position < 512 {
            let start = position as usize;
            let length = count.min(512 - start);
            buffer[..length].copy_from_slice(&self.boot[start..start + length]);
        }
        Ok(count)
    }
}
impl Seek for BootstrapReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader.seek(position)
    }
}
