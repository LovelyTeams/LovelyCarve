//! Times in the XML: KDBX 4 stores base64 seconds since 0001-01-01 UTC,
//! KDBX 3 stores ISO 8601 text.

use base64::Engine;

/// Seconds from 0001-01-01T00:00:00Z to the Unix epoch.
const EPOCH_OFFSET: i64 = 62_135_596_800;

/// A time as the database states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Timestamp {
    /// An instant, in seconds since 1970-01-01T00:00:00Z.
    Utc(i64),
    /// Text without a zone designator: the writer's wall-clock time in a zone
    /// the file does not state.
    Unzoned(String),
    /// Text that is not a time.
    Invalid(String),
}

impl Timestamp {
    pub(crate) fn parse(text: &str, kdbx4: bool) -> Self {
        let trimmed = text.trim();
        if kdbx4 {
            if let Some(seconds) = binary(trimmed) {
                return Self::Utc(seconds);
            }
        }
        match iso(trimmed) {
            Some(Zone::Utc(seconds)) => Self::Utc(seconds),
            Some(Zone::Unstated) => Self::Unzoned(trimmed.to_owned()),
            None => Self::Invalid(text.to_owned()),
        }
    }

    /// Seconds since the Unix epoch, when the instant is known.
    pub fn unix_seconds(&self) -> Option<i64> {
        match self {
            Self::Utc(seconds) => Some(*seconds),
            Self::Unzoned(_) | Self::Invalid(_) => None,
        }
    }
}

/// KDBX 4 binary time. KeePass pads or cuts other decoded lengths to eight
/// little-endian bytes.
fn binary(text: &str) -> Option<i64> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .ok()
        .filter(|bytes| !bytes.is_empty())?;
    let mut eight = [0u8; 8];
    let length = bytes.len().min(8);
    eight[..length].copy_from_slice(&bytes[..length]);
    i64::from_le_bytes(eight).checked_sub(EPOCH_OFFSET)
}

enum Zone {
    Utc(i64),
    Unstated,
}

fn digits(text: &str, start: usize, length: usize) -> Option<i64> {
    let part = text.get(start..start + length)?;
    if !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM|±HHMM]`.
fn iso(text: &str) -> Option<Zone> {
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = digits(text, 0, 4)?;
    let month = digits(text, 5, 2)?;
    let day = digits(text, 8, 2)?;
    let hour = digits(text, 11, 2)?;
    let minute = digits(text, 14, 2)?;
    let second = digits(text, 17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 {
            return None;
        }
        rest = &fraction[length..];
    }
    let offset = match rest {
        "" => return Some(Zone::Unstated),
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = &rest[1..];
            let (hours, minutes) = match zone.len() {
                5 if zone.as_bytes()[2] == b':' => (digits(zone, 0, 2)?, digits(zone, 3, 2)?),
                4 => (digits(zone, 0, 2)?, digits(zone, 2, 2)?),
                2 => (digits(zone, 0, 2)?, 0),
                _ => return None,
            };
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(Zone::Utc(seconds - offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_binary_and_text_times() {
        // 2018-06-10T11:04:01Z is 63664225441 seconds after 0001-01-01.
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(63_664_225_441i64.to_le_bytes());
        assert_eq!(
            Timestamp::parse(&encoded, true),
            Timestamp::Utc(1_528_628_641)
        );
        assert_eq!(
            Timestamp::parse("2018-06-10T11:04:01Z", false),
            Timestamp::Utc(1_528_628_641)
        );
        assert_eq!(
            Timestamp::parse("2018-06-10T13:04:01.5+02:00", false),
            Timestamp::Utc(1_528_628_641)
        );
        // A KDBX 4 file may still carry text times.
        assert_eq!(
            Timestamp::parse("2018-06-10T11:04:01Z", true),
            Timestamp::Utc(1_528_628_641)
        );
        assert_eq!(
            Timestamp::parse("2018-06-10T11:04:01", false),
            Timestamp::Unzoned("2018-06-10T11:04:01".into())
        );
        assert_eq!(
            Timestamp::parse("2018-02-30T11:04:01Z", false),
            Timestamp::Invalid("2018-02-30T11:04:01Z".into())
        );
        assert_eq!(
            Timestamp::parse("", true),
            Timestamp::Invalid(String::new())
        );
        assert_eq!(
            Timestamp::parse("0001-01-01T00:00:00Z", false),
            Timestamp::Utc(-EPOCH_OFFSET)
        );
        assert_eq!(
            Timestamp::parse("9999-12-31T23:59:59Z", false),
            Timestamp::Utc(253_402_300_799)
        );
    }
}
