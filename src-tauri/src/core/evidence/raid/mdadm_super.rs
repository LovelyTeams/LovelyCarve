//! Linux md superblock decoder and probe registry.
//!
//! Companion to `partitions/mdadm.rs`: that module answers "is this byte
//! range an mdadm member?" for the partition-table dispatcher. Assembly needs
//! the whole superblock — set identity, level, layout, chunk, member count,
//! data area, role table and event counter — to align members and decide
//! which of them are current.
//!
//! md writes four metadata versions, distinguished by where the superblock
//! sits on the member device:
//!
//! - 1.2: 4 KiB from the start (the default since mdadm 3.1);
//! - 1.1: at the start;
//! - 1.0: 8–12 KiB before the end, 4 KiB aligned;
//! - 0.90: in the last 64 KiB-aligned 64 KiB block, in host byte order.
//!
//! Versions 1.x share one little-endian layout (`struct mdp_superblock_1`);
//! 0.90 is the older word-oriented `mdp_super_t`. Every probe verifies the
//! checksum and, for 1.x, that the recorded `super_offset` names the sector it
//! was found at, so a superblock seen through an enclosing disk or partition
//! is not mistaken for one that belongs to it.

use std::io::{Cursor, SeekFrom};

use binrw::BinRead;

use crate::core::evidence::partitions::SectorSource;

const MD_MAGIC: u32 = 0xA92B4EFC;

/// `feature_map` bits of 1.x superblocks.
pub const FEATURE_RECOVERY_OFFSET: u32 = 1 << 1;
pub const FEATURE_RESHAPE_ACTIVE: u32 = 1 << 2;
pub const FEATURE_BAD_BLOCKS: u32 = 1 << 3;
pub const FEATURE_REPLACEMENT: u32 = 1 << 4;
pub const FEATURE_JOURNAL: u32 = 1 << 9;
pub const FEATURE_RAID0_LAYOUT: u32 = 1 << 12;

/// `dev_roles[]` values that are not array slots.
pub const ROLE_SPARE: u16 = 0xFFFF;
pub const ROLE_FAULTY: u16 = 0xFFFE;
pub const ROLE_JOURNAL: u16 = 0xFFFD;
/// Roles at or above this value are reserved, never slots.
pub const ROLE_MAX: u16 = 0xFF00;

/// Decoded superblock, normalised across metadata versions.
#[derive(Debug, Clone)]
pub struct MdadmSuperblock {
    pub major_version: u32,
    /// Metadata version label: `0.90`, `1.0`, `1.1` or `1.2`.
    pub version: &'static str,
    pub feature_map: u32,
    pub set_uuid: [u8; 16],
    pub set_name: String,
    /// md personality: linear = -1, RAID0 = 0, RAID1 = 1, RAID4/5/6 = 4/5/6,
    /// RAID10 = 10.
    pub level: i32,
    pub layout: u32,
    /// Sectors of each member the array uses (md's `size`); RAID0 and linear
    /// arrays use each member's whole data area instead.
    pub size_sectors: u64,
    /// Stripe size in 512-byte sectors.
    pub chunksize_sectors: u32,
    pub raid_disks: u32,
    /// Sector, from the member start, where the data area begins.
    pub data_offset_sectors: u64,
    /// Sectors of data area this member provides.
    pub data_size_sectors: u64,
    /// Sector, from the member start, where this superblock lives.
    pub super_offset_sectors: u64,
    /// Sectors already rebuilt on a member that is being recovered; only
    /// meaningful with [`FEATURE_RECOVERY_OFFSET`].
    pub recovery_offset_sectors: u64,
    /// Array sectors known to be in sync; `u64::MAX` for a clean array.
    pub resync_offset_sectors: u64,
    /// Next array sector a reshape will move; only meaningful while
    /// [`FEATURE_RESHAPE_ACTIVE`] is set.
    pub reshape_position_sectors: u64,
    /// Incremented on every superblock update; the freshest members carry the
    /// highest count.
    pub events: u64,
    pub device_uuid: [u8; 16],
    /// Permanent device number within the array, indexing `dev_roles`.
    pub dev_number: u32,
    /// `dev_roles[dev_number]` is the slot a device occupies, or one of
    /// [`ROLE_SPARE`] / [`ROLE_FAULTY`] / [`ROLE_JOURNAL`]. md reuses device
    /// numbers across array events (a spare taking over a failed member keeps
    /// its own number but the failed member's slot), so the slot must come from
    /// this table, never from `dev_number`.
    pub dev_roles: Vec<u16>,
    /// 0.90 only: this device was being rebuilt into its slot (active but not
    /// in sync).
    pub recovering: bool,
    pub checksum_valid: bool,
}

