//! SNIA DDF (Common RAID Disk Data Format) metadata decoder.
//!
//! DDF is the on-disk metadata of Dell PERC, LSI/Broadcom MegaRAID and Adaptec
//! controllers and of mdadm's `ddf` containers. Every member disk ends with an
//! anchor header (magic `0xDE11DE11`) in its last 512-byte sector. The anchor
//! points at a primary and an optional secondary copy of the metadata; each
//! copy is a header followed by sections at header-relative sector offsets:
//!
//! - physical disk records — every disk of the container with its reference
//!   number and state (global, the same on every disk);
//! - virtual disk records — every virtual disk with its GUID, name and state
//!   (global);
//! - configuration records — the geometry of each virtual disk this disk
//!   belongs to: level, strip size, the reference number of the disk in every
//!   position and where the virtual disk starts on it (local);
//! - physical disk data — this disk's own GUID and reference number (local).
//!
//! All fields are big-endian; every structure carries a CRC-32 over its whole
//! length computed with the CRC field set to `0xFFFFFFFF`. A copy is used only
//! when its header and every section it is read from pass their CRC. Field
//! layout and copy selection follow mdadm's `super-ddf.c`; block addresses are
//! in 512-byte sectors.

use crate::core::evidence::partitions::SectorSource;

pub const HEADER_MAGIC: u32 = 0xDE11_DE11;
const PHYS_RECORDS_MAGIC: u32 = 0x2222_2222;
const PHYS_DATA_MAGIC: u32 = 0x3333_3333;
const VIRT_RECORDS_MAGIC: u32 = 0xDDDD_DDDD;
const VD_CONFIG_MAGIC: u32 = 0xEEEE_EEEE;

/// Header `type` of the primary and secondary copies.
const HEADER_PRIMARY: u8 = 1;
const HEADER_SECONDARY: u8 = 2;

/// Physical disk record `state` bits.
pub const PD_ONLINE: u16 = 0x01;
pub const PD_FAILED: u16 = 0x02;
pub const PD_REBUILDING: u16 = 0x04;
pub const PD_MISSING: u16 = 0x40;
/// Physical disk record `type` bits of hot spares.
pub const PD_GLOBAL_SPARE: u16 = 0x04;
pub const PD_SPARE: u16 = 0x08;

/// Virtual disk record `state`: the low three bits and two flags.
pub const VD_STATE_MASK: u8 = 0x07;
pub const VD_STATE_DELETED: u8 = 0x02;
pub const VD_MORPHING: u8 = 0x08;
pub const VD_INCONSISTENT: u8 = 0x10;
/// Virtual disk record `init_state`: the low two bits.
pub const VD_INIT_MASK: u8 = 0x03;
pub const VD_INIT_FULL: u8 = 0x02;

/// Reference number of an unused position.
pub const NO_REFNUM: u32 = 0xFFFF_FFFF;

const SECTOR: usize = 512;
/// Largest section mdadm reads, in sectors.
const MAX_SECTION_SECTORS: u32 = 1024;
/// Section header before the records of the physical and virtual disk
/// sections, and the size of one record.
const RECORDS_OFFSET: usize = 64;
const RECORD_BYTES: usize = 64;
/// Fixed part of a configuration record; the position table follows.
const CONFIG_FIXED_BYTES: usize = 512;

/// The header copy whose sections were read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdfHeader {
    /// Identifies the container; the same on every member disk.
    pub guid: [u8; 24],
    /// Format revision, e.g. `01.02.00`.
    pub revision: String,
    /// Incremented on every metadata update.
    pub seq: u32,
    /// The copy was being written when the disk was last used.
    pub open: bool,
}

impl DdfHeader {
    /// Sequence number for choosing the freshest copy: an open copy counts as
    /// one update older, as mdadm counts it.
    pub fn effective_seq(&self) -> u32 {
        self.seq.saturating_sub(u32::from(self.open))
    }
}

/// One physical disk record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdfPhysDisk {
    pub guid: [u8; 24],
    pub refnum: u32,
    pub kind: u16,
    pub state: u16,
}

impl DdfPhysDisk {
    /// Online and neither failed, being rebuilt nor missing.
    pub fn in_sync(&self) -> bool {
        self.state & PD_ONLINE != 0 && self.state & (PD_FAILED | PD_REBUILDING | PD_MISSING) == 0
    }

    pub fn is_spare(&self) -> bool {
        self.kind & (PD_GLOBAL_SPARE | PD_SPARE) != 0
    }
}

/// One virtual disk record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdfVirtualDisk {
    pub guid: [u8; 24],
    pub state: u8,
    pub init_state: u8,
    pub name: String,
}

