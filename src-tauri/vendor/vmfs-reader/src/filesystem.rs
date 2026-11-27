//! File-descriptor and resource-file lookup over the mapped datastore.
use std::io::{self, Read, Seek, SeekFrom};

use crate::descriptor::{Descriptor, Layout, descriptor_address};
use crate::{
    Error, FileInfo, FileKind, Geometry, Result, Version, Volume, le32, le64, read_exact_at,
};

const FILE_BITMAP: u32 = 0x400004;
const DESCRIPTORS: u32 = 0x800004;
const POINTERS: u32 = 0xc00004;
const SUB_BLOCKS: u32 = 0x1000004;
const POINTERS2: u32 = 0x1800004;
const JOURNAL: u32 = 0x2000004;
const MAX_LOOKUP_DEPTH: u8 = 16;

struct ResourceFile {
    descriptor: Descriptor,
    layout: ResourceLayout,
}

#[derive(Clone, Copy)]
struct ResourceLayout {
    per_cluster: u64,
    clusters_per_group: u64,
    groups_start: u64,
    item_size: u64,
    group_size: u64,
    groups: u64,
    cluster_header: u64,
    child_metadata: u64,
}

impl ResourceLayout {
    fn parse(bytes: &[u8; 88], alignment: u64, version: Version) -> Result<Self> {
        if version == Version::Vmfs6 && le32(bytes, 32) != 0x72666d64 {
            return Err(Error::Corrupt("resource file metadata signature"));
        }
        Ok(Self {
            per_cluster: u64::from(le32(bytes, 0)),
            clusters_per_group: u64::from(le32(bytes, 4)),
            groups_start: u64::from(le32(bytes, 8)),
            item_size: u64::from(le32(bytes, 12)),
            group_size: u64::from(le32(bytes, 16)),
            groups: u64::from(le32(bytes, 24)),
            cluster_header: 2 * alignment,
            child_metadata: u64::from(le32(bytes, 52)),
        })
    }

    fn resource_position(self, cluster: u64, item: u64) -> Result<u64> {
        if self.clusters_per_group == 0 || self.item_size == 0 || item >= self.per_cluster {
            return Err(Error::Corrupt("resource index outside cluster"));
        }
        let group = cluster / self.clusters_per_group;
        if group >= self.groups {
            return Err(Error::Corrupt("resource cluster outside resource file"));
        }
        let index = cluster % self.clusters_per_group * self.per_cluster + item;
        let in_group = index
            .checked_mul(self.item_size)
            .and_then(|offset| offset.checked_add(self.clusters_per_group * self.cluster_header))
            .filter(|&offset| {
                offset
                    .checked_add(self.item_size)
                    .is_some_and(|end| end <= self.group_size)
            })
            .ok_or(Error::Corrupt("resource exceeds cluster group"))?;
        group
            .checked_mul(self.group_size)
            .and_then(|offset| offset.checked_add(self.groups_start))
            .and_then(|offset| offset.checked_add(in_group))
            .ok_or(Error::Corrupt("resource offset overflow"))
    }
}

pub struct Filesystem<R> {
    volume: Volume<R>,
    resources: [Option<ResourceFile>; 8],
    layout: Layout,
    small_blocks_per_cluster: u64,
}

