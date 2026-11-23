//! The decrypted VeraCrypt / TrueCrypt volume header.
//!
//! A 512-byte header sector is a 64-byte salt and 448 bytes encrypted under
//! the header key as one XTS data unit numbered 0. Decrypted, its fields are
//! (offsets into the 448 bytes, big-endian):
//!
//! ```text
//!    0  "VERA" / "TRUE"            44  encrypted area start   u64
//!    4  header version   u16       52  encrypted area length  u64
//!    6  required program u16       60  flags                  u32
//!    8  CRC-32 of [192..448]       64  sector size (v5+)      u32
//!   12  volume / header ctime      188 CRC-32 of [0..188] (v4+)
//!   28  hidden volume size u64     192 master key material (256 bytes)
//!   36  volume size      u64
//! ```

use zeroize::Zeroizing;

/// Length of the salt that precedes the encrypted header.
pub const SALT_LEN: usize = 64;
/// Length of the encrypted header.
pub const HEADER_LEN: usize = 448;
/// Length of a header sector (`salt + header`).
pub const HEADER_SECTOR_LEN: usize = SALT_LEN + HEADER_LEN;
/// Master key material in a header.
pub const MASTER_KEY_LEN: usize = 256;

/// The newest header format VeraCrypt writes.
pub const NEWEST_HEADER_VERSION: u16 = 5;

/// Header flag: system (pre-boot) encryption.
pub const FLAG_SYSTEM: u32 = 0x1;
/// Header flag: non-system in-place encryption created or converted the
/// volume.
pub const FLAG_IN_PLACE: u32 = 0x2;

/// Which program wrote the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// VeraCrypt (`VERA`).
    VeraCrypt,
    /// TrueCrypt (`TRUE`).
    TrueCrypt,
}

impl Flavor {
    /// The product name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Flavor::VeraCrypt => "VeraCrypt",
            Flavor::TrueCrypt => "TrueCrypt",
        }
    }
}

/// The fields of a decrypted, checksum-verified header.
#[derive(Clone)]
pub struct Header {
    /// VeraCrypt or TrueCrypt.
    pub flavor: Flavor,
    /// Header format version.
    pub version: u16,
    /// The oldest program version that opens the volume (`0x010b` is 1.11).
    pub required_program_version: u16,
    /// TrueCrypt's volume creation time (FILETIME); zero in VeraCrypt.
    pub volume_created: u64,
    /// TrueCrypt's header creation time (FILETIME); zero in VeraCrypt.
    pub header_created: u64,
    /// Hidden volume size; non-zero only in a hidden volume's own header.
    pub hidden_volume_size: u64,
    /// Volume size: the data area, or the system partition or drive.
    pub volume_size: u64,
    /// Where the encrypted area starts, from the start of the host.
    pub encrypted_area_start: u64,
    /// How much of the volume is encrypted.
    pub encrypted_area_length: u64,
    /// [`FLAG_SYSTEM`], [`FLAG_IN_PLACE`] …
    pub flags: u32,
    /// The volume's sector size (512 before header version 5).
    pub sector_size: u32,
    /// The primary and secondary master keys of every cipher.
    pub master_keys: Zeroizing<[u8; MASTER_KEY_LEN]>,
}

impl std::fmt::Debug for Header {
    // The master keys stay out of diagnostics.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Header")
            .field("flavor", &self.flavor)
            .field("version", &self.version)
            .field("required_program_version", &self.required_program_version)
            .field("hidden_volume_size", &self.hidden_volume_size)
            .field("volume_size", &self.volume_size)
            .field("encrypted_area_start", &self.encrypted_area_start)
            .field("encrypted_area_length", &self.encrypted_area_length)
            .field("flags", &self.flags)
            .field("sector_size", &self.sector_size)
            .finish_non_exhaustive()
    }
}

/// What a candidate decryption is.
pub(crate) enum Decoded {
    /// A valid header.
    Valid(Box<Header>),
    /// Not a header: wrong key, PRF or cipher.
    NotHeader,
    /// An authentic header (magic and checksums verify) of a format this
    /// reader does not read.
    Unsupported(String),
}

fn be16(data: &[u8], offset: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&data[offset..offset + 2]);
    u16::from_be_bytes(bytes)
}

fn be32(data: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&data[offset..offset + 4]);
    u32::from_be_bytes(bytes)
}

fn be64(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[offset..offset + 8]);
    u64::from_be_bytes(bytes)
}