/// One configuration record: one basic virtual disk (BVD). A virtual disk
/// with a secondary level (RAID10 as mirrored BVDs striped together) has one
/// record per BVD, told apart by `sec_elmnt_seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdfConfig {
    /// GUID of the virtual disk.
    pub guid: [u8; 24],
    /// Incremented whenever this record changes.
    pub seqnum: u32,
    pub prim_elmnt_count: u16,
    /// Strip size is `512 << chunk_shift` bytes.
    pub chunk_shift: u8,
    /// Primary RAID level, its qualifier, and the secondary level.
    pub prl: u8,
    pub rlq: u8,
    pub sec_elmnt_count: u8,
    pub sec_elmnt_seq: u8,
    pub srl: u8,
    /// Sectors of each disk the BVD uses.
    pub blocks: u64,
    /// Sectors of the virtual disk.
    pub array_blocks: u64,
    /// Reference number of the disk in each position (`NO_REFNUM` unused).
    pub phys_refnum: Vec<u32>,
    /// Sector where the BVD starts on the disk in each position.
    pub lba_offset: Vec<u64>,
}

/// The metadata one member disk carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdfMetadata {
    pub header: DdfHeader,
    pub phys_disks: Vec<DdfPhysDisk>,
    pub virtual_disks: Vec<DdfVirtualDisk>,
    /// This disk's configuration records.
    pub configs: Vec<DdfConfig>,
    /// This disk's own identity.
    pub disk_guid: [u8; 24],
    pub disk_refnum: u32,
}

/// What the end of a device holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdfProbe {
    /// No DDF anchor.
    Absent,
    /// An anchor whose metadata cannot be used; the reason is for diagnostics.
    Invalid(String),
    Valid(Box<DdfMetadata>),
}

/// Read the DDF metadata of `source`, which must be a whole disk.
pub fn probe(source: &dyn SectorSource) -> anyhow::Result<DdfProbe> {
    let size = source.size();
    if size < 2 * SECTOR as u64 || !size.is_multiple_of(SECTOR as u64) {
        return Ok(DdfProbe::Absent);
    }
    let sectors = size / SECTOR as u64;
    let anchor = read_sectors(source, sectors - 1, 1)?;
    if be_u32(&anchor, 0) != Some(HEADER_MAGIC) {
        return Ok(DdfProbe::Absent);
    }
    Ok(match read_metadata(source, sectors, &anchor)? {
        Ok(metadata) => DdfProbe::Valid(Box::new(metadata)),
        Err(reason) => DdfProbe::Invalid(reason),
    })
}

/// CRC-32 of `bytes` with the CRC field (bytes 4..8) read as `0xFFFFFFFF`.
pub fn crc(bytes: &[u8]) -> u32 {
    let mut checksum = flate2::Crc::new();
    checksum.update(bytes.get(..4).unwrap_or(bytes));
    if bytes.len() >= 8 {
        checksum.update(&[0xFF; 4]);
        checksum.update(&bytes[8..]);
    }
    checksum.sum()
}

/// Whether the CRC stored in a structure matches its contents.
fn crc_valid(bytes: &[u8]) -> bool {
    be_u32(bytes, 4) == Some(crc(bytes))
}

/// Outer `Err` is an I/O failure; inner `Err` is unusable metadata.
type Decoded<T> = anyhow::Result<Result<T, String>>;

fn read_metadata(source: &dyn SectorSource, sectors: u64, anchor: &[u8]) -> Decoded<DdfMetadata> {
    if !crc_valid(anchor) {
        return Ok(Err("anchor header fails its CRC".into()));
    }
    let revision = text(&anchor[32..40]);
    if !(revision.starts_with("01.") || revision.starts_with("02.")) {
        return Ok(Err(format!("revision {revision:?} is not supported")));
    }
    let primary_lba = be_u64(anchor, 96).unwrap_or(u64::MAX);
    let secondary_lba = be_u64(anchor, 104).unwrap_or(u64::MAX);
    let primary = read_copy(source, sectors, anchor, primary_lba, HEADER_PRIMARY)?;
    let secondary = read_copy(source, sectors, anchor, secondary_lba, HEADER_SECONDARY)?;
    // The secondary copy wins only when it is newer, or equally new and the
    // primary was left open.
    let (copy, other) = match (primary, secondary) {
        (Some(primary), Some(secondary)) => {
            let primary_seq = header_seq(&primary.0);
            let secondary_seq = header_seq(&secondary.0);
            let primary_open = primary.0[48] != 0;
            let secondary_open = secondary.0[48] != 0;
            if (primary_seq < secondary_seq && !secondary_open)
                || (primary_seq == secondary_seq && primary_open && !secondary_open)
            {
                (secondary, Some(primary))
            } else {
                (primary, Some(secondary))
            }
        }
        (Some(only), None) | (None, Some(only)) => (only, None),
        (None, None) => {
            return Ok(Err(
                "neither the primary nor the secondary header is valid".into()
            ))
        }
    };
    match read_sections(source, sectors, &copy.0, copy.1)? {
        Ok(metadata) => Ok(Ok(metadata)),
        // A damaged chosen copy falls back to the other one.
        Err(reason) => match other {
            Some((header, lba)) => Ok(read_sections(source, sectors, &header, lba)?
                .map_err(|second| format!("{reason}; the other copy: {second}"))),
            None => Ok(Err(reason)),
        },
    }
}

