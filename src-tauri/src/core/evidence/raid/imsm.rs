//! Intel Matrix Storage Manager (IMSM) metadata decoder.
//!
//! Intel RST and VROC firmware RAID describe a whole container — its member
//! disks and the volumes over them — in one metadata parameter block (MPB)
//! that every member disk carries. The MPB's first sector, the anchor, is the
//! second-to-last sector of the disk and starts with the signature
//! `Intel Raid ISM Cfg Sig. ` followed by the version; a longer MPB continues
//! in the sectors right before the anchor. The disk table names every member
//! by its drive serial number, and each volume's map lists, slot by slot, the
//! disk-table entry that fills the slot, together with the level, the strip
//! size, where the volume starts on the members and how much of each member it
//! uses. A volume in the middle of a migration carries a second map describing
//! the state it migrates from.
//!
//! All fields are little-endian. Disks with 4096-byte logical sectors keep
//! block counts in 4096-byte units; decoding converts every block count to
//! 512-byte sectors. The checksum, size limits and field semantics follow
//! mdadm's `super-intel.c`.

use sha1::{Digest as _, Sha1};

use crate::core::evidence::partitions::SectorSource;

/// First 24 bytes of every anchor; the version follows in the same field.
pub const SIGNATURE: &[u8; 24] = b"Intel Raid ISM Cfg Sig. ";

/// `imsm_disk.status` bits.
pub const DISK_SPARE: u32 = 0x01;
pub const DISK_CONFIGURED: u32 = 0x02;
pub const DISK_FAILED: u32 = 0x04;
pub const DISK_JOURNAL: u32 = 0x0200_0000;

/// `disk_ord_tbl` flag of a slot whose disk is being rebuilt.
pub const ORD_REBUILD: u32 = 1 << 24;

/// `imsm_map.map_state` values.
pub const MAP_NORMAL: u8 = 0;
pub const MAP_UNINITIALIZED: u8 = 1;
pub const MAP_DEGRADED: u8 = 2;
pub const MAP_FAILED: u8 = 3;

/// `imsm_vol.migr_type` values (`MIGR_REPAIR` is a verify with the
/// verify-and-fix volume flag).
pub const MIGR_INIT: u8 = 0;
pub const MIGR_REBUILD: u8 = 1;
pub const MIGR_VERIFY: u8 = 2;
pub const MIGR_GEN_MIGR: u8 = 3;
pub const MIGR_STATE_CHANGE: u8 = 4;
pub const MIGR_REPAIR: u8 = 5;

/// `imsm_vol.dirty` bit of a volume that was not shut down cleanly.
pub const VOLUME_DIRTY: u8 = 0x01;

/// `imsm_super.attributes` bits a reader may accept; any other bit (SSD
/// caching, Clone-and-Go, RAID1E, …) blocks assembly, as it does in mdadm.
const ATTRIB_RAID0: u32 = 0x0000_0001;
const ATTRIB_RAID1: u32 = 0x0000_0002;
const ATTRIB_RAID10: u32 = 0x0000_0004;
const ATTRIB_RAID5: u32 = 0x0000_0010;
const ATTRIB_EXP_STRIPE_SIZE: u32 = 0x0000_0040;
const ATTRIB_RAID10_EXT: u32 = 0x0000_0080;
const ATTRIB_2TB_DISK: u32 = 0x0400_0000;
const ATTRIB_BBM: u32 = 0x0800_0000;
const ATTRIB_2TB: u32 = 0x2000_0000;
const ATTRIB_NEVER_USE: u32 = 0x4000_0000;
const ATTRIB_CHECKSUM_VERIFY: u32 = 0x8000_0000;
const ATTRIB_ACCEPTED: u32 = ATTRIB_CHECKSUM_VERIFY
    | ATTRIB_2TB
    | ATTRIB_2TB_DISK
    | ATTRIB_RAID0
    | ATTRIB_RAID1
    | ATTRIB_RAID10
    | ATTRIB_RAID5
    | ATTRIB_EXP_STRIPE_SIZE
    | ATTRIB_RAID10_EXT
    | ATTRIB_BBM
    | ATTRIB_NEVER_USE;

/// `imsm_dev.status` flag that turns a verify migration into a repair.
const DEV_VERIFY_AND_FIX: u32 = 0x80;