impl MdadmSuperblock {
    /// Role recorded for device `dev_number` in this superblock's table.
    pub fn role_of(&self, dev_number: u32) -> Option<u16> {
        self.dev_roles.get(dev_number as usize).copied()
    }

    /// Slot this member occupies according to its own role table: `Some` for
    /// an active data/parity member, `None` for spare, faulty, journal,
    /// out-of-range or malformed roles.
    pub fn slot_index(&self) -> Option<u32> {
        let role = self.role_of(self.dev_number)?;
        (role < ROLE_MAX && u32::from(role) < self.raid_disks).then_some(u32::from(role))
    }
}

#[derive(Debug, BinRead)]
#[br(little)]
struct RawSuperblockV1 {
    magic: u32,
    major_version: u32,
    feature_map: u32,
    #[br(seek_before = SeekFrom::Start(16))]
    set_uuid: [u8; 16],
    set_name_raw: [u8; 32],
    #[br(seek_before = SeekFrom::Start(72))]
    level: i32,
    layout: u32,
    size_sectors: u64,
    chunksize_sectors: u32,
    raid_disks: u32,
    #[br(seek_before = SeekFrom::Start(104))]
    reshape_position: u64,
    #[br(seek_before = SeekFrom::Start(128))]
    data_offset_sectors: u64,
    data_size_sectors: u64,
    super_offset_sectors: u64,
    recovery_offset_sectors: u64,
    dev_number: u32,
    #[br(seek_before = SeekFrom::Start(168))]
    device_uuid: [u8; 16],
    #[br(seek_before = SeekFrom::Start(200))]
    events: u64,
    resync_offset: u64,
    sb_csum: u32,
    max_dev: u32,
}

/// Size of the 1.x fixed section; `dev_roles[]` follows it.
const V1_ROLE_TABLE_OFFSET: usize = 256;

/// Decode a 1.x superblock from the start of `buf`.
pub fn parse_v1(buf: &[u8], version: &'static str) -> Option<MdadmSuperblock> {
    if buf.len() < V1_ROLE_TABLE_OFFSET {
        return None;
    }
    let raw = RawSuperblockV1::read(&mut Cursor::new(buf)).ok()?;
    if raw.magic != MD_MAGIC || raw.major_version != 1 {
        return None;
    }
    let role_bytes = (raw.max_dev as usize).checked_mul(2)?;
    let role_end = V1_ROLE_TABLE_OFFSET.checked_add(role_bytes)?;
    // A table that does not fit the buffer cannot be checksummed, so the
    // superblock cannot be trusted.
    let roles = buf.get(V1_ROLE_TABLE_OFFSET..role_end)?;
    let dev_roles = roles
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect();
    let checksum_valid = v1_checksum(&buf[..role_end]) == raw.sb_csum;
    Some(MdadmSuperblock {
        major_version: raw.major_version,
        version,
        feature_map: raw.feature_map,
        set_uuid: raw.set_uuid,
        set_name: decode_cstring(&raw.set_name_raw),
        level: raw.level,
        layout: raw.layout,
        size_sectors: raw.size_sectors,
        chunksize_sectors: raw.chunksize_sectors,
        raid_disks: raw.raid_disks,
        data_offset_sectors: raw.data_offset_sectors,
        data_size_sectors: raw.data_size_sectors,
        super_offset_sectors: raw.super_offset_sectors,
        recovery_offset_sectors: raw.recovery_offset_sectors,
        resync_offset_sectors: raw.resync_offset,
        reshape_position_sectors: raw.reshape_position,
        events: raw.events,
        device_uuid: raw.device_uuid,
        dev_number: raw.dev_number,
        dev_roles,
        recovering: false,
        checksum_valid,
    })
}