fn header_seq(header: &[u8]) -> u32 {
    be_u32(header, 40).unwrap_or(0)
}

/// A primary or secondary header that is consistent with the anchor, and
/// its sector.
fn read_copy(
    source: &dyn SectorSource,
    sectors: u64,
    anchor: &[u8],
    lba: u64,
    kind: u8,
) -> anyhow::Result<Option<(Vec<u8>, u64)>> {
    if lba == u64::MAX || lba >= sectors - 1 {
        return Ok(None);
    }
    let header = read_sectors(source, lba, 1)?;
    let consistent = be_u32(&header, 0) == Some(HEADER_MAGIC)
        && crc_valid(&header)
        && header[8..40] == anchor[8..40]
        && header[96..112] == anchor[96..112]
        && header[112] == kind
        && header[113..116] == anchor[113..116];
    Ok(consistent.then_some((header, lba)))
}

fn read_sections(
    source: &dyn SectorSource,
    sectors: u64,
    header: &[u8],
    base: u64,
) -> Decoded<DdfMetadata> {
    let word = |at: usize| be_u32(header, at).unwrap_or(0);
    let half = |at: usize| be_u16(header, at).unwrap_or(0);
    let mut guid = [0u8; 24];
    guid.copy_from_slice(&header[8..32]);
    let parsed = DdfHeader {
        guid,
        revision: text(&header[32..40]),
        seq: word(40),
        open: header[48] != 0,
    };
    // Each configuration record is `config_record_len` sectors; its position
    // table holds `max_primary_element_entries` reference numbers and start
    // sectors.
    let record_sectors = half(134);
    let mppe = usize::from(half(136));
    let record_bytes = usize::from(record_sectors) * SECTOR;
    if record_bytes == 0 || record_bytes < CONFIG_FIXED_BYTES + 12 * mppe {
        return Ok(Err(format!(
            "configuration records of {record_sectors} sectors cannot hold {mppe} positions"
        )));
    }

    let section = |offset_at: usize| -> Decoded<Vec<u8>> {
        let (offset, length) = (word(offset_at), word(offset_at + 4));
        if length == 0 || length > MAX_SECTION_SECTORS {
            return Ok(Err(format!(
                "a section of {length} sectors is out of range"
            )));
        }
        let Some(start) = base.checked_add(u64::from(offset)) else {
            return Ok(Err("a section offset overflows".into()));
        };
        if start.saturating_add(u64::from(length)) > sectors {
            return Ok(Err("a section lies past the end of the disk".into()));
        }
        Ok(Ok(read_sectors(source, start, u64::from(length))?))
    };
    let checked = |bytes: Vec<u8>, magic: u32, name: &str| -> Result<Vec<u8>, String> {
        if be_u32(&bytes, 0) != Some(magic) {
            return Err(format!("{name} section has the wrong magic"));
        }
        if !crc_valid(&bytes) {
            return Err(format!("{name} section fails its CRC"));
        }
        Ok(bytes)
    };

    let phys =
        match section(200)?.and_then(|bytes| checked(bytes, PHYS_RECORDS_MAGIC, "physical disk")) {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(Err(reason)),
        };
    let virt =
        match section(208)?.and_then(|bytes| checked(bytes, VIRT_RECORDS_MAGIC, "virtual disk")) {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(Err(reason)),
        };
    let config = match section(216)? {
        Ok(bytes) => bytes,
        Err(reason) => return Ok(Err(reason)),
    };
    let data = match section(224)? {
        Ok(bytes) => bytes,
        Err(reason) => return Ok(Err(reason)),
    };
    let Some(disk_data) = data.get(..SECTOR) else {
        return Ok(Err("physical disk data section is truncated".into()));
    };
    let disk_data = match checked(disk_data.to_vec(), PHYS_DATA_MAGIC, "physical disk data") {
        Ok(bytes) => bytes,
        Err(reason) => return Ok(Err(reason)),
    };
    let mut disk_guid = [0u8; 24];
    disk_guid.copy_from_slice(&disk_data[8..32]);

    let configs = match config_records(&config, record_bytes, mppe) {
        Ok(configs) => configs,
        Err(reason) => return Ok(Err(reason)),
    };
    Ok(Ok(DdfMetadata {
        header: parsed,
        phys_disks: phys_records(&phys),
        virtual_disks: virtual_records(&virt),
        configs,
        disk_guid,
        disk_refnum: be_u32(&disk_data, 32).unwrap_or(NO_REFNUM),
    }))
}

