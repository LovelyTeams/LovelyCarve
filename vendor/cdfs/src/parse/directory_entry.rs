// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::str;

use bitflags::bitflags;
use nom::{
    branch::alt,
    bytes::complete::{tag, take},
    combinator::{map_parser, opt, value},
    multi::length_data,
    number::complete::le_u8,
};
use time::OffsetDateTime;

use super::{
    both_endian::{both_endian16, both_endian32},
    date_time::date_time,
    decode_string,
    susp::SystemUseEntry,
    CharacterEncoding, Result,
};

use crate::error::NomRes;

bitflags! {
    #[derive(Clone, Debug)]
    pub struct FileFlags: u8 {
        const EXISTANCE = 1 << 0;
        const DIRECTORY = 1 << 1;
        const ASSOCIATED_FILE = 1 << 2;
        const RECORD = 1 << 3;
        const PROTECTION = 1 << 4;
        // Bits 5 and 6 are reserved; should be zero
        const MULTIEXTENT = 1 << 7;
    }
}

#[derive(Clone, Debug)]
pub struct DirectoryEntryHeader {
    pub length: u8,
    pub extended_attribute_record_length: u8,
    pub extent_loc: u32,
    pub extent_length: u32,
    pub time: OffsetDateTime,
    pub file_flags: FileFlags,
    pub file_unit_size: u8,
    pub interleave_gap_size: u8,
    pub volume_sequence_number: u16,
    pub character_encoding: CharacterEncoding,
}

impl DirectoryEntryHeader {
    pub(crate) fn parse(
        input: &[u8],
        character_encoding: CharacterEncoding,
    ) -> Result<(DirectoryEntryHeader, String, Option<Vec<SystemUseEntry>>)> {
        Ok(directory_entry(input, character_encoding)?.1)
    }
}

pub(crate) fn directory_entry<'a>(
    i: &'a [u8],
    character_encoding: CharacterEncoding,
) -> NomRes<&[u8], (DirectoryEntryHeader, String, Option<Vec<SystemUseEntry>>)> {
    let orig_len = i.len();
    let (i, length) = le_u8(i)?;
    let (i, extended_attribute_record_length) = le_u8(i)?;
    let (i, extent_loc) = both_endian32(i)?;
    let (i, extent_length) = both_endian32(i)?;
    let (i, time) = date_time(i)?;
    let (i, file_flags) = le_u8(i)?;
    let file_flags = FileFlags::from_bits_truncate(file_flags);
    let (i, file_unit_size) = le_u8(i)?;
    let (i, interleave_gap_size) = le_u8(i)?;
    let (i, volume_sequence_number) = both_endian16(i)?;

    #[cfg(feature = "assertions")]
    {
        assert_eq!(interleave_gap_size, 0);
        assert_eq!(file_unit_size, 0);
    }

    let identifier_len = i.len();
    let (i, identifier) = map_parser(
        length_data(le_u8),
        |bytes: &'a [u8]| -> NomRes<&'a [u8], String> {
            if bytes.len() == 1 {
                alt((
                    value(String::from("\u{0}"), tag(&[0])),
                    value(String::from("\u{1}"), tag(&[1])),
                    decode_string(character_encoding),
                ))(bytes)
            } else {
                decode_string(character_encoding)(bytes)
            }
        },
    )(i)?;
    let identifier_len = identifier_len - i.len();

    // Padding
    let i = if identifier_len % 2 == 0 {
        i
    } else {
        take(1_usize)(i)?.0
    };

    let offset = orig_len - i.len();
    let remainder = usize::from(length) - offset;
    let (i, susp) = map_parser(take(remainder), opt(crate::parse::susp::system_use_entries))(i)?;

    Ok((
        i,
        (
            DirectoryEntryHeader {
                length,
                extended_attribute_record_length,
                extent_loc,
                extent_length,
                time,
                file_flags,
                file_unit_size,
                interleave_gap_size,
                volume_sequence_number,
                character_encoding,
            },
            identifier,
            susp,
        ),
    ))
}