/// Offset of the disk table, size of one entry, and the smallest MPB
/// (`sizeof(struct imsm_super)` with one disk entry).
const DISK_TABLE_OFFSET: usize = 0xD8;
const DISK_BYTES: usize = 48;
const MPB_MIN_BYTES: u64 = 264;
/// Largest MPB in sectors (`MPB_SECTOR_CNT`).
const MPB_MAX_SECTORS: u64 = 2210;
/// `imsm_dev` up to its first map, and `imsm_map` up to its slot table.
const DEV_FIXED_BYTES: usize = 112;
const MAP_FIXED_BYTES: usize = 48;
/// Disk-table index bits of a `disk_ord_tbl` entry.
const ORD_INDEX_MASK: u32 = 0x00FF_FFFF;

/// One entry of the container's disk table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsmDisk {
    /// Drive serial number as the firmware recorded it (see
    /// [`normalize_serial`]), NUL padded. A disk marked failed carries
    /// `:<index>` after its serial.
    pub serial: [u8; 16],
    /// Size of the disk in 512-byte sectors.
    pub total_sectors: u64,
    pub status: u32,
}

impl ImsmDisk {
    pub fn is_spare(&self) -> bool {
        self.status & DISK_SPARE != 0
    }

    pub fn is_failed(&self) -> bool {
        self.status & DISK_FAILED != 0
    }

    pub fn is_journal(&self) -> bool {
        self.status & DISK_JOURNAL != 0
    }

    /// The serial as text, for diagnostics.
    pub fn serial_text(&self) -> String {
        serial_text(&self.serial)
    }
}

/// One RAID map of a volume. Block counts are in 512-byte sectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsmMap {
    /// Sector where the volume starts on every member.
    pub pba_of_lba0: u64,
    pub blocks_per_member: u64,
    pub num_data_stripes: u64,
    /// Strip (chunk) size.
    pub blocks_per_strip: u64,
    pub map_state: u8,
    /// Level as recorded: 0, 1 (RAID10 when the map has more than two
    /// members), 5 or 10.
    pub raw_level: u8,
    pub num_members: u8,
    /// Copies of each data strip (2 for RAID1 and RAID10).
    pub num_domains: u8,
    /// Per slot: the disk-table index in the low 24 bits, plus flags such as
    /// [`ORD_REBUILD`].
    pub ord: Vec<u32>,
}

impl ImsmMap {
    /// Level the map describes: a RAID1 map with other than two members is
    /// RAID10.
    pub fn level(&self) -> u8 {
        if self.raw_level == 1 && self.num_members != 2 {
            10
        } else {
            self.raw_level
        }
    }

    /// Disk-table index of the disk filling `slot`.
    pub fn disk_index(&self, slot: usize) -> Option<usize> {
        self.ord
            .get(slot)
            .map(|&ord| (ord & ORD_INDEX_MASK) as usize)
    }

    /// Whether the disk in `slot` is still being rebuilt.
    pub fn rebuilding(&self, slot: usize) -> bool {
        self.ord
            .get(slot)
            .is_some_and(|&ord| ord & ORD_REBUILD != 0)
    }

    /// Members holding distinct data: all of them for RAID0, half for the
    /// mirrored levels, all but one for RAID5. `None` for other levels.
    pub fn data_members(&self) -> Option<u64> {
        let members = u64::from(self.num_members);
        match self.level() {
            0 => Some(members),
            1 | 10 => Some(members / 2),
            5 => members.checked_sub(1),
            _ => None,
        }
        .filter(|&count| count > 0)
    }

    /// Sectors of each member the volume uses, as mdadm computes them: the
    /// recorded data stripes, unless they disagree with the volume size by
    /// more than the 1 MiB-per-member rounding, then the volume size split
    /// over the data members; rounded down to whole strips except for RAID1.
    pub fn component_sectors(&self, volume_sectors: u64) -> Option<u64> {
        let data_members = self.data_members()?;
        let copies = if matches!(self.level(), 1 | 10) { 2 } else { 1 };
        let recorded = self
            .num_data_stripes
            .checked_mul(self.blocks_per_strip)?
            .checked_mul(copies)?;
        let mut component = match recorded.checked_mul(data_members) {
            Some(total) if total.abs_diff(volume_sectors) <= 2048 * data_members => recorded,
            _ => volume_sectors / data_members,
        };
        if self.level() != 1 && self.blocks_per_strip > 0 {
            component -= component % self.blocks_per_strip;
        }
        Some(component)
    }
}