impl<R: Read + Seek> Filesystem<R> {
    pub fn open(volume: Volume<R>) -> Result<Self> {
        let layout = Layout::new(volume.geometry());
        let mut fs = Self {
            volume,
            layout,
            resources: std::array::from_fn(|_| None),
            small_blocks_per_cluster: 0,
        };
        if fs.geometry().second_pointer_descriptor.1 != 0 {
            let address = if fs.geometry().version == Version::Vmfs5 {
                fs.geometry().second_pointer_descriptor.0
            } else {
                POINTERS2
            };
            fs.install_resource(5, address)?;
        }
        fs.install_resource(3, POINTERS)?;
        if fs.geometry().version == Version::Vmfs6 {
            fs.install_resource(7, FILE_BITMAP)?;
            let resource = fs.resources[7].take().unwrap();
            let offset = resource.layout.child_metadata;
            let layout = fs.read_resource_layout(&resource.descriptor, offset)?;
            fs.small_blocks_per_cluster = layout.per_cluster;
            if !fs.small_blocks_per_cluster.is_power_of_two() || fs.small_blocks_per_cluster > 8192
            {
                return Err(Error::Corrupt("small file block cluster size"));
            }
            fs.resources[7] = Some(resource);
            fs.install_resource(6, JOURNAL)?;
        }
        fs.install_resource(4, DESCRIPTORS)?;
        fs.install_resource(2, SUB_BLOCKS)?;
        if fs.file_info(crate::ROOT)?.kind != FileKind::Directory {
            return Err(Error::Corrupt("VMFS root descriptor is not a directory"));
        }
        Ok(fs)
    }

    pub fn geometry(&self) -> &Geometry {
        self.volume.geometry()
    }

    fn read_resource_layout(
        &mut self,
        descriptor: &Descriptor,
        offset: u64,
    ) -> Result<ResourceLayout> {
        let mut bytes = [0; 88];
        read_file(
            &mut self.volume,
            &self.resources,
            self.small_blocks_per_cluster,
            descriptor,
            offset,
            &mut bytes,
            0,
        )?;
        ResourceLayout::parse(
            &bytes,
            self.geometry().metadata_alignment,
            self.geometry().version,
        )
    }

    fn install_resource(&mut self, kind: usize, address: u32) -> Result<()> {
        let offset = self.layout.bootstrap_offset(self.geometry(), address)?;
        let mut bytes = vec![0; self.layout.size];
        read_exact_at(&mut self.volume, offset, &mut bytes)?;
        let descriptor = Descriptor::parse(bytes, address, self.layout)?;
        let layout = self.read_resource_layout(&descriptor, 0)?;
        self.resources[kind] = Some(ResourceFile { descriptor, layout });
        Ok(())
    }

    pub(crate) fn descriptor(&mut self, address: u32) -> Result<Descriptor> {
        if address & 7 != 4 {
            return Err(Error::Corrupt("invalid file descriptor address"));
        }
        let mut bytes = vec![0; self.layout.size];
        read_resource(
            &mut self.volume,
            &self.resources,
            self.small_blocks_per_cluster,
            u64::from(address),
            0,
            &mut bytes,
            0,
        )?;
        Descriptor::parse(bytes, address, self.layout)
    }

    pub fn file_info(&mut self, address: u32) -> Result<FileInfo> {
        Ok(self.descriptor(address)?.info)
    }

    pub fn open_file(&mut self, address: u32) -> Result<FileReader<'_, R>> {
        let descriptor = self.descriptor(address)?;
        check_file_kind(descriptor.info.kind)?;
        Ok(FileReader {
            fs: self,
            descriptor,
            position: 0,
        })
    }

    pub fn read_range(&mut self, address: u32, offset: u64, bytes: &mut [u8]) -> Result<usize> {
        let descriptor = self.descriptor(address)?;
        check_file_kind(descriptor.info.kind)?;
        let count = (bytes.len() as u64).min(descriptor.info.size.saturating_sub(offset)) as usize;
        if count != 0 {
            self.read_descriptor(&descriptor, offset, &mut bytes[..count])?;
        }
        Ok(count)
    }

    pub(crate) fn read_descriptor(
        &mut self,
        descriptor: &Descriptor,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<()> {
        read_file(
            &mut self.volume,
            &self.resources,
            self.small_blocks_per_cluster,
            descriptor,
            offset,
            bytes,
            0,
        )
    }
}

fn check_file_kind(kind: FileKind) -> Result<()> {
    match kind {
        FileKind::Regular | FileKind::Symlink => Ok(()),
        FileKind::Directory => Err(Error::Unsupported(
            "directory content requires directory enumeration",
        )),
        FileKind::RawDeviceMapping => Err(Error::Unsupported(
            "raw device mapping refers to external storage",
        )),
    }
}