/// Used physical disk records.
fn phys_records(section: &[u8]) -> Vec<DdfPhysDisk> {
    records(section)
        .filter_map(|record| {
            let refnum = be_u32(record, 24)?;
            if refnum == NO_REFNUM {
                return None;
            }
            let mut guid = [0u8; 24];
            guid.copy_from_slice(&record[..24]);
            Some(DdfPhysDisk {
                guid,
                refnum,
                kind: be_u16(record, 28)?,
                state: be_u16(record, 30)?,
            })
        })
        .collect()
}

/// Populated virtual disk records (an all-`0xFF` GUID marks a free one).
fn virtual_records(section: &[u8]) -> Vec<DdfVirtualDisk> {
    records(section)
        .filter(|record| record[..24].iter().any(|&byte| byte != 0xFF))
        .map(|record| {
            let mut guid = [0u8; 24];
            guid.copy_from_slice(&record[..24]);
            DdfVirtualDisk {
                guid,
                state: record[32],
                init_state: record[33],
                name: text(&record[48..64]),
            }
        })
        .collect()
}

/// The records of a physical or virtual disk section, bounded by both the
/// record count the section header claims and the section's length.
fn records(section: &[u8]) -> impl Iterator<Item = &[u8; RECORD_BYTES]> {
    let claimed = usize::from(be_u16(section, 10).unwrap_or(0));
    section
        .get(RECORDS_OFFSET..)
        .unwrap_or_default()
        .as_chunks::<RECORD_BYTES>()
        .0
        .iter()
        .take(claimed)
}

/// Every configuration record in the section. Records of other kinds
/// (spare assignments, vendor records, unused slots) are skipped; a
/// configuration record failing its CRC makes the copy unusable.
fn config_records(
    section: &[u8],
    record_bytes: usize,
    mppe: usize,
) -> Result<Vec<DdfConfig>, String> {
    let mut configs = Vec::new();
    for (index, record) in section.chunks_exact(record_bytes).enumerate() {
        if be_u32(record, 0) != Some(VD_CONFIG_MAGIC) {
            continue;
        }
        if !crc_valid(record) {
            return Err(format!("configuration record {index} fails its CRC"));
        }
        let mut guid = [0u8; 24];
        guid.copy_from_slice(&record[8..32]);
        let table = &record[CONFIG_FIXED_BYTES..];
        let phys_refnum = (0..mppe)
            .map(|position| be_u32(table, position * 4).unwrap_or(NO_REFNUM))
            .collect();
        let lba_offset = (0..mppe)
            .map(|position| be_u64(table, mppe * 4 + position * 8).unwrap_or(u64::MAX))
            .collect();
        configs.push(DdfConfig {
            guid,
            seqnum: be_u32(record, 36).unwrap_or(0),
            prim_elmnt_count: be_u16(record, 64).unwrap_or(0),
            chunk_shift: record[66],
            prl: record[67],
            rlq: record[68],
            sec_elmnt_count: record[69],
            sec_elmnt_seq: record[70],
            srl: record[71],
            blocks: be_u64(record, 72).unwrap_or(0),
            array_blocks: be_u64(record, 80).unwrap_or(0),
            phys_refnum,
            lba_offset,
        });
    }
    Ok(configs)
}

fn read_sectors(source: &dyn SectorSource, lba: u64, count: u64) -> anyhow::Result<Vec<u8>> {
    let offset = lba
        .checked_mul(SECTOR as u64)
        .ok_or_else(|| anyhow::anyhow!("DDF sector {lba} overflows"))?;
    let len = count as usize * SECTOR;
    let bytes = source.read(offset, len)?;
    if bytes.len() != len {
        anyhow::bail!(
            "short read at offset {offset}: {} of {len} bytes",
            bytes.len()
        );
    }
    Ok(bytes)
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Space- or NUL-padded ASCII field as text.
fn text(bytes: &[u8]) -> String {
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
#[path = "ddf_fixture.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "ddf_tests.rs"]
mod tests;
