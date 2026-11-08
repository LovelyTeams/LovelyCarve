// SPDX-License-Identifier: (MIT OR Apache-2.0)

#[allow(unused)]
use log::{debug, error, info, trace, warn};

use std::{convert::TryInto, str};
use time::OffsetDateTime;

use bitflags::bitflags;
use nom::error::ParseError;
use nom::{
    branch::alt,
    bytes::complete::{tag, take},
    combinator::{map, map_res, opt, rest, value},
    multi::{length_data, many1},
    number::complete::le_u8,
};

use super::{both_endian::both_endian32, date_time::date_time};
use crate::error::NomRes;

trait ParseSusp<'a> {
    const SIGNATURE: Option<&'static [u8; 2]>;

    fn parse(input: &'a [u8]) -> NomRes<&'a [u8], Self>
    where
        Self: Sized,
    {
        let (input, (sig, _length, version, data)) = Self::parse_sig(input)?;
        let (_, entry) = Self::parse_data(data, sig.try_into().unwrap(), version)?;
        Ok((input, entry))
    }

    fn parse_sig(input: &'a [u8]) -> NomRes<&'a [u8], (&'a [u8], u8, u8, &'a [u8])> {
        let (input, sig) = match Self::SIGNATURE {
            Some(signature) => tag(signature)(input)?,
            None => take(2_usize)(input)?,
        };

        let (input, length) = le_u8(input)?;
        let (input, version) = le_u8(input)?;

        if length == 0 {
            Err(nom::Err::Error(crate::error::OurNomError::from_error_kind(
                input,
                nom::error::ErrorKind::Fail,
            )))?
        }
        let remainder = usize::from(length) - 4;
        let (input, data) = take(remainder)(input)?;

        Ok((input, (sig, length, version, data)))
    }

    fn parse_data(input: &'a [u8], sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self>
    where
        Self: Sized;
}

/// System Use extensions registered as being used in a directory hierarchy
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SuspExtension {
    /// Rock Ridge v1.9 and v1.10
    RockRidge1_09,

    /// IEEE spec Rock Ridge extensions
    RockRidge1_12,
}

/// System Use Sharing Protocol (SUSP) entries.  SUSP specifies a method of storing additional data in the [`DirectoryEntry`] structure.
#[derive(Clone, Debug)]
pub(crate) enum SystemUseEntry {
    // CE
    ContinuationArea(ContinuationArea),

    // ER
    ExtensionsReference(ExtensionsReference),

    // SP
    SuspIndicator(SuspIndicator),

    // ST
    // SuspTerminator(SuspTerminator),

    // Rock Ridge variants

    // NM
    AlternateName(AlternateName),

    // PX
    PosixAttributes(PosixAttributes),

    // TF
    PosixTimestamp(PosixTimestamp),

    // RR
    RockRidge(RockRidge),

    // SL
    SymbolicLink(SymbolicLink),

    // CL
    ChildLink(ChildLink),

    // RE
    RelocatedDirectory(RelocatedDirectory),

    // Catch-all
    Unknown(Unknown),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AlternateName {
    pub name: String,
    pub flags: AlternateNameFlags,
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    pub struct AlternateNameFlags: u8 {
        const CONTINUE = 1 << 0;
        const CURRENT = 1 << 1;
        const PARENT = 1 << 2;
        // 3
        // 4
        const HOST = 1 << 5;
        // 6
        // 7
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChildLink(pub u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContinuationArea {
    pub block_location: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Clone, Debug)]
pub struct ExtensionsReference {
    pub extensions: Vec<SuspExtension>,
}

/// Timestamps from a `TF` entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PosixTimestamp {
    /// POSIX.1 `st_atime`
    pub access: Option<OffsetDateTime>,

    /// POSIX.1 `st_ctime`
    pub attributes: Option<OffsetDateTime>,

    /// The "backup" time.  Its use is essentially undefined.
    pub backup: Option<OffsetDateTime>,

    /// ISO 9660 / ECMA-119 § 9.5.4
    pub creation: Option<OffsetDateTime>,

    /// ISO 9660 / ECMA-119 § 9.5.7
    pub effective: Option<OffsetDateTime>,

    /// ISO 9660 / ECMA-119 § 9.5.6
    pub expiration: Option<OffsetDateTime>,

    /// POSIX.1 `st_mtime`, ISO 9660 / ECMA-119 § 9.5.5
    pub modify: Option<OffsetDateTime>,
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    struct PosixTimestampFlags: u8 {
        const CREATION = 1 << 0;
        const MODIFY = 1 << 1;
        const ACCESS = 1 << 2;
        const ATTRIBUTES = 1 << 3;
        const BACKUP = 1 << 4;
        const EXPIRATION = 1 << 5;
        const EFFECTIVE = 1 << 6;
        const LONG_FORM = 1 << 7;
    }
}

bitflags! {
    /// The mode from a `PX` entry.  Equivalent to POSIX.1's `st_mode` field.
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub struct PosixFileMode: u32 {
        /// Directory entry is an `AF_LOCAL` (née `AF_UNIX`) socket.  Equivalent to `S_IFSOCK`.
        const TYPE_SOCKET    = 0o0140000;

        /// Directory entry is a symbolic link.  This indicates an `SL` entry should also be present.  Equivalent to X/Open System Interfaces (XSI) `S_IFLNK` and POSIX.1 `S_ISSOCK()`.
        const TYPE_SYMLINK   = 0o0120000;

        /// Directory entry is a regular file.  Equivalent to X/Open System Interfaces (XSI) `S_IFREG` and POSIX.1 `S_ISREG()`.
        const TYPE_FILE      = 0o0100000;

        /// Directory entry is a block device.  Equivalent to X/Open System Interfaces (XSI) `S_IFBLK` and POSIX.1 `S_ISBLK()`.
        const TYPE_BLOCK_DEV = 0o0060000;

        /// Directory entry is a directory.  Equivalent to X/Open System Interfaces (XSI) `S_IFDIR` and POSIX.1 `S_ISDIR()`.
        const TYPE_DIRECTORY = 0o0040000;

        /// Directory entry is a character device.  Equivalent to X/Open System Interfaces (XSI) `S_IFCHR` and POSIX.1 `S_ISCHR()`.
        const TYPE_CHAR_DEV  = 0o0020000;

        /// Directory entry is a named pipe.  Equivalent to X/Open System Interfaces (XSI) `S_IFIFO` and POSIX.1 `S_ISFIFO()`.
        const TYPE_PIPE      = 0o0010000;

        /// If executed, will be run with the UID of the file's owner.  Equivalent to POSIX.1 `S_ISUID`.
        const SET_UID        = 0o0004000;

        /// If executed, will be run with the GID of the file's owner.  Equivalent to POSIX.1 `S_ISGID`.
        const SET_GID        = 0o0002000;

        /// This needs to go away.
        const LOCKABLE       = 0o0002000;

        /// Sticky bit, or lots of legacy cruft.  Your choice.  Equivalent to X/Open System Interfaces (XSI) `S_ISVTX` and BSD `S_ISTXT`.
        const STICKY         = 0o0001000;

        /// Equivalent to POSIX.1 `S_IRUSR`.
        const OWN_READ       = 0o0000400;

        /// Equivalent to POSIX.1 `S_IWUSR`.
        const OWN_WRITE      = 0o0000200;

        /// Equivalent to POSIX.1 `S_IXUSR`.
        const OWN_EXEC       = 0o0000100;

        /// Equivalent to POSIX.1 `S_IRGRP`.
        const GROUP_READ     = 0o0000040;

        /// Equivalent to POSIX.1 `S_IWGRP`.
        const GROUP_WRITE    = 0o0000020;

        /// Equivalent to POSIX.1 `S_IXGRP`.
        const GROUP_EXEC     = 0o0000010;

        /// Equivalent to POSIX.1 `S_IROTH`.
        const WORLD_READ     = 0o0000004;

        /// Equivalent to POSIX.1 `S_IWOTH`.
        const WORLD_WRITE    = 0o0000002;

        /// Equivalent to POSIX.1 `S_IXOTH`.
        const WORLD_EXEC     = 0o0000001;

        /// Equivalent to POSIX.1 `S_ISUID | S_ISGID | S_IRWXU | S_IRWXG | S_IRWXO`
        const ALL_PERMISSIONS = Self::OWN_READ.bits() | Self::OWN_WRITE.bits() | Self::OWN_EXEC.bits() | Self::SET_UID.bits() |
                                Self::GROUP_READ.bits() | Self::GROUP_WRITE.bits() | Self::GROUP_EXEC.bits() | Self::SET_GID.bits() |
                                Self::WORLD_READ.bits() | Self::WORLD_WRITE.bits() | Self::WORLD_EXEC.bits();
    }
}

/// POSIX.1 attributes from a `PX` entry.
///
/// This struct contains the permissions, ownership, links, and inode attributes generated from a
/// `PX` entry in the system use table.
///
/// ## See Also
///
/// System Use Sharing Protocol § 4.1.1
#[derive(Clone, Debug, PartialEq)]
pub struct PosixAttributes {
    /// Equivalent to the POSIX.1 `st_mode` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub mode: PosixFileMode,

    /// Equivalent to the POSIX.1 `st_nlink` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub links: u32,

    /// Equivalent to the POSIX.1 `st_uid` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub uid: u32,

    /// Equivalent to the POSIX.1 `st_gid` field.
    ///
    /// ## See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    pub gid: u32,

    /// Serial number. Equivalent to the POSIX.1 `st_ino` field.
    ///
    /// ## Notes
    ///
    /// This was introduced in Rock Ridge v1.12 and may not be present.
    pub inode: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelocatedDirectory(bool);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RockRidge {
    pub flags: RockRidgeFlags,
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct RockRidgeFlags: u8 {
        const PX = 1 << 0;
        const PN = 1 << 1;
        const SL = 1 << 2;
        const NM = 1 << 3;
        const CL = 1 << 4;
        const PL = 1 << 5;
        const RE = 1 << 6;
        const TF = 1 << 7;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SuspIndicator {
    pub skip: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolicLink {
    pub should_continue: bool,
    pub records: Vec<SymbolicLinkRecord>,
}

bitflags! {
    #[derive(Clone, Debug, PartialEq)]
    pub struct SymbolicLinkRecordFlags: u8 {
        const CONTINUE = 1 << 0;
        const CURRENT = 1 << 1;
        const PARENT = 1 << 2;
        const ROOT = 1 << 3;
        const VOLUME_ROOT = 1 << 4;
        const HOSTNAME = 1 << 5;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolicLinkRecord {
    pub flags: SymbolicLinkRecordFlags,
    pub component: String,
}

#[derive(Clone, PartialEq)]
pub struct Unknown {
    pub sig: [u8; 2],
    pub version: u8,
    pub data: Vec<u8>,
}

impl<'a> ParseSusp<'a> for AlternateName {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'N', b'M']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, flags) = map(le_u8, AlternateNameFlags::from_bits_truncate)(input)?;
        let (input, name) = map(
            map(map_res(rest, std::str::from_utf8), |s| {
                s.trim_matches('\u{0}')
            }),
            String::from,
        )(input)?;

        Ok((input, Self { flags, name }))
    }
}

impl<'a> ParseSusp<'a> for ChildLink {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'C', b'L']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, lba) = both_endian32(input)?;

        Ok((input, Self(lba)))
    }
}

impl<'a> ParseSusp<'a> for ContinuationArea {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'C', b'E']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
            assert_eq!(input.len(), 24);
        }

        let (input, block_location) = both_endian32(input)?;
        let (input, offset) = both_endian32(input)?;
        let (input, length) = both_endian32(input)?;

        Ok((
            input,
            Self {
                block_location,
                offset,
                length,
            },
        ))
    }
}

fn susp_extension(input: &[u8]) -> NomRes<&[u8], SuspExtension> {
    let (input, id_len) = le_u8(input)?;
    let (input, description_len) = le_u8(input)?;
    let (input, source_len) = le_u8(input)?;
    let (input, version) = le_u8(input)?;

    let (input, id) = map(
        map(
            map_res(take(usize::from(id_len)), str::from_utf8),
            str::trim_end,
        ),
        str::to_string,
    )(input)?;
    let (input, description) = map(
        map(
            map_res(take(usize::from(description_len)), str::from_utf8),
            str::trim_end,
        ),
        str::to_string,
    )(input)?;
    let (input, source) = map(
        map(
            map_res(take(usize::from(source_len)), str::from_utf8),
            str::trim_end,
        ),
        str::to_string,
    )(input)?;

    let extension = match (id.as_ref(), version) {
        ("RRIP_1991A", 1) => SuspExtension::RockRidge1_09,
        ("IEEE_P1282", 1) => SuspExtension::RockRidge1_12,
        _ => unimplemented!(
            "Unknown extension {id:?}, description={description:?}, source={source:?}"
        ),
    };

    Ok((input, extension))
}

impl<'a> ParseSusp<'a> for ExtensionsReference {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'E', b'R']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, extensions) = many1(susp_extension)(input)?;

        Ok((input, Self { extensions }))
    }
}