pub struct FileReader<'a, R> {
    fs: &'a mut Filesystem<R>,
    descriptor: Descriptor,
    position: u64,
}

impl<R: Read + Seek> Read for FileReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = (bytes.len() as u64)
            .min(self.descriptor.info.size.saturating_sub(self.position))
            as usize;
        if count == 0 {
            return Ok(0);
        }
        self.fs
            .read_descriptor(&self.descriptor, self.position, &mut bytes[..count])
            .map_err(io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for FileReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.descriptor.info.size.checked_add_signed(delta),
        };
        self.position = position.ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}

fn read_file<R: Read + Seek>(
    volume: &mut Volume<R>,
    resources: &[Option<ResourceFile>; 8],
    small_per_cluster: u64,
    descriptor: &Descriptor,
    mut offset: u64,
    mut bytes: &mut [u8],
    depth: u8,
) -> Result<()> {
    if depth > MAX_LOOKUP_DEPTH {
        return Err(Error::Corrupt("cyclic VMFS resource mappings"));
    }
    if offset
        .checked_add(bytes.len() as u64)
        .is_none_or(|end| end > descriptor.info.size)
    {
        return Err(Error::Corrupt("file read exceeds logical size"));
    }
    if bytes.is_empty() {
        return Ok(());
    }
    if descriptor.info.kind == FileKind::RawDeviceMapping {
        return Err(Error::Unsupported(
            "raw device mapping refers to external storage",
        ));
    }
    if descriptor.zla == 0x10d1 {
        bytes.copy_from_slice(descriptor.resident(offset, bytes.len()));
        return Ok(());
    }
    while !bytes.is_empty() {
        let within = offset % descriptor.block_size;
        let index = offset / descriptor.block_size;
        let count = (bytes.len() as u64).min(descriptor.block_size - within) as usize;
        let address = file_block(
            volume,
            resources,
            small_per_cluster,
            descriptor,
            index,
            depth,
        )?;
        let (chunk, rest) = bytes.split_at_mut(count);
        if address == 0 {
            chunk.fill(0);
        } else {
            let kind = (address & 7) as usize;
            let version = volume.geometry().version;
            match kind {
                1 | 7 => {
                    let (base, block_size) = data_block(volume, address, small_per_cluster)?;
                    let tbz = if version == Version::Vmfs5 {
                        (address >> 5) & 1
                    } else {
                        (address >> 7) & 255
                    };
                    if tbz == 0 {
                        let inner = offset % block_size;
                        if inner
                            .checked_add(count as u64)
                            .is_none_or(|end| end > block_size)
                        {
                            return Err(Error::Corrupt("file data block length"));
                        }
                        read_exact_at(volume, base + inner, chunk)?;
                    } else if version == Version::Vmfs5 || tbz == 255 {
                        chunk.fill(0);
                    } else {
                        return Err(Error::Unsupported("partially zeroed VMFS6 data block"));
                    }
                }
                2 => read_resource(
                    volume,
                    resources,
                    small_per_cluster,
                    address,
                    within,
                    chunk,
                    depth + 1,
                )?,
                _ => return Err(Error::Corrupt("unexpected file data block type")),
            }
        }
        bytes = rest;
        offset += count as u64;
    }
    Ok(())
}

