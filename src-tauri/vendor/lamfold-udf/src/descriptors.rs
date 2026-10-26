//! ECMA-167 tags, OSTA names and timestamps. No native layouts or unchecked casts.
use alloc::string::String;
use lamfold::{BlockSource, FoldError, Result};

pub(crate) fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(FoldError::Corrupt("udf: short u16"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(FoldError::Corrupt("udf: short u32"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}
pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from(u32_at(bytes, offset)?) | (u64::from(u32_at(bytes, offset + 4)?) << 32))
}
pub(crate) fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "udf: source range/truncation",
    )?;
    source.read_at(offset, bytes)
}
pub(crate) fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}
/// Validate the tag and all bytes used by this parser, including the CRC's coverage.
pub(crate) fn descriptor(bytes: &[u8], location: u32, minimum_end: usize) -> Result<u16> {
    let header = bytes
        .get(..16)
        .ok_or(FoldError::Corrupt("udf: short descriptor tag"))?;
    require(
        matches!(u16_at(header, 2)?, 2 | 3) && header[5] == 0,
        "udf: descriptor version/reserved byte",
    )?;
    let sum = header
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 4)
        .fold(0u8, |sum, (_, byte)| sum.wrapping_add(*byte));
    require(sum == header[4], "udf: descriptor tag checksum")?;
    require(
        u32_at(header, 12)? == location,
        "udf: descriptor tag location",
    )?;
    let end = 16 + usize::from(u16_at(header, 10)?);
    require(
        end >= minimum_end,
        "udf: descriptor CRC does not cover fields",
    )?;
    let body = bytes
        .get(16..end)
        .ok_or(FoldError::Corrupt("udf: descriptor CRC length"))?;
    require(
        crc16(body) == u16_at(header, 8)?,
        "udf: descriptor body CRC",
    )?;
    u16_at(header, 0)
}

/// Physical block geometry candidates; UDF logical block size must match the medium.
pub const BLOCK_SIZES: [u32; 4] = [2048, 512, 1024, 4096];
pub fn anchor_locations(length: u64, block_size: u32) -> [u64; 3] {
    let last = length.checked_div(u64::from(block_size)).unwrap_or(0);
    [256, last.saturating_sub(1), last.saturating_sub(257)]
}
/// Bounded anchor validation shared by the host signature probe and full reader.
pub fn valid_anchor(bytes: &[u8], location: u32) -> bool {
    descriptor(bytes, location, 512).is_ok_and(|tag| tag == 2)
}

pub(crate) fn domain(bytes: &[u8]) -> Result<()> {
    require(
        bytes.get(1..24) == Some(b"*OSTA UDF Compliant\0\0\0\0"),
        "udf: unknown domain identifier",
    )?;
    let revision = u16_at(bytes, 24)?;
    if !matches!(revision, 0x0102 | 0x0150 | 0x0200 | 0x0201) {
        return Err(FoldError::Unsupported(
            "udf: revision outside 1.02–2.01 physical-partition profile",
        ));
    }
    Ok(())
}

pub(crate) fn decode_name(bytes: &[u8]) -> Result<String> {
    match bytes.split_first() {
        Some((8, rest)) => Ok(rest.iter().map(|byte| char::from(*byte)).collect()),
        Some((16, rest)) => {
            require(
                rest.len().is_multiple_of(2),
                "udf: odd compressed Unicode name",
            )?;
            let units = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair));
            char::decode_utf16(units)
                .collect::<core::result::Result<String, _>>()
                .map_err(|_| FoldError::Corrupt("udf: invalid Unicode name"))
        }
        _ => Err(FoldError::Unsupported("udf: name compression identifier")),
    }
}
pub(crate) fn safe_component(name: &str) -> Result<()> {
    require(
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']),
        "udf: unsafe path component",
    )
}

/// Convert only timestamps whose recorded timezone is known to UTC. The UDF
/// unknown-offset sentinel (-2047 minutes) stays unknown, never labelled UTC.
pub(crate) fn timestamp(bytes: &[u8]) -> Result<Option<(i64, u32)>> {
    let bytes = bytes
        .get(..12)
        .ok_or(FoldError::Corrupt("udf: short timestamp"))?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let zone = u16_at(bytes, 0)?;
    if zone >> 12 != 1 {
        return Err(FoldError::Unsupported("udf: non-local timestamp type"));
    }
    let minutes = ((zone << 4) as i16) >> 4;
    if minutes == -2047 {
        return Ok(None);
    }
    require((-1440..=1440).contains(&minutes), "udf: timestamp timezone")?;
    let year = i64::from(u16_at(bytes, 2)?);
    let month = i64::from(bytes[4]);
    let day = i64::from(bytes[5]);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    require(
        year >= 1
            && day >= 1
            && day <= days
            && bytes[6] < 24
            && bytes[7] < 60
            && bytes[8] < 60
            && bytes[9..].iter().all(|value| *value < 100),
        "udf: invalid calendar time",
    )?;
    // Proleptic Gregorian civil date to Unix days (400-year eras).
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let seconds = (era * 146097 + day_of_era - 719468) * 86400
        + i64::from(bytes[6]) * 3600
        + i64::from(bytes[7]) * 60
        + i64::from(bytes[8])
        - i64::from(minutes) * 60;
    let nanos = u32::from(bytes[9]) * 10_000_000
        + u32::from(bytes[10]) * 100_000
        + u32::from(bytes[11]) * 1000;
    Ok(Some((seconds, nanos)))
}