impl From<PosixFileMode> for u16 {
    fn from(mode: PosixFileMode) -> Self {
        mode.intersection(PosixFileMode::ALL_PERMISSIONS)
            .bits()
            .try_into()
            .expect("The mask is u16, we should never be here.")
    }
}

impl std::fmt::Display for PosixFileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        if self.contains(Self::TYPE_SOCKET) {
            write!(f, "s")?;
        } else if self.contains(Self::TYPE_SYMLINK) {
            write!(f, "l")?;
        } else if self.contains(Self::TYPE_FILE) {
            write!(f, "-")?;
        } else if self.contains(Self::TYPE_BLOCK_DEV) {
            write!(f, "b")?;
        } else if self.contains(Self::TYPE_DIRECTORY) {
            write!(f, "d")?;
        } else if self.contains(Self::TYPE_CHAR_DEV) {
            write!(f, "c")?;
        } else if self.contains(Self::TYPE_PIPE) {
            write!(f, "p")?;
        } else {
            cfg_if::cfg_if! {
                if #[cfg(feature = "assertions")] {
                    unimplemented!("Unknown file type: {self:?}");
                } else {
                    write!(f, "?")?;
                }
            }
        }

        if self.contains(Self::OWN_READ) {
            write!(f, "r")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::OWN_WRITE) {
            write!(f, "w")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::OWN_EXEC) {
            if self.contains(Self::SET_UID) {
                write!(f, "S")?;
            } else {
                write!(f, "x")?;
            }
        } else if self.contains(Self::SET_UID) {
            write!(f, "s")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::GROUP_READ) {
            write!(f, "r")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::GROUP_WRITE) {
            write!(f, "w")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::GROUP_EXEC) {
            if self.contains(Self::SET_GID) {
                write!(f, "S")?;
            } else {
                write!(f, "x")?;
            }
        } else if self.contains(Self::SET_GID) {
            write!(f, "s")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::WORLD_READ) {
            write!(f, "r")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::WORLD_WRITE) {
            write!(f, "w")?;
        } else {
            write!(f, "-")?;
        }

        if self.contains(Self::WORLD_EXEC) {
            write!(f, "x")?;
        } else {
            write!(f, "-")?;
        }

        Ok(())
    }
}