/// One volume (`imsm_dev`) of the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsmVolume {
    /// Position in the MPB; part of the volume identity.
    pub index: u32,
    /// Volume name, NUL padded.
    pub name: [u8; 16],
    /// Volume size in 512-byte sectors.
    pub size_sectors: u64,
    pub migrating: bool,
    /// Migration kind while [`migrating`](Self::migrating).
    pub migr_type: u8,
    pub dirty: u8,
    /// Progress of the migration in migration units.
    pub curr_migr_unit: u64,
    /// The volume's state, or a migration's target state.
    pub map: ImsmMap,
    /// While migrating: the state the volume migrates from.
    pub source_map: Option<ImsmMap>,
}

impl ImsmVolume {
    pub fn name_text(&self) -> String {
        serial_text(&self.name)
    }

    /// The map whose slot flags describe the current state: the source map
    /// while migrating (mdadm's `MAP_X`), the only map otherwise.
    pub fn current_map(&self) -> &ImsmMap {
        self.source_map.as_ref().unwrap_or(&self.map)
    }
}

/// A decoded, checksum-verified MPB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsmSuper {
    /// Version text after the signature, e.g. `1.3.00`.
    pub version: String,
    /// Logical sector size the metadata was written with.
    pub sector_size: u32,
    pub checksum: u32,
    /// Shared by every disk of the container.
    pub family_num: u32,
    pub orig_family_num: u32,
    /// Incremented on every metadata update.
    pub generation_num: u32,
    pub attributes: u32,
    pub disks: Vec<ImsmDisk>,
    pub volumes: Vec<ImsmVolume>,
}

impl ImsmSuper {
    /// Attribute bits that block assembly.
    pub fn unsupported_attributes(&self) -> u32 {
        self.attributes & !ATTRIB_ACCEPTED
    }

    /// Volume identity as mdadm derives it: SHA-1 over the signature, the
    /// container's original family number (the current one when unset), and
    /// the volume's index and name.
    pub fn volume_uuid(&self, volume: &ImsmVolume) -> [u8; 16] {
        let family = if self.orig_family_num != 0 {
            self.orig_family_num
        } else {
            self.family_num
        };
        let mut hasher = Sha1::new();
        hasher.update(SIGNATURE);
        hasher.update(family.to_le_bytes());
        hasher.update(volume.index.to_le_bytes());
        hasher.update(volume.name);
        let digest = hasher.finalize();
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&digest[..16]);
        uuid
    }
}

/// What the end of a device holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImsmProbe {
    /// No IMSM anchor.
    Absent,
    /// An anchor whose MPB cannot be used; the reason is for diagnostics.
    Invalid(String),
    Valid(ImsmSuper),
}

/// Read the MPB at the end of `source`, which must be a whole disk. 512-byte
/// sectors are tried first, then 4096-byte ones.
pub fn probe(source: &dyn SectorSource) -> anyhow::Result<ImsmProbe> {
    let size = source.size();
    for sector_size in [512u64, 4096] {
        if size < 2 * sector_size || !size.is_multiple_of(sector_size) {
            continue;
        }
        let anchor = read_exact(source, size - 2 * sector_size, sector_size)?;
        if anchor.get(..SIGNATURE.len()) != Some(SIGNATURE.as_slice()) {
            continue;
        }
        let mpb_size = u64::from(le_u32(&anchor, 0x24).unwrap_or(0));
        if !(MPB_MIN_BYTES..=MPB_MAX_SECTORS * sector_size).contains(&mpb_size) {
            return Ok(ImsmProbe::Invalid(format!(
                "MPB size {mpb_size} is out of range"
            )));
        }
        let sectors = mpb_size.div_ceil(sector_size);
        let mut mpb = anchor;
        if sectors > 1 {
            // The sectors after the anchor's live right before it.
            let Some(start) = size.checked_sub(sector_size * (sectors + 1)) else {
                return Ok(ImsmProbe::Invalid(format!(
                    "MPB of {mpb_size} bytes does not fit the device"
                )));
            };
            mpb.extend(read_exact(source, start, (sectors - 1) * sector_size)?);
        }
        mpb.truncate(mpb_size as usize);
        return Ok(match decode(&mpb, sector_size as u32) {
            Ok(decoded) => ImsmProbe::Valid(decoded),
            Err(reason) => ImsmProbe::Invalid(reason),
        });
    }
    Ok(ImsmProbe::Absent)
}