fn data_block<R: Read + Seek>(
    volume: &Volume<R>,
    address: u64,
    small_per_cluster: u64,
) -> Result<(u64, u64)> {
    let geometry = volume.geometry();
    let kind = address & 7;
    let size = geometry.file_block_size;
    let (index, block_size) = match (geometry.version, kind) {
        (Version::Vmfs5, 1) => ((address >> 6) & 0x3ffffff, size),
        (Version::Vmfs6, 1) => {
            let small_per_cluster = if small_per_cluster == 0 {
                (0x20000000 / size).min(8192)
            } else {
                small_per_cluster
            };
            let index = (address >> 51) & 0x1fff;
            if index >= small_per_cluster {
                return Err(Error::Corrupt("small file block resource index"));
            }
            (
                ((address >> 15) & 0x7fffffff) * small_per_cluster + index,
                size,
            )
        }
        (Version::Vmfs6, 7) if geometry.large_block_shift <= 16 => (
            (address >> 15) & 0x7fffffff,
            size << geometry.large_block_shift,
        ),
        _ => return Err(Error::Corrupt("invalid file data block type or scale")),
    };
    let offset = index
        .checked_mul(block_size)
        .ok_or(Error::Corrupt("file data offset overflow"))?;
    if offset
        .checked_add(block_size)
        .is_none_or(|end| end > volume.size())
    {
        return Err(Error::Corrupt("file data block exceeds volume"));
    }
    Ok((offset, block_size))
}

fn file_block<R: Read + Seek>(
    volume: &mut Volume<R>,
    resources: &[Option<ResourceFile>; 8],
    small_per_cluster: u64,
    descriptor: &Descriptor,
    index: u64,
    depth: u8,
) -> Result<u64> {
    let levels = match descriptor.zla {
        1 | 2 => 0,
        3 | 5 => 1,
        0x10d0 => 2,
        _ => return Err(Error::Unsupported("file block addressing mode")),
    };
    let fanout = descriptor.layout.pointer_fanout;
    let mut divisor = fanout.pow(levels);
    let mut address = descriptor.pointer(index / divisor)?;
    let mut pointer = [0; 8];
    let width = descriptor.layout.pointer_width;
    for _ in 0..levels {
        if address == 0 {
            return Ok(0);
        }
        divisor /= fanout;
        let entry = index / divisor % fanout;
        read_resource(
            volume,
            resources,
            small_per_cluster,
            address,
            entry * width as u64,
            &mut pointer[..width],
            depth + 1,
        )?;
        address = le64(&pointer, 0);
    }
    Ok(address)
}

fn read_resource<R: Read + Seek>(
    volume: &mut Volume<R>,
    resources: &[Option<ResourceFile>; 8],
    small_per_cluster: u64,
    address: u64,
    within: u64,
    bytes: &mut [u8],
    depth: u8,
) -> Result<()> {
    let kind = (address & 7) as usize;
    if kind == 1 || kind == 7 {
        let (base, size) = data_block(volume, address, small_per_cluster)?;
        if within
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > size)
        {
            return Err(Error::Corrupt("resource length exceeds backing block"));
        }
        read_exact_at(volume, base + within, bytes)?;
        return Ok(());
    }
    let resource = resources[kind]
        .as_ref()
        .ok_or(Error::Corrupt("reference to absent resource file"))?;
    let dense = kind == 2 && volume.geometry().config & 4 != 0;
    let (cluster, item) = match (volume.geometry().version, kind) {
        (_, 4) => descriptor_address(address),
        (Version::Vmfs5, 2 | 3 | 5) => {
            let extra = if dense { (address & 0x18) << 1 } else { 0 };
            ((address >> 6) & 0x3fffff, ((address >> 28) & 0xf) | extra)
        }
        (Version::Vmfs6, 2 | 3 | 5) => ((address >> 6) & 0xfffffffff, address >> 56),
        (Version::Vmfs6, 6) => ((address >> 3) & 0x1fff, (address >> 26) & 0x3f),
        _ => return Err(Error::Corrupt("invalid resource address type")),
    };
    let offset = resource.layout.resource_position(cluster, item)?;
    if within
        .checked_add(bytes.len() as u64)
        .is_none_or(|end| end > resource.layout.item_size)
    {
        return Err(Error::Corrupt("resource read exceeds its item"));
    }
    let offset = offset
        .checked_add(within)
        .ok_or(Error::Corrupt("resource offset overflow"))?;
    read_file(
        volume,
        resources,
        small_per_cluster,
        &resource.descriptor,
        offset,
        bytes,
        depth,
    )
}
