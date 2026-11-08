// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::str;

use nom::{
    branch::alt,
    bytes::complete::{tag, take},
    combinator::{map, map_res, rest, value},
};

use crate::{error::NomRes, Result};

mod both_endian;
mod date_time;

pub(crate) mod directory_entry;
pub(crate) mod susp;
pub(crate) mod volume_descriptor;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CharacterEncoding {
    Iso9660,
    Ucs2Level1,
    Ucs2Level2,
    Ucs2Level3,
}

impl CharacterEncoding {
    pub fn parse(input: &[u8]) -> Result<Self> {
        Ok(character_encoding(input)?.1)
    }
}

pub(crate) fn character_encoding(bytes: &[u8]) -> NomRes<&[u8], CharacterEncoding> {
    // The field is 32 bytes long, and per §8.5.6 ECMA says there can be multiple
    // encodings listed.  But. Really.  For now let's just check for the standard
    // ISO 9660 encoding or UCS-2.

    // Per ECMA 35 / ISO 2022 §13.2.2:
    // I byte 0x25 (02/05) = Designate Other Coding System
    //
    // Per ECMA 35 / ISO 2022 §15.4.2:
    // DOCS with I byte 0x2F (02/15) shall mean it's not really DOCS and we should
    // use the registry as a reference.
    //
    // ISO Registry shows:
    // #162 UCS-2 Level 1 F byte is 0x40 (04/00)
    // #175 UCS-2 Level 2 F byte is 0x43 (04/03)
    // #175 UCS-2 Level 3 F byte is 0x45 (04/05)
    let orig_len = bytes.len();

    let (bytes, encoding) = alt((
        value(CharacterEncoding::Ucs2Level1, tag(&[0x25, 0x2F, 0x40])),
        value(CharacterEncoding::Ucs2Level2, tag(&[0x25, 0x2F, 0x43])),
        value(CharacterEncoding::Ucs2Level3, tag(&[0x25, 0x2F, 0x45])),
        value(CharacterEncoding::Iso9660, tag(&[0_u8; 32])),
    ))(bytes)?;

    let bytes = match orig_len - bytes.len() {
        len if len < 32 => take(32 - len)(bytes)?.0,
        _ => bytes,
    };

    Ok((bytes, encoding))
}

pub(crate) fn decode_string(
    encoding: CharacterEncoding,
) -> impl Fn(&[u8]) -> NomRes<&[u8], String> {
    move |i: &[u8]| match encoding {
        CharacterEncoding::Ucs2Level1
        | CharacterEncoding::Ucs2Level2
        | CharacterEncoding::Ucs2Level3 => map(rest, |input| {
            let (cow, _encoding_used, had_errors) = encoding_rs::UTF_16BE.decode(input);

            // TODO: Real error handling
            #[cfg(feature = "assertions")]
            assert!(!had_errors);

            cow.trim_end().to_string()
        })(i),
        CharacterEncoding::Iso9660 => map(
            map(map_res(rest, str::from_utf8), str::trim_end),
            str::to_string,
        )(i),
    }
}
