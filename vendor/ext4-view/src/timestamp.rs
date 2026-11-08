// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimestampError;

/// Timestamps associated with an inode, relative time to EPOCH.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Timestamp {
    seconds: i64,
    nanoseconds: u32,
}

impl Timestamp {
    /// Seconds since EPOCH.
    #[must_use]
    pub fn seconds(self) -> i64 {
        self.seconds
    }

    /// Nanoseconds within the second.
    #[must_use]
    pub fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }

    /// Decode a timestamp from the base field in the classic 128-byte
    /// inode, plus the corresponding `*_extra` field if present.
    pub(crate) fn from_raw(secs: u32, extra: Option<u32>) -> Result<Self, TimestampError> {
        let mut seconds = i64::from(i32::from_le_bytes(secs.to_le_bytes()));
        let mut nanoseconds = 0;

        if let Some(extra) = extra {
            // The lower two bits extend the seconds, the higher 30 bits hold
            // nanoseconds.
            seconds = seconds
                .checked_add(i64::from(extra & 0x3) << 32)
                .ok_or(TimestampError)?;
            nanoseconds = extra >> 2;

            // Ensure nanoseconds are valid.
            if nanoseconds >= 1_000_000_000 {
                return Err(TimestampError);
            }
        }

        Ok(Self {
            seconds,
            nanoseconds,
        })
    }
}
