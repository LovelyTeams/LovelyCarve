// SPDX-License-Identifier: (MIT OR Apache-2.0)

#[allow(unused)]
use log::{debug, error, info, trace, warn};

use nom::number::complete::{be_u16, be_u32, le_u16, le_u32};

use crate::error::NomRes;

// ISO 9660 uses a representation for integers with both little and big endian representations of
// the same number. The Linux kernel only reads the little endian value, with a comment about some
// programs generating invalid ISO with incorrect big endian values.  We read the little endian
// value by default as well, this can be changed to reading the big endian version by enabling the
// `big-endian` feature

pub(crate) fn both_endian16(i: &[u8]) -> NomRes<&[u8], u16> {
    let (i, little_endian) = le_u16(i)?;
    let (i, big_endian) = be_u16(i)?;

    if little_endian != big_endian {
        warn!("16-bit endian mismatch, little={little_endian}, big={big_endian}");
    }

    cfg_if::cfg_if! {
        if #[cfg(feature = "big-endian")] {
            Ok((i, big_endian))
        } else {
            Ok((i, little_endian))
        }
    }
}

pub(crate) fn both_endian32(i: &[u8]) -> NomRes<&[u8], u32> {
    let (i, little_endian) = le_u32(i)?;
    let (i, big_endian) = be_u32(i)?;

    if little_endian != big_endian {
        warn!("32-bit endian mismatch, little={little_endian}, big={big_endian}");
    }

    cfg_if::cfg_if! {
        if #[cfg(feature = "big-endian")] {
            Ok((i, big_endian))
        } else {
            Ok((i, little_endian))
        }
    }
}