/// Decode a whole MPB (anchor first, `mpb_size` bytes) written with
/// `sector_size`-byte blocks.
pub fn decode(mpb: &[u8], sector_size: u32) -> Result<ImsmSuper, String> {
    let field = |at: usize| le_u32(mpb, at).ok_or_else(|| "MPB is truncated".to_string());
    let stored = field(0x20)?;
    let computed = checksum(mpb);
    if computed != stored {
        return Err(format!(
            "MPB checksum {computed:#010x} does not match the recorded {stored:#010x}"
        ));
    }
    let version = mpb
        .get(SIGNATURE.len()..32)
        .map(serial_text)
        .unwrap_or_default();
    let attributes = field(0x34)?;
    let num_disks = usize::from(*mpb.get(0x38).ok_or("MPB is truncated")?);
    let num_volumes = *mpb.get(0x39).ok_or("MPB is truncated")?;
    let units = Units {
        scale: if sector_size == 4096 { 8 } else { 1 },
        high_words: attributes & ATTRIB_2TB_DISK != 0,
    };

    let mut disks = Vec::with_capacity(num_disks);
    for index in 0..num_disks {
        let at = DISK_TABLE_OFFSET + index * DISK_BYTES;
        let entry = mpb
            .get(at..at + DISK_BYTES)
            .ok_or_else(|| format!("disk table entry {index} lies outside the MPB"))?;
        let mut serial = [0u8; 16];
        serial.copy_from_slice(&entry[..16]);
        let total = units.blocks(
            le_u32(entry, 16).unwrap_or(0),
            le_u32(entry, 32).unwrap_or(0),
        )?;
        disks.push(ImsmDisk {
            serial,
            total_sectors: total,
            status: le_u32(entry, 24).unwrap_or(0),
        });
    }

    let mut volumes = Vec::with_capacity(usize::from(num_volumes));
    let mut at = DISK_TABLE_OFFSET + num_disks * DISK_BYTES;
    for index in 0..u32::from(num_volumes) {
        let (volume, length) = decode_volume(mpb, at, index, units)?;
        volumes.push(volume);
        at += length;
    }

    Ok(ImsmSuper {
        version,
        sector_size,
        checksum: stored,
        family_num: field(0x28)?,
        orig_family_num: field(0x40)?,
        generation_num: field(0x2C)?,
        attributes,
        disks,
        volumes,
    })
}

/// MPB checksum: the 32-bit sum of every whole little-endian word of the
/// MPB, minus the stored checksum (which is part of the sum).
pub fn checksum(mpb: &[u8]) -> u32 {
    let (words, _) = mpb.as_chunks::<4>();
    let sum = words.iter().fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_le_bytes(*word))
    });
    sum.wrapping_sub(le_u32(mpb, 0x20).unwrap_or(0))
}

/// Normalise a drive serial number the way the firmware records it in the
/// disk table: every byte at or below a space and every non-ASCII byte
/// removed, `:` replaced by `;` (`:` marks failed-disk placeholders), and only
/// the last 16 characters kept. `None` when nothing is left.
pub fn normalize_serial(raw: &str) -> Option<[u8; 16]> {
    let kept: Vec<u8> = raw
        .bytes()
        .filter(|&byte| byte > 0x20 && byte < 0x80)
        .map(|byte| if byte == b':' { b';' } else { byte })
        .collect();
    if kept.is_empty() {
        return None;
    }
    let tail = &kept[kept.len().saturating_sub(16)..];
    let mut serial = [0u8; 16];
    serial[..tail.len()].copy_from_slice(tail);
    Some(serial)
}

/// Serial comparison up to the first NUL, like the firmware's `strncmp`.
pub fn serial_eq(left: &[u8; 16], right: &[u8; 16]) -> bool {
    for (a, b) in left.iter().zip(right) {
        if a != b {
            return false;
        }
        if *a == 0 {
            return true;
        }
    }
    true
}

