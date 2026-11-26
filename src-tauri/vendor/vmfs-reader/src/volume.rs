//! Mapping a VMFS logical volume back to its individual source images.
use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom};

use crate::lvm::{Extent, Member};
use crate::{Error, Result, le32, le64, name, read_exact_at};

const FS_HEADER: u64 = 0x200000;
const VMFS_MAGIC: u32 = 0x2fabf15e;
const VMFSL_MAGIC: u32 = 0x2fabf15f;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Version {
    Vmfs5,
    Vmfs6,
}

#[derive(Debug, Clone)]
pub struct Geometry {
    pub version: Version,
    pub minor_version: u8,
    pub local: bool,
    pub label: String,
    pub uuid: [u8; 16],
    pub file_block_size: u64,
    pub sub_block_size: u64,
    pub metadata_alignment: u64,
    pub config: u32,
    pub descriptor_group_offset: u64,
    pub descriptor_clusters_per_group: u64,
    pub second_pointer_descriptor: (u32, u64),
    pub system_directory_descriptor: (u32, u64),
    pub large_block_shift: u16,
}

impl Geometry {
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let mut bytes = [0; 512];
        read_exact_at(reader, FS_HEADER, &mut bytes)?;
        Self::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8; 512]) -> Result<Self> {
        let magic = le32(bytes, 0);
        if !matches!(magic, VMFS_MAGIC | VMFSL_MAGIC) {
            return Err(Error::Signature);
        }
        let version = match le32(bytes, 4) {
            14 => Version::Vmfs5,
            24 => Version::Vmfs6,
            _ => return Err(Error::Unsupported("filesystem descriptor version")),
        };
        let metadata_alignment = match version {
            Version::Vmfs5 => 512,
            Version::Vmfs6 => u64::from(le32(bytes, 308)),
        };
        let file_block_size = le64(bytes, 161);
        let sub_block_size = u64::from(le32(bytes, 217));
        let minimum_alignment = if version == Version::Vmfs5 { 512 } else { 4096 };
        if !(minimum_alignment..=65536).contains(&metadata_alignment)
            || !metadata_alignment.is_power_of_two()
            || !file_block_size.is_power_of_two()
            || !(65536..=64 * 1024 * 1024).contains(&file_block_size)
            || !(512..=file_block_size).contains(&sub_block_size)
            || !sub_block_size.is_power_of_two()
        {
            return Err(Error::Corrupt("invalid filesystem descriptor geometry"));
        }
        let config = le32(bytes, 25);
        Ok(Self {
            version,
            minor_version: bytes[8],
            local: magic == VMFSL_MAGIC || config & 0x100 != 0,
            label: name(&bytes[29..157]),
            uuid: bytes[9..25].try_into().unwrap(),
            file_block_size,
            sub_block_size,
            metadata_alignment,
            config,
            descriptor_group_offset: u64::from(le32(bytes, 209)),
            descriptor_clusters_per_group: u64::from(le32(bytes, 213)),
            second_pointer_descriptor: (le32(bytes, 233), le64(bytes, 225)),
            system_directory_descriptor: (le32(bytes, 269), le64(bytes, 261)),
            large_block_shift: u16::from_le_bytes([bytes[312], bytes[313]]),
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SourceSpan {
    pub member: usize,
    pub offset: u64,
    pub length: u64,
}

/// Seekable VMFS address space. A member index refers to the input order.
pub struct Volume<R> {
    sources: Vec<R>,
    extents: Vec<Extent>,
    position: u64,
    size: u64,
    geometry: Geometry,
}

impl<R: Read + Seek> Volume<R> {
    pub fn open(source: R) -> Result<Self> {
        Self::open_members(vec![source])
    }

    pub fn open_members(mut sources: Vec<R>) -> Result<Self> {
        if sources.is_empty() || sources.len() > 32 {
            return Err(Error::Unsupported("VMFS source count"));
        }
        let mut members = Vec::with_capacity(sources.len());
        for (index, source) in sources.iter_mut().enumerate() {
            if let Some(member) = Member::read(source)? {
                members.push((index, member));
            }
        }
        let (extents, size, expected_version) = if let Some((_, first)) = members.first() {
            let identity = &first.info;
            if members.len() != sources.len() || members.len() != identity.member_count as usize {
                return Err(Error::MissingMembers);
            }
            let mut seen = HashSet::new();
            if members.iter().any(|(_, member)| {
                let info = &member.info;
                info.volume_uuid != identity.volume_uuid
                    || info.snapshot_id != identity.snapshot_id
                    || info.volume_size != identity.volume_size
                    || info.volume_generation != identity.volume_generation
                    || info.member_count != identity.member_count
                    || info.version != identity.version
                    || !seen.insert(info.member_uuid)
            }) {
                return Err(Error::Corrupt(
                    "incompatible, stale or duplicate extent members",
                ));
            }
            let size = identity.volume_size;
            let version = if identity.version == 5 {
                Version::Vmfs5
            } else {
                Version::Vmfs6
            };
            members.sort_by_key(|(_, member)| member.info.first_extent);
            let mut expected_first = 0;
            let mut extents = Vec::new();
            for (index, member) in members {
                if member.info.first_extent != expected_first {
                    return Err(Error::Corrupt("noncontiguous member extent indices"));
                }
                expected_first = member
                    .info
                    .last_extent
                    .checked_add(1)
                    .ok_or(Error::Corrupt("member extent index overflow"))?;
                extents.extend(member.extents(&mut sources[index], index)?);
            }
            (coalesce(extents, size)?, size, Some(version))
        } else {
            if sources.len() != 1 {
                return Err(Error::Signature);
            }
            let size = sources[0].seek(SeekFrom::End(0))?;
            (
                vec![Extent {
                    logical: 0,
                    physical: 0,
                    length: size,
                    source: 0,
                }],
                size,
                None,
            )
        };
        let header = read_header(&mut sources, &extents, size)?;
        let geometry = Geometry::from_bytes(&header)?;
        if expected_version.is_some_and(|version| version != geometry.version) {
            return Err(Error::Corrupt(
                "filesystem and extent member versions differ",
            ));
        }
        Ok(Self {
            sources,
            extents,
            position: 0,
            size,
            geometry,
        })
    }

    pub fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn source_spans(&self, offset: u64, length: u64) -> Result<Vec<SourceSpan>> {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= self.size)
            .ok_or(Error::Corrupt("logical byte range exceeds volume"))?;
        let mut cursor = offset;
        let mut spans = Vec::new();
        while cursor < end {
            let span = span_at(&self.extents, cursor, end - cursor)
                .ok_or(Error::Corrupt("logical byte range has no source extent"))?;
            cursor += span.length;
            spans.push(span);
        }
        Ok(spans)
    }
}

fn read_header<R: Read + Seek>(
    sources: &mut [R],
    extents: &[Extent],
    size: u64,
) -> Result<[u8; 512]> {
    let mut bytes = [0; 512];
    if size < FS_HEADER + bytes.len() as u64 {
        return Err(Error::Signature);
    }
    read_volume(sources, extents, FS_HEADER, &mut bytes)?;
    Ok(bytes)
}

fn coalesce(mut extents: Vec<Extent>, size: u64) -> Result<Vec<Extent>> {
    extents.sort_by_key(|extent| extent.logical);
    let mut merged: Vec<Extent> = Vec::new();
    let mut end = 0u64;
    for extent in extents {
        if extent.logical != end {
            return Err(Error::Corrupt(
                "logical extent coverage has a gap or overlap",
            ));
        }
        end = end
            .checked_add(extent.length)
            .ok_or(Error::Corrupt("extent length overflow"))?;
        if let Some(last) = merged.last_mut()
            && last.source == extent.source
            && last.physical + last.length == extent.physical
        {
            last.length += extent.length;
        } else {
            merged.push(extent);
        }
    }
    if end != size {
        return Err(Error::Corrupt(
            "logical extent coverage differs from declared size",
        ));
    }
    Ok(merged)
}

fn span_at(extents: &[Extent], offset: u64, wanted: u64) -> Option<SourceSpan> {
    let index = extents.partition_point(|extent| extent.logical + extent.length <= offset);
    let extent = extents.get(index)?;
    let within = offset.checked_sub(extent.logical)?;
    Some(SourceSpan {
        member: extent.source,
        offset: extent.physical + within,
        length: wanted.min(extent.length - within),
    })
}

fn read_volume<R: Read + Seek>(
    sources: &mut [R],
    extents: &[Extent],
    mut offset: u64,
    mut bytes: &mut [u8],
) -> io::Result<()> {
    while !bytes.is_empty() {
        let span =
            span_at(extents, offset, bytes.len() as u64).ok_or(io::ErrorKind::UnexpectedEof)?;
        let (head, tail) = bytes.split_at_mut(span.length as usize);
        read_exact_at(&mut sources[span.member], span.offset, head)?;
        bytes = tail;
        offset += span.length;
    }
    Ok(())
}

impl<R: Read + Seek> Read for Volume<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let wanted = self
            .size
            .saturating_sub(self.position)
            .min(bytes.len() as u64) as usize;
        read_volume(
            &mut self.sources,
            &self.extents,
            self.position,
            &mut bytes[..wanted],
        )?;
        self.position += wanted as u64;
        Ok(wanted)
    }
}

impl<R: Read + Seek> Seek for Volume<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.size.checked_add_signed(delta),
        };
        self.position = position.ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}
