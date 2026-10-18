//! Synthetic IMSM metadata for tests: an encoder for MPBs and a helper that
//! writes one at the end of a member image the way the firmware lays it out.
use super::*;

#[derive(Clone)]
pub(crate) struct DiskFields {
    pub serial: &'static str,
    /// In metadata blocks (512 or 4096 bytes).
    pub total_blocks: u64,
    pub status: u32,
}

#[derive(Clone)]
pub(crate) struct MapFields {
    /// Block counts in metadata blocks.
    pub pba_of_lba0: u64,
    pub blocks_per_member: u64,
    pub num_data_stripes: u64,
    pub blocks_per_strip: u16,
    pub map_state: u8,
    pub raw_level: u8,
    pub num_domains: u8,
    pub ord: Vec<u32>,
}

#[derive(Clone)]
pub(crate) struct VolumeFields {
    pub name: &'static str,
    /// In metadata blocks.
    pub size_blocks: u64,
    pub migr_state: u8,
    pub migr_type: u8,
    pub dirty: u8,
    pub status: u32,
    pub map: MapFields,
    pub source_map: Option<MapFields>,
}

#[derive(Clone)]
pub(crate) struct MpbFields {
    pub version: &'static str,
    pub family_num: u32,
    pub generation_num: u32,
    pub attributes: u32,
    pub disks: Vec<DiskFields>,
    pub volumes: Vec<VolumeFields>,
    /// Extra zero bytes appended to the MPB, to grow it past one sector.
    pub padding: usize,
}

impl MapFields {
    /// A map of `members` slots filled by disks `0..members` in order, using
    /// `component_blocks` of every member from the start of the disk. The
    /// data stripe count is the one the firmware writes for that component.
    pub(crate) fn new(
        raw_level: u8,
        members: u32,
        strip_blocks: u16,
        component_blocks: u64,
    ) -> Self {
        let num_domains = if matches!(raw_level, 1 | 10) { 2 } else { 1 };
        Self {
            pba_of_lba0: 0,
            blocks_per_member: component_blocks,
            num_data_stripes: component_blocks / u64::from(strip_blocks) / u64::from(num_domains),
            blocks_per_strip: strip_blocks,
            map_state: MAP_NORMAL,
            raw_level,
            num_domains,
            ord: (0..members).collect(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; MAP_FIXED_BYTES];
        put_split(&mut out, 0, 20, self.pba_of_lba0);
        put_split(&mut out, 4, 24, self.blocks_per_member);
        put_split(&mut out, 8, 28, self.num_data_stripes);
        out[12..14].copy_from_slice(&self.blocks_per_strip.to_le_bytes());
        out[14] = self.map_state;
        out[15] = self.raw_level;
        out[16] = self.ord.len() as u8;
        out[17] = self.num_domains;
        out[18] = 0xFF;
        for entry in &self.ord {
            out.extend_from_slice(&entry.to_le_bytes());
        }
        out
    }
}

impl VolumeFields {
    pub(crate) fn new(name: &'static str, map: MapFields, size_blocks: u64) -> Self {
        Self {
            name,
            size_blocks,
            migr_state: 0,
            migr_type: 0,
            dirty: 0,
            status: 0,
            map,
            source_map: None,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; DEV_FIXED_BYTES];
        out[..self.name.len()].copy_from_slice(self.name.as_bytes());
        put_split(&mut out, 16, 20, self.size_blocks);
        out[24..28].copy_from_slice(&self.status.to_le_bytes());
        out[88] = self.migr_state;
        out[89] = self.migr_type;
        out[90] = self.dirty;
        out.extend(self.map.encode());
        if let Some(source) = &self.source_map {
            out.extend(source.encode());
        }
        out
    }
}

impl MpbFields {
    pub(crate) fn new(disks: Vec<DiskFields>, volumes: Vec<VolumeFields>) -> Self {
        Self {
            version: "1.3.00",
            family_num: 0x1234_5678,
            generation_num: 40,
            attributes: ATTRIB_CHECKSUM_VERIFY
                | ATTRIB_RAID0
                | ATTRIB_RAID1
                | ATTRIB_RAID5
                | ATTRIB_RAID10,
            disks,
            volumes,
            padding: 0,
        }
    }

    /// The MPB bytes (`mpb_size` long) with a valid checksum.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; DISK_TABLE_OFFSET];
        out[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        out[SIGNATURE.len()..SIGNATURE.len() + self.version.len()]
            .copy_from_slice(self.version.as_bytes());
        out[0x28..0x2C].copy_from_slice(&self.family_num.to_le_bytes());
        out[0x2C..0x30].copy_from_slice(&self.generation_num.to_le_bytes());
        out[0x34..0x38].copy_from_slice(&self.attributes.to_le_bytes());
        out[0x38] = self.disks.len() as u8;
        out[0x39] = self.volumes.len() as u8;
        out[0x40..0x44].copy_from_slice(&self.family_num.to_le_bytes());
        for disk in &self.disks {
            let mut entry = vec![0u8; DISK_BYTES];
            entry[..disk.serial.len()].copy_from_slice(disk.serial.as_bytes());
            put_split(&mut entry, 16, 32, disk.total_blocks);
            entry[24..28].copy_from_slice(&disk.status.to_le_bytes());
            out.extend(entry);
        }
        if self.disks.is_empty() {
            out.extend([0u8; DISK_BYTES]);
        }
        for volume in &self.volumes {
            out.extend(volume.encode());
        }
        out.extend(vec![0u8; self.padding]);
        let size = out.len() as u32;
        out[0x24..0x28].copy_from_slice(&size.to_le_bytes());
        let sum = checksum(&out);
        out[0x20..0x24].copy_from_slice(&sum.to_le_bytes());
        out
    }

    /// Write the MPB at the end of `image` (`sector_size`-byte sectors): its
    /// first sector in the second-to-last sector, the rest right before it.
    pub(crate) fn place(&self, image: &mut [u8], sector_size: usize) {
        let mpb = self.encode();
        let sectors = mpb.len().div_ceil(sector_size);
        let mut padded = mpb;
        padded.resize(sectors * sector_size, 0);
        let anchor = image.len() - 2 * sector_size;
        image[anchor..anchor + sector_size].copy_from_slice(&padded[..sector_size]);
        if sectors > 1 {
            let start = image.len() - (sectors + 1) * sector_size;
            image[start..anchor].copy_from_slice(&padded[sector_size..]);
        }
    }
}

/// Store `value` as a low word at `low` and a high word at `high`.
fn put_split(buf: &mut [u8], low: usize, high: usize, value: u64) {
    buf[low..low + 4].copy_from_slice(&(value as u32).to_le_bytes());
    buf[high..high + 4].copy_from_slice(&((value >> 32) as u32).to_le_bytes());
}