/// Block-count conversion shared by every field of one MPB.
#[derive(Clone, Copy)]
struct Units {
    /// 512-byte sectors per metadata block.
    scale: u64,
    /// Whether the high words of disk and map block counts are in use (the
    /// MPB records disks larger than 2 TiB); otherwise they are ignored.
    high_words: bool,
}

impl Units {
    fn blocks(&self, low: u32, high: u32) -> Result<u64, String> {
        let high = if self.high_words { high } else { 0 };
        self.scaled(u64::from(low) | u64::from(high) << 32)
    }

    fn scaled(&self, blocks: u64) -> Result<u64, String> {
        blocks
            .checked_mul(self.scale)
            .ok_or_else(|| "MPB block count overflows".to_string())
    }
}

/// Decode the volume at byte `at`; returns it and its length in the MPB.
fn decode_volume(
    mpb: &[u8],
    at: usize,
    index: u32,
    units: Units,
) -> Result<(ImsmVolume, usize), String> {
    let fixed = mpb
        .get(at..at + DEV_FIXED_BYTES)
        .ok_or_else(|| format!("volume {index} lies outside the MPB"))?;
    let mut name = [0u8; 16];
    name.copy_from_slice(&fixed[..16]);
    let word = |offset: usize| le_u32(fixed, offset).unwrap_or(0);
    // The volume size has no 2 TiB gate: its high word is always in use.
    let size_sectors = units.scaled(u64::from(word(16)) | u64::from(word(20)) << 32)?;
    let status = word(24);
    let migrating = fixed[88] != 0;
    let migr_type = match fixed[89] {
        MIGR_VERIFY if status & DEV_VERIFY_AND_FIX != 0 => MIGR_REPAIR,
        other => other,
    };
    let curr_migr_unit = units.scaled(u64::from(word(80)) | u64::from(word(96)) << 32)?;
    let (map, map_length) = decode_map(mpb, at + DEV_FIXED_BYTES, index, units)?;
    let mut length = DEV_FIXED_BYTES + map_length;
    let source_map = if migrating {
        let (source, source_length) = decode_map(mpb, at + length, index, units)?;
        length += source_length;
        Some(source)
    } else {
        None
    };
    Ok((
        ImsmVolume {
            index,
            name,
            size_sectors,
            migrating,
            migr_type,
            dirty: fixed[90],
            curr_migr_unit,
            map,
            source_map,
        },
        length,
    ))
}

/// Decode the map at byte `at`; returns it and its length.
fn decode_map(
    mpb: &[u8],
    at: usize,
    volume: u32,
    units: Units,
) -> Result<(ImsmMap, usize), String> {
    let outside = || format!("a map of volume {volume} lies outside the MPB");
    let fixed = mpb.get(at..at + MAP_FIXED_BYTES).ok_or_else(outside)?;
    let num_members = fixed[16];
    let length = MAP_FIXED_BYTES + 4 * usize::from(num_members);
    let table = mpb
        .get(at + MAP_FIXED_BYTES..at + length)
        .ok_or_else(outside)?;
    let word = |offset: usize| le_u32(fixed, offset).unwrap_or(0);
    let stripes_high = if units.high_words { word(28) } else { 0 };
    let map = ImsmMap {
        pba_of_lba0: units.blocks(word(0), word(20))?,
        blocks_per_member: units.blocks(word(4), word(24))?,
        num_data_stripes: u64::from(word(8)) | u64::from(stripes_high) << 32,
        blocks_per_strip: units.scaled(u64::from(u16::from_le_bytes([fixed[12], fixed[13]])))?,
        map_state: fixed[14],
        raw_level: fixed[15],
        num_members,
        num_domains: fixed[17],
        ord: table
            .as_chunks::<4>()
            .0
            .iter()
            .map(|entry| u32::from_le_bytes(*entry))
            .collect(),
    };
    Ok((map, length))
}

fn read_exact(source: &dyn SectorSource, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
    let bytes = source.read(offset, len as usize)?;
    if bytes.len() as u64 != len {
        anyhow::bail!(
            "short read at offset {offset}: {} of {len} bytes",
            bytes.len()
        );
    }
    Ok(bytes)
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// NUL-terminated ASCII field as text, non-printable bytes dropped.
fn serial_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&byte| byte != 0)
        .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
        .map(|&byte| char::from(byte))
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
#[path = "imsm_fixture.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "imsm_tests.rs"]
mod tests;