/// Check a decrypted 448-byte header the way VeraCrypt's `ReadVolumeHeader`
/// does: the magic, the header-field CRC from version 4 on, and the master
/// key CRC always. Format limits are judged only on a header that verified.
pub(crate) fn decode(decrypted: &[u8; HEADER_LEN]) -> Decoded {
    let flavor = match &decrypted[0..4] {
        b"VERA" => Flavor::VeraCrypt,
        b"TRUE" => Flavor::TrueCrypt,
        _ => return Decoded::NotHeader,
    };
    let version = be16(decrypted, 4);
    if version >= 4 && be32(decrypted, 188) != crc32fast::hash(&decrypted[..188]) {
        return Decoded::NotHeader;
    }
    if be32(decrypted, 8) != crc32fast::hash(&decrypted[192..]) {
        return Decoded::NotHeader;
    }
    if version > NEWEST_HEADER_VERSION {
        return Decoded::Unsupported(format!("header version {version}"));
    }
    let sector_size = if version >= 5 {
        be32(decrypted, 64)
    } else {
        512
    };
    if !(512..=4096).contains(&sector_size) || sector_size % 512 != 0 {
        return Decoded::Unsupported(format!("sector size {sector_size}"));
    }
    let mut master_keys = Zeroizing::new([0u8; MASTER_KEY_LEN]);
    master_keys.copy_from_slice(&decrypted[192..]);
    Decoded::Valid(Box::new(Header {
        flavor,
        version,
        required_program_version: be16(decrypted, 6),
        volume_created: be64(decrypted, 12),
        header_created: be64(decrypted, 20),
        hidden_volume_size: be64(decrypted, 28),
        volume_size: be64(decrypted, 36),
        encrypted_area_start: be64(decrypted, 44),
        encrypted_area_length: be64(decrypted, 52),
        flags: be32(decrypted, 60),
        sector_size,
        master_keys,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(magic: &[u8; 4], version: u16, sector: u32) -> [u8; HEADER_LEN] {
        let mut data = [0u8; HEADER_LEN];
        data[0..4].copy_from_slice(magic);
        data[4..6].copy_from_slice(&version.to_be_bytes());
        data[6..8].copy_from_slice(&0x010bu16.to_be_bytes());
        data[28..36].copy_from_slice(&4096u64.to_be_bytes());
        data[36..44].copy_from_slice(&65536u64.to_be_bytes());
        data[44..52].copy_from_slice(&131_072u64.to_be_bytes());
        data[52..60].copy_from_slice(&65536u64.to_be_bytes());
        data[60..64].copy_from_slice(&FLAG_IN_PLACE.to_be_bytes());
        data[64..68].copy_from_slice(&sector.to_be_bytes());
        for (index, byte) in data[192..].iter_mut().enumerate() {
            *byte = index as u8 ^ 0x5a;
        }
        seal(&mut data);
        data
    }

    fn seal(data: &mut [u8; HEADER_LEN]) {
        let keys = crc32fast::hash(&data[192..]);
        data[8..12].copy_from_slice(&keys.to_be_bytes());
        let fields = crc32fast::hash(&data[..188]);
        data[188..192].copy_from_slice(&fields.to_be_bytes());
    }

    fn valid(data: &[u8; HEADER_LEN]) -> Header {
        match decode(data) {
            Decoded::Valid(header) => *header,
            Decoded::NotHeader => panic!("not a header"),
            Decoded::Unsupported(reason) => panic!("unsupported: {reason}"),
        }
    }

    #[test]
    fn fields_decode_for_both_magics() {
        let decoded = valid(&header(b"VERA", 5, 4096));
        assert_eq!(
            (
                decoded.flavor,
                decoded.version,
                decoded.required_program_version,
                decoded.hidden_volume_size,
                decoded.volume_size,
                decoded.encrypted_area_start,
                decoded.encrypted_area_length,
                decoded.flags,
                decoded.sector_size,
                decoded.master_keys[0],
            ),
            (
                Flavor::VeraCrypt,
                5,
                0x010b,
                4096,
                65536,
                131_072,
                65536,
                FLAG_IN_PLACE,
                4096,
                0x5a
            )
        );
        assert_eq!(valid(&header(b"TRUE", 5, 512)).flavor, Flavor::TrueCrypt);
    }

    #[test]
    fn checksums_and_magic_reject_wrong_keys() {
        let mut data = header(b"VERA", 5, 512);
        data[400] ^= 1;
        assert!(matches!(decode(&data), Decoded::NotHeader));
        let mut data = header(b"VERA", 5, 512);
        data[40] ^= 1;
        assert!(matches!(decode(&data), Decoded::NotHeader));
        let mut data = header(b"VERA", 5, 512);
        data[0] = b'X';
        seal(&mut data);
        assert!(matches!(decode(&data), Decoded::NotHeader));
    }

    #[test]
    fn old_headers_skip_the_field_checksum_and_use_512_byte_sectors() {
        let mut data = header(b"TRUE", 3, 4096);
        data[188..192].fill(0);
        let decoded = valid(&data);
        assert_eq!((decoded.version, decoded.sector_size), (3, 512));
    }

    #[test]
    fn newer_versions_and_bad_sector_sizes_are_unsupported() {
        assert!(matches!(
            decode(&header(b"VERA", 6, 512)),
            Decoded::Unsupported(_)
        ));
        assert!(matches!(
            decode(&header(b"VERA", 5, 8192)),
            Decoded::Unsupported(_)
        ));
        assert!(matches!(
            decode(&header(b"VERA", 5, 1000)),
            Decoded::Unsupported(_)
        ));
        assert!(matches!(
            decode(&header(b"VERA", 5, 0)),
            Decoded::Unsupported(_)
        ));
    }

    #[test]
    fn debug_output_omits_master_keys() {
        let text = format!("{:?}", valid(&header(b"VERA", 5, 512)));
        assert!(!text.contains("master"), "{text}");
    }
}