/// 1.x checksum: the 32-bit little-endian words of the superblock through
/// `dev_roles[max_dev]` with `sb_csum` read as zero, summed into 64 bits and
/// folded once (the kernel's `calc_sb_1_csum`).
pub fn v1_checksum(bytes: &[u8]) -> u32 {
    let (words, rest) = bytes.as_chunks::<4>();
    let mut sum: u64 = 0;
    for (index, word) in words.iter().enumerate() {
        // sb_csum lives at byte 216, word 54.
        if index != 54 {
            sum += u64::from(u32::from_le_bytes(*word));
        }
    }
    if let [low, high] = rest {
        sum += u64::from(u16::from_le_bytes([*low, *high]));
    }
    ((sum & 0xffff_ffff) + (sum >> 32)) as u32
}

/// 0.90 superblock size and position (`MD_SB_BYTES`, `MD_RESERVED_SECTORS`).
const V090_BYTES: usize = 4096;
const V090_RESERVED_SECTORS: u64 = 128;
const V090_DISKS: usize = 27;
const V090_DISK_WORDS: usize = 32;
const V090_DISKS_WORD: usize = 128;
const V090_THIS_DISK_WORD: usize = 992;
const V090_CHECKSUM_WORD: usize = 38;
/// `mdp_disk_t.state` bits.
const DISK_FAULTY: u32 = 1 << 0;
const DISK_ACTIVE: u32 = 1 << 1;
const DISK_SYNC: u32 = 1 << 2;
/// `mdp_super_t.state` bit for a cleanly stopped array.
const SB_CLEAN: u32 = 1 << 0;

/// Sector of the 0.90 superblock on a device of `device_sectors`.
pub fn v090_super_sector(device_sectors: u64) -> Option<u64> {
    (device_sectors & !(V090_RESERVED_SECTORS - 1)).checked_sub(V090_RESERVED_SECTORS)
}