impl<'a> ParseSusp<'a> for PosixTimestamp {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'T', b'F']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, flags) = map(le_u8, PosixTimestampFlags::from_bits_truncate)(input)?;

        let (input, timestamps) = many1(date_time)(input)?;

        let mut it = timestamps.into_iter();

        let creation = match flags.contains(PosixTimestampFlags::CREATION) {
            true => it.next(),
            false => None,
        };

        let modify = match flags.contains(PosixTimestampFlags::MODIFY) {
            true => it.next(),
            false => None,
        };

        let access = match flags.contains(PosixTimestampFlags::ACCESS) {
            true => it.next(),
            false => None,
        };

        let attributes = match flags.contains(PosixTimestampFlags::ATTRIBUTES) {
            true => it.next(),
            false => None,
        };

        let backup = match flags.contains(PosixTimestampFlags::BACKUP) {
            true => it.next(),
            false => None,
        };

        let expiration = match flags.contains(PosixTimestampFlags::EXPIRATION) {
            true => it.next(),
            false => None,
        };

        let effective = match flags.contains(PosixTimestampFlags::EFFECTIVE) {
            true => it.next(),
            false => None,
        };

        let ret = Self {
            creation,
            modify,
            access,
            attributes,
            backup,
            expiration,
            effective,
        };
        trace!("{flags:?} {ret:?}");

        Ok((input, ret))
    }
}

