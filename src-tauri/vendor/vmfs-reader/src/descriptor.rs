//! VMFS5/VMFS6 descriptor layouts and the bootstrap system descriptors.
use crate::{Error, Geometry, Result, Version, le32, le64};

pub const ROOT: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Directory,
    Regular,
    Symlink,
    RawDeviceMapping,
}

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub address: u32,
    pub generation: u32,
    pub kind: FileKind,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub link_count: u32,
    pub modified: u32,
    pub changed: u32,
    pub accessed: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub size: usize,
    metadata: usize,
    data: usize,
    pointers: usize,
    pub pointer_width: usize,
    pub pointer_fanout: u64,
}

impl Layout {
    pub(crate) fn new(geometry: &Geometry) -> Self {
        match geometry.version {
            Version::Vmfs5 => Self {
                size: 2048,
                metadata: 512,
                data: 1024,
                pointers: 1024,
                pointer_width: 4,
                pointer_fanout: 1024,
            },
            Version::Vmfs6 => {
                let metadata = geometry.metadata_alignment as usize;
                let size = 2 * metadata;
                let pointer_bytes = if metadata <= 4096 { 2560 } else { metadata / 2 };
                Self {
                    size,
                    metadata,
                    data: metadata + 512,
                    pointers: size - pointer_bytes,
                    pointer_width: 8,
                    pointer_fanout: (metadata / 8).max(8192) as u64,
                }
            }
        }
    }

    pub(crate) fn bootstrap_offset(self, geometry: &Geometry, address: u32) -> Result<u64> {
        let (cluster, resource) = descriptor_address(u64::from(address));
        if cluster != 0 {
            return Err(Error::Corrupt("system descriptor cluster"));
        }
        let (group_base, cluster_header) = match geometry.version {
            Version::Vmfs5 => {
                if geometry.config & 4 == 0 {
                    for (id, offset) in [
                        geometry.second_pointer_descriptor,
                        geometry.system_directory_descriptor,
                    ] {
                        if id == address && offset != 0 {
                            return Ok(offset);
                        }
                    }
                }
                let size = geometry.file_block_size;
                (size * ((size + 0x3fffff) / size), 1024)
            }
            Version::Vmfs6 => (
                (geometry.metadata_alignment * 1024 + 0x3fffff) & !0xfffff,
                self.size as u64,
            ),
        };
        Ok(group_base
            + geometry.descriptor_group_offset
            + geometry.descriptor_clusters_per_group * cluster_header
            + resource * self.size as u64)
    }
}

pub(crate) fn descriptor_address(address: u64) -> (u64, u64) {
    ((address >> 6) & 0xffff, (address >> 22) & 0x3ff)
}

pub(crate) struct Descriptor {
    pub info: FileInfo,
    pub zla: u32,
    pub block_size: u64,
    pub layout: Layout,
    pub raw: Vec<u8>,
}

impl Descriptor {
    pub(crate) fn parse(raw: Vec<u8>, address: u32, layout: Layout) -> Result<Self> {
        if raw.len() != layout.size {
            return Err(Error::Corrupt("short file descriptor"));
        }
        let meta = &raw[layout.metadata..];
        if le32(meta, 0) != address || (layout.pointer_width == 8 && le32(meta, 0x64) != 0x66646d64)
        {
            return Err(Error::Corrupt("file descriptor address or signature"));
        }
        let size = le64(meta, 0x14);
        let block_size = le64(meta, 0x1c);
        let mode = le32(meta, 0x40);
        let zla = le32(meta, 0x44);
        let kind = match le32(meta, 12) {
            2 => FileKind::Directory,
            3 => FileKind::Regular,
            4 => FileKind::Symlink,
            5 if mode & 0xf000 == 0x4000 => FileKind::Directory,
            5 => FileKind::Regular,
            6 => FileKind::RawDeviceMapping,
            _ => return Err(Error::Corrupt("unsupported file descriptor type")),
        };
        if size != 0
            && (!block_size.is_power_of_two()
                || block_size.trailing_zeros() != u32::from(meta[0x89]))
        {
            return Err(Error::Corrupt("file block geometry"));
        }
        if zla == 0x10d1 && size > (layout.size - layout.data) as u64 {
            return Err(Error::Corrupt("resident file length exceeds descriptor"));
        }
        Ok(Self {
            info: FileInfo {
                address,
                generation: le32(meta, 4),
                kind,
                size,
                mode,
                uid: le32(meta, 0x38),
                gid: le32(meta, 0x3c),
                link_count: le32(meta, 8),
                modified: le32(meta, 0x2c),
                changed: le32(meta, 0x30),
                accessed: le32(meta, 0x34),
            },
            zla,
            block_size,
            layout,
            raw,
        })
    }

    pub(crate) fn pointer(&self, index: u64) -> Result<u64> {
        let capacity = (self.layout.size - self.layout.pointers) / self.layout.pointer_width;
        if index >= capacity as u64 {
            return Err(Error::Corrupt(
                "logical block exceeds the descriptor pointer table",
            ));
        }
        let start = self.layout.pointers + index as usize * self.layout.pointer_width;
        Ok(if self.layout.pointer_width == 4 {
            u64::from(le32(&self.raw, start))
        } else {
            le64(&self.raw, start)
        })
    }

    pub(crate) fn resident(&self, offset: u64, length: usize) -> &[u8] {
        let start = self.layout.data + offset as usize;
        &self.raw[start..start + length]
    }
}