/// Decode a 0.90 superblock found at `super_sector`. 0.90 is written in the
/// byte order of the host that created it; both orders are accepted.
pub fn parse_v090(buf: &[u8], super_sector: u64) -> Option<MdadmSuperblock> {
    let buf = buf.get(..V090_BYTES)?;
    let (words, _) = buf.as_chunks::<4>();
    let big_endian = match words.first() {
        Some(magic) if u32::from_le_bytes(*magic) == MD_MAGIC => false,
        Some(magic) if u32::from_be_bytes(*magic) == MD_MAGIC => true,
        _ => return None,
    };
    let word = |index: usize| -> u32 {
        words.get(index).map_or(0, |bytes| {
            if big_endian {
                u32::from_be_bytes(*bytes)
            } else {
                u32::from_le_bytes(*bytes)
            }
        })
    };
    let minor_version = word(2);
    if word(1) != 0 || !(90..=91).contains(&minor_version) {
        return None;
    }
    let mut checksum: u64 = 0;
    for index in 0..V090_BYTES / 4 {
        if index != V090_CHECKSUM_WORD {
            checksum += u64::from(word(index));
        }
    }
    let checksum = ((checksum & 0xffff_ffff) + (checksum >> 32)) as u32;
    // Old kernels stored a 16-bit folded sum on some architectures; md
    // compares the folded values.
    let checksum_valid = csum_fold(checksum) == csum_fold(word(V090_CHECKSUM_WORD));

    let mut set_uuid = [0u8; 16];
    for (target, source) in set_uuid.chunks_mut(4).zip([5, 13, 14, 15]) {
        target.copy_from_slice(&word(source).to_be_bytes());
    }
    // The event counter's halves follow the host order of the struct fields.
    let (events_high, events_low) = if big_endian {
        (word(39), word(40))
    } else {
        (word(40), word(39))
    };
    let disk_state = |disk: usize| word(V090_DISKS_WORD + disk * V090_DISK_WORDS + 4);
    let disk_role = |disk: usize| word(V090_DISKS_WORD + disk * V090_DISK_WORDS + 3);
    let dev_roles = (0..V090_DISKS)
        .map(|disk| {
            let state = disk_state(disk);
            let role = disk_role(disk);
            if state & DISK_FAULTY != 0 {
                ROLE_FAULTY
            } else if state & DISK_SYNC != 0 && role < u32::from(ROLE_MAX) {
                role as u16
            } else {
                ROLE_SPARE
            }
        })
        .collect();
    let this_state = word(V090_THIS_DISK_WORD + 4);
    let state = word(33);
    Some(MdadmSuperblock {
        major_version: 0,
        version: "0.90",
        feature_map: if minor_version >= 91 {
            FEATURE_RESHAPE_ACTIVE
        } else {
            0
        },
        set_uuid,
        set_name: format!("md{}", word(11)),
        level: word(7) as i32,
        layout: word(64),
        size_sectors: u64::from(word(8)) * 2,
        chunksize_sectors: word(65) / 512,
        raid_disks: word(10),
        data_offset_sectors: 0,
        data_size_sectors: super_sector,
        super_offset_sectors: super_sector,
        recovery_offset_sectors: 0,
        resync_offset_sectors: if state & SB_CLEAN != 0 {
            u64::MAX
        } else {
            u64::from(word(43))
        },
        reshape_position_sectors: u64::from(word(44)) | u64::from(word(45)) << 32,
        events: u64::from(events_high) << 32 | u64::from(events_low),
        device_uuid: [0; 16],
        dev_number: word(V090_THIS_DISK_WORD),
        dev_roles,
        recovering: this_state & DISK_ACTIVE != 0
            && this_state & DISK_SYNC == 0
            && this_state & DISK_FAULTY == 0,
        checksum_valid,
    })
}

/// Fold a 32-bit sum to 16 bits (md's `md_csum_fold`).
fn csum_fold(sum: u32) -> u32 {
    let sum = (sum & 0xffff) + (sum >> 16);
    (sum & 0xffff) + (sum >> 16)
}

fn decode_cstring(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|b| format!("{b:02x}")).collect()
}

/// Probe-object pattern for md superblocks, mirroring `PartParser` in
/// `partitions.rs`: each impl knows where its version lives on a member
/// device, reads that region through `SectorSource` and decodes it.
pub trait MdadmSuperblockProbe: Sync {
    /// Version label used in diagnostics (`"1.2"`, `"0.90"`, …).
    fn version(&self) -> &'static str;
    /// Sector where this version's superblock lives on a device of
    /// `device_sectors`, or `None` when the device is too small to hold it.
    fn super_sector(&self, device_sectors: u64) -> Option<u64>;
    /// Decode a superblock read from [`super_sector`](Self::super_sector).
    fn decode(&self, buf: &[u8], super_sector: u64) -> Option<MdadmSuperblock>;
}

const SB_BUF_LEN: usize = 0x1000;
/// Smallest device that can carry a superblock plus some data.
const MIN_MEMBER_SECTORS: u64 = 2 * V090_RESERVED_SECTORS;

struct V1Probe {
    version: &'static str,
}

impl MdadmSuperblockProbe for V1Probe {
    fn version(&self) -> &'static str {
        self.version
    }

    fn super_sector(&self, device_sectors: u64) -> Option<u64> {
        match self.version {
            "1.2" => Some(8),
            "1.1" => Some(0),
            // 8–12 KiB before the end, 4 KiB aligned.
            _ => device_sectors.checked_sub(16).map(|sector| sector & !7),
        }
    }

    fn decode(&self, buf: &[u8], super_sector: u64) -> Option<MdadmSuperblock> {
        parse_v1(buf, self.version).filter(|sb| sb.super_offset_sectors == super_sector)
    }
}