impl<'a> ParseSusp<'a> for PosixAttributes {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'P', b'X']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);

            // Between v1.09 and v1.12 the spec added a field. What's the point of even versioning this shit?
            // Meanwhile mksiofs writes the `RR` entry (which was dropped after v1.09) with a v1.12 sized attribute field…
            assert!(input.len() == 32 || input.len() == 40);
        }

        let (input, mode) = map(both_endian32, PosixFileMode::from_bits_truncate)(input)?;
        let (input, links) = both_endian32(input)?;
        let (input, uid) = both_endian32(input)?;
        let (input, gid) = both_endian32(input)?;
        let (input, inode) = opt(both_endian32)(input)?;

        Ok((
            input,
            Self {
                mode,
                links,
                uid,
                gid,
                inode,
            },
        ))
    }
}

impl<'a> ParseSusp<'a> for RelocatedDirectory {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'R', b'E']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
            assert_eq!(input.len(), 0);
        }

        Ok((input, Self(true)))
    }
}

impl<'a> ParseSusp<'a> for RockRidge {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'R', b'R']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(input.len(), 1);
            assert_eq!(version, 1);
        }

        let (input, flags) = map(le_u8, RockRidgeFlags::from_bits_truncate)(input)?;

        Ok((input, Self { flags }))
    }
}

