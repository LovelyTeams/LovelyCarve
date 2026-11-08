pub struct Timestamps {
    created: Timestamp,
    modified: Timestamp,
    accessed: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_offset_is_signed_and_can_be_unknown() {
        for (raw, expected) in [
            (0, None),
            (0x20, None),
            (0x80, Some(0)),
            (0xa0, Some(32)),
            (0xec, Some(-20)),
            (0xc0, Some(-64)),
        ] {
            assert_eq!(Timestamp::from_raw(1, 0, raw).offset(), expected);
        }
    }

    #[test]
    fn ten_millisecond_increment_keeps_seconds_and_fraction() {
        let timestamp = Timestamp::from_raw(3, 199, 0x80);
        assert_eq!(
            (timestamp.time().second, timestamp.milliseconds()),
            (7, 990)
        );
        assert!(!Timestamp::from_raw(3, 200, 0x80).is_valid());
        assert!(!Timestamp::from_raw(0, 0, 0x80).is_valid());
    }
}

impl Timestamps {
    pub fn new(created: Timestamp, modified: Timestamp, accessed: Timestamp) -> Self {
        Timestamps {
            created,
            modified,
            accessed,
        }
    }

    pub fn created(&self) -> &Timestamp {
        &self.created
    }

    pub fn modified(&self) -> &Timestamp {
        &self.modified
    }

    pub fn accessed(&self) -> &Timestamp {
        &self.accessed
    }
}

pub struct Timestamp {
    timestamp: u32,
    ms_increment: u8,
    // Offset from UTC in 15 minute intervals
    utc_offset: Option<i8>,
}

pub struct Date {
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

// See https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification#748-timestamp-fields
impl Timestamp {
    pub fn new(timestamp: u32, ms_increment: u8, utc_offset: i8) -> Self {
        Timestamp {
            timestamp,
            ms_increment,
            utc_offset: Some(utc_offset),
        }
    }

    /// Decode OffsetValid and the signed seven-bit count of 15-minute intervals.
    pub fn from_raw(timestamp: u32, increment: u8, raw_offset: u8) -> Self {
        Self {
            timestamp,
            ms_increment: increment,
            utc_offset: (raw_offset & 0x80 != 0).then_some(((raw_offset << 1) as i8) >> 1),
        }
    }

    pub fn offset(&self) -> Option<i8> {
        self.utc_offset
    }
    pub fn milliseconds(&self) -> u16 {
        u16::from(self.ms_increment % 100) * 10
    }
    pub fn is_valid(&self) -> bool {
        self.timestamp != 0 && self.ms_increment <= 199
    }

    pub fn date(&self) -> Date {
        Date {
            day: ((self.timestamp >> 16) & 0x1F) as u8,
            month: ((self.timestamp >> 21) & 0xF) as u8,
            year: 1980 + ((self.timestamp >> 25) & 0x7F) as u16,
        }
    }

    pub fn time(&self) -> Time {
        Time {
            second: (self.ms_increment / 100) + (self.timestamp & 0x1F) as u8 * 2,
            minute: ((self.timestamp >> 5) & 0x3f) as u8,
            hour: ((self.timestamp >> 11) & 0x1F) as u8,
        }
    }

    pub fn utc_offset(&self) -> i8 {
        self.utc_offset.unwrap_or(0)
    }
}