struct V090Probe;

impl MdadmSuperblockProbe for V090Probe {
    fn version(&self) -> &'static str {
        "0.90"
    }

    fn super_sector(&self, device_sectors: u64) -> Option<u64> {
        v090_super_sector(device_sectors)
    }

    fn decode(&self, buf: &[u8], super_sector: u64) -> Option<MdadmSuperblock> {
        parse_v090(buf, super_sector)
    }
}

/// Priority-ordered probe table: the head-of-device versions first (1.2 is
/// what modern mdadm writes), then the end-of-device ones.
static PARSERS: &[&dyn MdadmSuperblockProbe] = &[
    &V1Probe { version: "1.2" },
    &V1Probe { version: "1.1" },
    &V1Probe { version: "1.0" },
    &V090Probe,
];

/// A superblock found on a device.
#[derive(Debug, Clone)]
pub struct ProbeHit {
    pub superblock: MdadmSuperblock,
    /// Byte offset of the superblock on the probed device.
    pub super_offset_bytes: u64,
}

/// Return the first registered version that decodes a superblock on `source`.
/// Superblocks with a bad checksum are returned too (`checksum_valid` false)
/// so the caller can report them; probing does not fail on them.
pub fn probe(source: &dyn SectorSource) -> anyhow::Result<Option<ProbeHit>> {
    let device_sectors = source.size() / 512;
    if device_sectors < MIN_MEMBER_SECTORS {
        return Ok(None);
    }
    for parser in PARSERS {
        let Some(sector) = parser.super_sector(device_sectors) else {
            continue;
        };
        let offset = sector * 512;
        let buf = source.read(offset, SB_BUF_LEN)?;
        if let Some(superblock) = parser.decode(&buf, sector) {
            return Ok(Some(ProbeHit {
                superblock,
                super_offset_bytes: offset,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Fields of a synthetic 1.x superblock.
    #[derive(Clone)]
    pub(crate) struct V1Fields {
        pub set_uuid: [u8; 16],
        pub set_name: &'static str,
        pub level: i32,
        pub layout: u32,
        pub size_sectors: u64,
        pub chunk_sectors: u32,
        pub raid_disks: u32,
        pub data_offset_sectors: u64,
        pub data_size_sectors: u64,
        pub super_offset_sectors: u64,
        pub feature_map: u32,
        pub recovery_offset_sectors: u64,
        pub events: u64,
        pub dev_number: u32,
        pub device_uuid: [u8; 16],
        pub dev_roles: Vec<u16>,
    }

    impl V1Fields {
        pub(crate) fn new(level: i32, raid_disks: u32) -> Self {
            Self {
                set_uuid: [0xAA; 16],
                set_name: "host:0",
                level,
                layout: 0,
                size_sectors: 0,
                chunk_sectors: 8,
                raid_disks,
                data_offset_sectors: 0,
                data_size_sectors: 0,
                super_offset_sectors: 8,
                feature_map: 0,
                recovery_offset_sectors: 0,
                events: 10,
                dev_number: 0,
                device_uuid: [0xBB; 16],
                dev_roles: (0..raid_disks as u16).collect(),
            }
        }

        /// Encode with a valid checksum.
        pub(crate) fn encode(&self) -> Vec<u8> {
            let mut buf = vec![0u8; SB_BUF_LEN];
            buf[0..4].copy_from_slice(&MD_MAGIC.to_le_bytes());
            buf[4..8].copy_from_slice(&1u32.to_le_bytes());
            buf[8..12].copy_from_slice(&self.feature_map.to_le_bytes());
            buf[16..32].copy_from_slice(&self.set_uuid);
            buf[32..32 + self.set_name.len()].copy_from_slice(self.set_name.as_bytes());
            buf[72..76].copy_from_slice(&self.level.to_le_bytes());
            buf[76..80].copy_from_slice(&self.layout.to_le_bytes());
            buf[80..88].copy_from_slice(&self.size_sectors.to_le_bytes());
            buf[88..92].copy_from_slice(&self.chunk_sectors.to_le_bytes());
            buf[92..96].copy_from_slice(&self.raid_disks.to_le_bytes());
            buf[128..136].copy_from_slice(&self.data_offset_sectors.to_le_bytes());
            buf[136..144].copy_from_slice(&self.data_size_sectors.to_le_bytes());
            buf[144..152].copy_from_slice(&self.super_offset_sectors.to_le_bytes());
            buf[152..160].copy_from_slice(&self.recovery_offset_sectors.to_le_bytes());
            buf[160..164].copy_from_slice(&self.dev_number.to_le_bytes());
            buf[168..184].copy_from_slice(&self.device_uuid);
            buf[200..208].copy_from_slice(&self.events.to_le_bytes());
            buf[208..216].copy_from_slice(&u64::MAX.to_le_bytes());
            buf[220..224].copy_from_slice(&(self.dev_roles.len() as u32).to_le_bytes());
            for (index, role) in self.dev_roles.iter().enumerate() {
                let at = V1_ROLE_TABLE_OFFSET + index * 2;
                buf[at..at + 2].copy_from_slice(&role.to_le_bytes());
            }
            let end = V1_ROLE_TABLE_OFFSET + self.dev_roles.len() * 2;
            let checksum = v1_checksum(&buf[..end]);
            buf[216..220].copy_from_slice(&checksum.to_le_bytes());
            buf
        }
    }

    /// Fields of a synthetic 0.90 superblock.
    #[derive(Clone)]
    pub(crate) struct V090Fields {
        pub uuid_words: [u32; 4],
        pub level: i32,
        pub layout: u32,
        pub size_kib: u32,
        pub chunk_bytes: u32,
        pub raid_disks: u32,
        pub md_minor: u32,
        pub events: u64,
        pub clean: bool,
        pub this_disk: u32,
        /// `(raid_disk, state)` per descriptor, indexed by disk number.
        pub disks: Vec<(u32, u32)>,
    }

    impl V090Fields {
        pub(crate) fn new(level: i32, raid_disks: u32) -> Self {
            Self {
                uuid_words: [0x0102_0304, 0x0506_0708, 0x090a_0b0c, 0x0d0e_0f10],
                level,
                layout: 0,
                size_kib: 0,
                chunk_bytes: 4096,
                raid_disks,
                md_minor: 0,
                events: 10,
                clean: true,
                this_disk: 0,
                disks: (0..raid_disks)
                    .map(|slot| (slot, DISK_ACTIVE | DISK_SYNC))
                    .collect(),
            }
        }

        pub(crate) fn encode(&self, big_endian: bool) -> Vec<u8> {
            let mut words = vec![0u32; V090_BYTES / 4];
            words[0] = MD_MAGIC;
            words[2] = 90;
            words[5] = self.uuid_words[0];
            words[7] = self.level as u32;
            words[8] = self.size_kib;
            words[9] = self.raid_disks;
            words[10] = self.raid_disks;
            words[11] = self.md_minor;
            words[13] = self.uuid_words[1];
            words[14] = self.uuid_words[2];
            words[15] = self.uuid_words[3];
            words[33] = if self.clean { SB_CLEAN } else { 0 };
            let (first, second) = if big_endian {
                ((self.events >> 32) as u32, self.events as u32)
            } else {
                (self.events as u32, (self.events >> 32) as u32)
            };
            words[39] = first;
            words[40] = second;
            words[64] = self.layout;
            words[65] = self.chunk_bytes;
            for (index, &(role, state)) in self.disks.iter().enumerate() {
                let base = V090_DISKS_WORD + index * V090_DISK_WORDS;
                words[base] = index as u32;
                words[base + 3] = role;
                words[base + 4] = state;
            }
            let this = self.this_disk as usize;
            let (role, state) = self.disks.get(this).copied().unwrap_or((0, 0));
            words[V090_THIS_DISK_WORD] = self.this_disk;
            words[V090_THIS_DISK_WORD + 3] = role;
            words[V090_THIS_DISK_WORD + 4] = state;
            let sum: u64 = words.iter().map(|&word| u64::from(word)).sum();
            words[V090_CHECKSUM_WORD] = ((sum & 0xffff_ffff) + (sum >> 32)) as u32;
            words
                .iter()
                .flat_map(|word| {
                    if big_endian {
                        word.to_be_bytes()
                    } else {
                        word.to_le_bytes()
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::core::evidence::partitions::test_support::BufSource;

    fn raid0_fields() -> V1Fields {
        V1Fields {
            size_sectors: 123_456,
            chunk_sectors: 1024,
            data_offset_sectors: 67_584,
            data_size_sectors: 126_187_520,
            dev_roles: vec![0, 1, ROLE_SPARE, ROLE_FAULTY],
            set_name: "mac:0",
            ..V1Fields::new(0, 2)
        }
    }

    #[test]
    fn decodes_canonical_v1_fields() {
        let sb = parse_v1(&raid0_fields().encode(), "1.2").expect("decode");
        assert_eq!(sb.major_version, 1);
        assert_eq!(sb.set_uuid, [0xAA; 16]);
        assert_eq!(sb.set_name, "mac:0");
        assert_eq!(sb.level, 0);
        assert_eq!(sb.chunksize_sectors, 1024);
        assert_eq!(sb.raid_disks, 2);
        assert_eq!(sb.data_offset_sectors, 67_584);
        assert_eq!(sb.data_size_sectors, 126_187_520);
        assert_eq!(sb.super_offset_sectors, 8);
        assert_eq!(sb.events, 10);
        assert_eq!(sb.resync_offset_sectors, u64::MAX);
        assert_eq!(sb.dev_number, 0);
        assert!(sb.checksum_valid);
    }

    #[test]
    fn checksum_detects_corruption() {
        let mut buf = raid0_fields().encode();
        buf[92] ^= 1;
        let sb = parse_v1(&buf, "1.2").expect("decode");
        assert!(!sb.checksum_valid);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut buf = raid0_fields().encode();
        buf[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_v1(&buf, "1.2").is_none());
    }

    #[test]
    fn formats_uuid_lowercase_hex() {
        let uuid = [
            0x88, 0x0f, 0x20, 0x57, 0x30, 0xe6, 0x4d, 0xa6, 0x5b, 0xfd, 0xb0, 0x6f, 0x70, 0x01,
            0x31, 0xbb,
        ];
        assert_eq!(format_uuid(&uuid), "880f205730e64da65bfdb06f700131bb");
    }

    #[test]
    fn slot_index_resolves_via_role_table() {
        // dev3 took over slot 2 after the original dev2 became a spare.
        let fields = V1Fields {
            dev_number: 3,
            dev_roles: vec![0, 1, ROLE_SPARE, 2],
            ..V1Fields::new(5, 3)
        };
        let sb = parse_v1(&fields.encode(), "1.2").expect("decode");
        assert_eq!(sb.slot_index(), Some(2));
    }

    #[test]
    fn slot_index_treats_spare_faulty_and_journal_as_none() {
        for role in [ROLE_SPARE, ROLE_FAULTY, ROLE_JOURNAL, 7] {
            let fields = V1Fields {
                dev_roles: vec![role, 1],
                ..V1Fields::new(1, 2)
            };
            let sb = parse_v1(&fields.encode(), "1.2").expect("decode");
            assert_eq!(sb.slot_index(), None, "role {role:#x}");
        }
    }

    fn device_with(fields: &V1Fields, size: usize) -> BufSource {
        let mut bytes = vec![0u8; size];
        let offset = fields.super_offset_sectors as usize * 512;
        bytes[offset..offset + SB_BUF_LEN].copy_from_slice(&fields.encode());
        BufSource(bytes)
    }

    #[test]
    fn probe_finds_each_v1_variant_where_it_lives() {
        let size = 512 * 1024;
        let sectors = size as u64 / 512;
        for (version, sector) in [("1.2", 8u64), ("1.1", 0), ("1.0", (sectors - 16) & !7)] {
            let fields = V1Fields {
                super_offset_sectors: sector,
                ..raid0_fields()
            };
            let hit = probe(&device_with(&fields, size))
                .expect("probe")
                .expect("superblock");
            assert_eq!(hit.superblock.version, version);
            assert_eq!(hit.super_offset_bytes, sector * 512);
        }
    }

    /// A superblock recorded for another location (here: a 1.2 superblock of
    /// a device that starts 8 sectors earlier) is not accepted.
    #[test]
    fn probe_rejects_superblocks_that_name_another_location() {
        let fields = V1Fields {
            super_offset_sectors: 16,
            ..raid0_fields()
        };
        let mut bytes = vec![0u8; 512 * 1024];
        bytes[0x1000..0x2000].copy_from_slice(&fields.encode());
        assert!(probe(&BufSource(bytes)).expect("probe").is_none());
    }

    #[test]
    fn probe_returns_none_for_undersized_source() {
        let src = BufSource(vec![0u8; 0x100]);
        assert!(probe(&src).expect("probe").is_none());
    }

    #[test]
    fn decodes_v090_in_both_byte_orders() {
        let size = 1024 * 1024 + 4096;
        let sectors = size as u64 / 512;
        let super_sector = v090_super_sector(sectors).expect("fits");
        assert_eq!(super_sector, (sectors & !127) - 128);
        let fields = V090Fields {
            size_kib: 448,
            events: 0x1_0000_0002,
            md_minor: 3,
            this_disk: 1,
            disks: vec![
                (0, DISK_ACTIVE | DISK_SYNC),
                (1, DISK_ACTIVE | DISK_SYNC),
                (2, DISK_FAULTY),
                (2, DISK_ACTIVE),
            ],
            ..V090Fields::new(5, 3)
        };
        for big_endian in [false, true] {
            let mut bytes = vec![0u8; size];
            let offset = super_sector as usize * 512;
            bytes[offset..offset + 4096].copy_from_slice(&fields.encode(big_endian));
            let hit = probe(&BufSource(bytes))
                .expect("probe")
                .expect("superblock");
            let sb = hit.superblock;
            assert_eq!(sb.version, "0.90");
            assert!(sb.checksum_valid, "big_endian={big_endian}");
            assert_eq!(
                format_uuid(&sb.set_uuid),
                "0102030405060708090a0b0c0d0e0f10"
            );
            assert_eq!(sb.set_name, "md3");
            assert_eq!(sb.level, 5);
            assert_eq!(sb.chunksize_sectors, 8);
            assert_eq!(sb.size_sectors, 896);
            assert_eq!(sb.data_size_sectors, super_sector);
            assert_eq!(sb.events, 0x1_0000_0002);
            assert_eq!(sb.dev_number, 1);
            assert_eq!(sb.slot_index(), Some(1));
            assert_eq!(&sb.dev_roles[..4], &[0, 1, ROLE_FAULTY, ROLE_SPARE]);
            assert_eq!(sb.resync_offset_sectors, u64::MAX);
            assert!(!sb.recovering);
        }
    }

    #[test]
    fn v090_marks_a_member_being_rebuilt() {
        let fields = V090Fields {
            this_disk: 1,
            disks: vec![(0, DISK_ACTIVE | DISK_SYNC), (1, DISK_ACTIVE)],
            ..V090Fields::new(1, 2)
        };
        let sb = parse_v090(&fields.encode(false), 1024).expect("decode");
        assert!(sb.recovering);
        assert_eq!(sb.slot_index(), None);
    }
}