impl<'a> ParseSusp<'a> for SuspIndicator {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'S', b'P']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, _cow) = tag(&[0xBE, 0xEF])(input)?;
        let (input, skip) = le_u8(input)?;
        let ret = Self { skip };

        Ok((input, ret))
    }
}

impl SymbolicLinkRecord {
    fn parse(input: &[u8]) -> NomRes<&[u8], Self> {
        let (input, flags) = map(le_u8, SymbolicLinkRecordFlags::from_bits_truncate)(input)?;
        let (input, component) =
            map(map_res(length_data(le_u8), str::from_utf8), String::from)(input)?;

        Ok((input, Self { flags, component }))
    }
}

impl<'a> ParseSusp<'a> for SymbolicLink {
    const SIGNATURE: Option<&'static [u8; 2]> = Some(&[b'S', b'L']);

    fn parse_data(input: &'a [u8], _sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        #[cfg(feature = "assertions")]
        {
            assert_eq!(Self::SIGNATURE.unwrap(), _sig);
            assert_eq!(version, 1);
        }

        let (input, should_continue) =
            alt((value(true, tag(b"\x01")), value(false, tag(b"\x00"))))(input)?;

        let (input, records) = many1(SymbolicLinkRecord::parse)(input)?;

        Ok((
            input,
            Self {
                should_continue,
                records,
            },
        ))
    }
}

impl std::fmt::Debug for Unknown {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut d = fmt.debug_struct("Unknown");

        let sig0 = char::from(self.sig[0]);
        let sig1 = char::from(self.sig[1]);

        let d = if sig0.is_ascii() && sig1.is_ascii() {
            let sig = format!("{sig0}{sig1}");
            d.field("sig", &sig)
        } else {
            d.field("sig", &self.sig)
        };
        let d = d.field("version", &self.version);
        let d = d.field("data", &self.data);

        d.finish()
    }
}

impl<'a> ParseSusp<'a> for Unknown {
    const SIGNATURE: Option<&'static [u8; 2]> = None;

    fn parse_data(input: &'a [u8], sig: &'a [u8; 2], version: u8) -> NomRes<&'a [u8], Self> {
        let ret = Self {
            sig: *sig,
            data: input.to_vec(),
            version,
        };

        Ok((input, ret))
    }
}

pub(crate) fn system_use_entries(input: &[u8]) -> NomRes<&[u8], Vec<SystemUseEntry>> {
    let (input, entries) = many1(alt((
        map(SuspIndicator::parse, SystemUseEntry::SuspIndicator),
        map(ContinuationArea::parse, SystemUseEntry::ContinuationArea),
        map(
            ExtensionsReference::parse,
            SystemUseEntry::ExtensionsReference,
        ),
        map(PosixTimestamp::parse, SystemUseEntry::PosixTimestamp),
        map(AlternateName::parse, SystemUseEntry::AlternateName),
        map(PosixAttributes::parse, SystemUseEntry::PosixAttributes),
        map(RockRidge::parse, SystemUseEntry::RockRidge),
        map(ChildLink::parse, SystemUseEntry::ChildLink),
        map(
            RelocatedDirectory::parse,
            SystemUseEntry::RelocatedDirectory,
        ),
        map(SymbolicLink::parse, SystemUseEntry::SymbolicLink),
        map(Unknown::parse, SystemUseEntry::Unknown),
    )))(input)?;

    // eprintln!("suspremainder={}", input.len());
    Ok((input, entries))
}
