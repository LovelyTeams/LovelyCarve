//! Metadata that is stored as bits and needs decoding: the protection
//! longword, and the `DateStamp`'s calendar conversion.
//!
//! Both are places where "it's just a number" costs correctness. The
//! protection long's low nibble reads *backwards* from every other
//! permission system anyone has met since; the date's epoch is 1978 and
//! its leap rules are the platform's rather than the calendar's. Neither
//! belongs in a consumer, guessed at once per consumer.

use core::fmt;

use crate::read::DateStamp;

// ---------------------------------------------------------------------------
// Protection bits
// ---------------------------------------------------------------------------

/// Bit 0, `FIBF_DELETE`: **set means deletion is denied.**
pub const FIBF_DELETE: u32 = 0x0000_0001;
/// Bit 1, `FIBF_EXECUTE`: **set means execution is denied.**
pub const FIBF_EXECUTE: u32 = 0x0000_0002;
/// Bit 2, `FIBF_WRITE`: **set means writing is denied.**
pub const FIBF_WRITE: u32 = 0x0000_0004;
/// Bit 3, `FIBF_READ`: **set means reading is denied.**
pub const FIBF_READ: u32 = 0x0000_0008;

/// Bit 4, `FIBF_ARCHIVE`: the file has been backed up since last change.
pub const FIBF_ARCHIVE: u32 = 0x0000_0010;
/// Bit 5, `FIBF_PURE`: the executable is re-entrant and may be made
/// resident.
pub const FIBF_PURE: u32 = 0x0000_0020;
/// Bit 6, `FIBF_SCRIPT`: the file is a Shell script, runnable without
/// `Execute`.
pub const FIBF_SCRIPT: u32 = 0x0000_0040;
/// Bit 7, `FIBF_HIDDEN`: hide from directory listings. Honoured by
/// almost nothing, stored by everything.
pub const FIBF_HIDDEN: u32 = 0x0000_0080;

/// Bit 8, `FIBF_GRP_DELETE`: **set means the group may delete.**
pub const FIBF_GRP_DELETE: u32 = 0x0000_0100;
/// Bit 9, `FIBF_GRP_EXECUTE`: set means the group may execute.
pub const FIBF_GRP_EXECUTE: u32 = 0x0000_0200;
/// Bit 10, `FIBF_GRP_WRITE`: set means the group may write.
pub const FIBF_GRP_WRITE: u32 = 0x0000_0400;
/// Bit 11, `FIBF_GRP_READ`: set means the group may read.
pub const FIBF_GRP_READ: u32 = 0x0000_0800;

/// Bit 12, `FIBF_OTR_DELETE`: set means others may delete.
pub const FIBF_OTR_DELETE: u32 = 0x0000_1000;
/// Bit 13, `FIBF_OTR_EXECUTE`: set means others may execute.
pub const FIBF_OTR_EXECUTE: u32 = 0x0000_2000;
/// Bit 14, `FIBF_OTR_WRITE`: set means others may write.
pub const FIBF_OTR_WRITE: u32 = 0x0000_4000;
/// Bit 15, `FIBF_OTR_READ`: set means others may read.
pub const FIBF_OTR_READ: u32 = 0x0000_8000;

/// Bits 16..=23, reserved by AmigaDOS for the *user* — never interpreted
/// by the filesystem, preserved by it.
pub const FIBF_USER_MASK: u32 = 0x00FF_0000;

/// A typed view over the 32-bit protection longword.
///
/// # The inverted-sense trap
///
/// The owner's four RWED bits (0..=3) are **denial** bits: a zero bit
/// means the operation is *allowed*. A freshly created file therefore has
/// protection `0`, meaning full owner access, and `List` prints `----rwed`
/// for it. The group and other nibbles above (8..=15), added later by
/// muFS and adopted into the standard layout, use the **normal** sense: a
/// set bit grants.
///
/// So `0x0000_0000` is "owner may do everything, nobody else may do
/// anything", and the two halves of the same longword disagree about what
/// a 1 means. Every accessor here returns "is this allowed", so callers
/// never have to remember which half they are in; [`Protection::bits`]
/// hands back the raw longword for anyone who does.
///
/// The whole longword is carried, not just the byte AmigaDOS's own
/// `Examine()` shows: muFS uses the group/other bits for real, and the
/// upper bits are the user's to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct Protection(u32);

impl Protection {
    /// Wrap a raw protection longword.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw longword, exactly as it sits on disk.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The owner may read. Bit 3 *clear*.
    pub const fn readable(self) -> bool {
        self.0 & FIBF_READ == 0
    }
    /// The owner may write. Bit 2 *clear*.
    pub const fn writable(self) -> bool {
        self.0 & FIBF_WRITE == 0
    }
    /// The owner may execute. Bit 1 *clear*.
    pub const fn executable(self) -> bool {
        self.0 & FIBF_EXECUTE == 0
    }
    /// The owner may delete. Bit 0 *clear*.
    pub const fn deletable(self) -> bool {
        self.0 & FIBF_DELETE == 0
    }

    /// The file has been archived since it last changed (bit 4).
    pub const fn archived(self) -> bool {
        self.0 & FIBF_ARCHIVE != 0
    }
    /// The executable is re-entrant and may be made resident (bit 5).
    pub const fn pure(self) -> bool {
        self.0 & FIBF_PURE != 0
    }
    /// The file is a Shell script (bit 6).
    pub const fn script(self) -> bool {
        self.0 & FIBF_SCRIPT != 0
    }
    /// The file asks to be hidden from listings (bit 7).
    pub const fn hidden(self) -> bool {
        self.0 & FIBF_HIDDEN != 0
    }

    /// The group may read. Bit 11 *set* — the normal sense, unlike the
    /// owner's.
    pub const fn group_readable(self) -> bool {
        self.0 & FIBF_GRP_READ != 0
    }
    /// The group may write (bit 10 set).
    pub const fn group_writable(self) -> bool {
        self.0 & FIBF_GRP_WRITE != 0
    }
    /// The group may execute (bit 9 set).
    pub const fn group_executable(self) -> bool {
        self.0 & FIBF_GRP_EXECUTE != 0
    }
    /// The group may delete (bit 8 set).
    pub const fn group_deletable(self) -> bool {
        self.0 & FIBF_GRP_DELETE != 0
    }

    /// Others may read (bit 15 set).
    pub const fn other_readable(self) -> bool {
        self.0 & FIBF_OTR_READ != 0
    }
    /// Others may write (bit 14 set).
    pub const fn other_writable(self) -> bool {
        self.0 & FIBF_OTR_WRITE != 0
    }
    /// Others may execute (bit 13 set).
    pub const fn other_executable(self) -> bool {
        self.0 & FIBF_OTR_EXECUTE != 0
    }
    /// Others may delete (bit 12 set).
    pub const fn other_deletable(self) -> bool {
        self.0 & FIBF_OTR_DELETE != 0
    }

    /// Bits 16..=23, the user's own, shifted down to a byte. The
    /// filesystem never reads these; this crate never writes them.
    pub const fn user_bits(self) -> u8 {
        ((self.0 & FIBF_USER_MASK) >> 16) as u8
    }
}

/// The eight-character form `List` prints: `hsparwed`, with `-` for each
/// flag that is off — and, for `rwed`, off meaning *denied*, so the
/// letters appear exactly when the owner is allowed. The group and other
/// bits have no place in this notation and are not shown; that is
/// AmigaDOS's omission, faithfully reproduced.
impl fmt::Display for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const LETTERS: &[u8; 8] = b"hsparwed";
        let allowed = [
            self.hidden(),
            self.script(),
            self.pure(),
            self.archived(),
            self.readable(),
            self.writable(),
            self.executable(),
            self.deletable(),
        ];
        let mut out = [b'-'; 8];
        for (o, (&letter, &on)) in out.iter_mut().zip(LETTERS.iter().zip(allowed.iter())) {
            if on {
                *o = letter;
            }
        }
        // ASCII by construction, so this cannot fail.
        f.write_str(core::str::from_utf8(&out).unwrap_or("????????"))
    }
}

// ---------------------------------------------------------------------------
// Calendar conversion
// ---------------------------------------------------------------------------

/// A `DateStamp` unpacked into a proleptic-Gregorian calendar date.
///
/// `year` is signed and full (`1978`, not `78`) because the two-digit
/// habit is precisely how the platform acquired its date bugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CalendarDate {
    /// Full year, e.g. `1978`.
    pub year: i32,
    /// Month, 1..=12.
    pub month: u32,
    /// Day of month, 1..=31.
    pub day: u32,
    /// Hour, 0..=23.
    pub hour: u32,
    /// Minute, 0..=59.
    pub minute: u32,
    /// Second, 0..=59.
    pub second: u32,
    /// Ticks past the second, 0..=49 (a tick is 1/50 s, the PAL field
    /// rate — the Amiga's clock counts frames, not milliseconds).
    pub tick: u32,
}

/// Ticks in a second: the vertical blank rate the `DateStamp`'s third
/// longword counts in. 50, not 60, on every machine — the field is
/// defined in units of 1/50 s regardless of the display standard.
pub const TICKS_PER_SECOND: u32 = 50;
/// Ticks in a minute, the modulus of `DateStamp::ticks`.
pub const TICKS_PER_MINUTE: u32 = TICKS_PER_SECOND * 60;
/// Minutes in a day, the modulus of `DateStamp::mins`.
pub const MINUTES_PER_DAY: u32 = 24 * 60;

/// Days from the Unix epoch (1970-01-01) to the AmigaDOS epoch
/// (1978-01-01): eight years containing two leap days (1972, 1976).
pub const AMIGA_EPOCH_UNIX_DAYS: i64 = 8 * 365 + 2;

impl DateStamp {
    /// Convert to a calendar date and time.
    ///
    /// # Leap years
    ///
    /// This uses the *correct* proleptic Gregorian rule — divisible by 4,
    /// except centuries, except multiples of 400 — computed with Howard
    /// Hinnant's `civil_from_days`, which is exact for the whole `u32`
    /// range of `days` without a table or a loop.
    ///
    /// AmigaDOS's own `Amiga2Date()` in `dos.library` uses only the
    /// divisible-by-4 rule in its older implementations, so a Rust
    /// conversion and a running Amiga agree on every date this crate will
    /// ever meet on a real volume and then diverge by one day from
    /// 2100-03-01 onward, where the Amiga believes in a 29 February that
    /// the calendar does not have. The disagreement is the platform's,
    /// not this crate's, and this crate declines to reproduce it: a
    /// filesystem library's job is to say what day the number means, and
    /// a consumer that needs bug-compatibility with a specific ROM has
    /// [`DateStamp::days`] to do it with.
    ///
    /// # Normalisation
    ///
    /// A stamp whose `mins` reaches into another day, or whose `ticks`
    /// reach into another minute, is out of contract but not unheard of
    /// on real disks. The overflow is carried upward rather than clamped
    /// or refused, so the result is always a real moment in time and the
    /// arithmetic never wraps.
    pub fn to_calendar(self) -> CalendarDate {
        let mins = self.mins as u64 + (self.ticks / TICKS_PER_MINUTE) as u64;
        let ticks = self.ticks % TICKS_PER_MINUTE;
        let days = self.days as u64 + mins / MINUTES_PER_DAY as u64;
        let mins = (mins % MINUTES_PER_DAY as u64) as u32;

        let (year, month, day) = civil_from_days(days as i64 + AMIGA_EPOCH_UNIX_DAYS);
        CalendarDate {
            year,
            month,
            day,
            hour: mins / 60,
            minute: mins % 60,
            second: ticks / TICKS_PER_SECOND,
            tick: ticks % TICKS_PER_SECOND,
        }
    }

    /// The inverse: a calendar date back to the three longwords.
    ///
    /// Returns `None` for anything before the AmigaDOS epoch or past what
    /// `u32` days can hold — the `DateStamp` simply has no room for those,
    /// and silently wrapping into a plausible-looking 1978 date is how a
    /// bad host timestamp becomes a bad disk.
    pub fn from_calendar(date: CalendarDate) -> Option<Self> {
        let days = days_from_civil(date.year, date.month, date.day)? - AMIGA_EPOCH_UNIX_DAYS;
        if !(0..=u32::MAX as i64).contains(&days) {
            return None;
        }
        if date.hour > 23 || date.minute > 59 || date.second > 59 || date.tick >= TICKS_PER_SECOND {
            return None;
        }
        Some(Self {
            days: days as u32,
            mins: date.hour * 60 + date.minute,
            ticks: date.second * TICKS_PER_SECOND + date.tick,
        })
    }
}

/// Days since 1970-01-01 to (year, month, day). Hinnant's algorithm,
/// shifted to an era beginning on 0000-03-01 so that the leap day is the
/// last day of the year and the month lengths become a linear formula.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // 0..=146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..=399
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp = (5 * doy + 2) / 153; // 0..=11, March-based
    let d = doy - (153 * mp + 2) / 5 + 1; // 1..=31
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..=12
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

/// The exact inverse of [`civil_from_days`]. `None` for a month or day
/// outside the calendar rather than a silently normalised answer.
fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Days in a month under the full Gregorian rule.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// The Gregorian leap rule in full: every fourth year, except centuries,
/// except every fourth century. The "except centuries" clause is the one
/// AmigaDOS's own conversion omits, and 2100 is where that shows.
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn a_zero_protection_long_allows_the_owner_everything() {
        let p = Protection::from_bits(0);
        assert!(p.readable() && p.writable() && p.executable() && p.deletable());
        assert!(!p.hidden() && !p.script() && !p.pure() && !p.archived());
        // ...and grants nobody else anything: the other half is normal-sense.
        assert!(!p.group_readable() && !p.other_readable());
        assert_eq!(p.to_string(), "----rwed");
    }

    #[test]
    fn setting_the_low_nibble_denies_rather_than_grants() {
        let p = Protection::from_bits(0xF);
        assert!(!p.readable() && !p.writable() && !p.executable() && !p.deletable());
        assert_eq!(p.to_string(), "--------");

        // One bit at a time, each denying exactly its own operation.
        let d = Protection::from_bits(FIBF_DELETE);
        assert!(d.readable() && d.writable() && d.executable() && !d.deletable());
        assert_eq!(d.to_string(), "----rwe-");
        let w = Protection::from_bits(FIBF_WRITE);
        assert!(w.readable() && !w.writable());
        assert_eq!(w.to_string(), "----r-ed");
    }

    #[test]
    fn hspa_and_the_group_other_nibbles_read_normally() {
        let p = Protection::from_bits(FIBF_HIDDEN | FIBF_SCRIPT | FIBF_PURE | FIBF_ARCHIVE);
        assert!(p.hidden() && p.script() && p.pure() && p.archived());
        assert_eq!(p.to_string(), "hsparwed");

        let g = Protection::from_bits(FIBF_GRP_READ | FIBF_GRP_EXECUTE);
        assert!(g.group_readable() && g.group_executable());
        assert!(!g.group_writable() && !g.group_deletable());
        assert!(!g.other_readable());
        // And the group bits do not disturb the owner's, which stay
        // "allowed" because they are still zero.
        assert!(g.readable() && g.deletable());

        let o = Protection::from_bits(0xF000);
        assert!(o.other_readable() && o.other_writable() && o.other_executable());
        assert!(o.other_deletable() && !o.group_readable());

        assert_eq!(Protection::from_bits(0x00AB_0000).user_bits(), 0xAB);
        assert_eq!(Protection::from_bits(0xFFFF_FFFF).bits(), 0xFFFF_FFFF);
    }

    #[test]
    fn day_zero_is_the_first_of_january_1978() {
        let c = DateStamp {
            days: 0,
            mins: 0,
            ticks: 0,
        }
        .to_calendar();
        assert_eq!((c.year, c.month, c.day), (1978, 1, 1));
        assert_eq!((c.hour, c.minute, c.second, c.tick), (0, 0, 0, 0));
    }

    #[test]
    fn the_clock_fields_decode_as_minutes_and_fiftieths() {
        let c = DateStamp {
            days: 0,
            mins: 13 * 60 + 45,
            ticks: 42 * 50 + 7,
        }
        .to_calendar();
        assert_eq!((c.hour, c.minute, c.second, c.tick), (13, 45, 42, 7));
    }

    #[test]
    fn leap_days_land_where_the_calendar_puts_them() {
        // 1978 is not a leap year: day 365 is 1979-01-01.
        let c = DateStamp {
            days: 365,
            ..Default::default()
        }
        .to_calendar();
        assert_eq!((c.year, c.month, c.day), (1979, 1, 1));

        // 1980 is: 1978 and 1979 are 365 days each, so 1980-01-01 is day
        // 730 and 1980-02-28 is day 788.
        let feb28 = 365 + 365 + 31 + 27;
        assert_eq!(feb28, 788);
        let c = DateStamp {
            days: feb28,
            ..Default::default()
        }
        .to_calendar();
        assert_eq!((c.year, c.month, c.day), (1980, 2, 28));
        let c = DateStamp {
            days: feb28 + 1,
            ..Default::default()
        }
        .to_calendar();
        assert_eq!(
            (c.year, c.month, c.day),
            (1980, 2, 29),
            "1980 is a leap year"
        );
        let c = DateStamp {
            days: feb28 + 2,
            ..Default::default()
        }
        .to_calendar();
        assert_eq!((c.year, c.month, c.day), (1980, 3, 1));

        assert!(is_leap_year(1980) && is_leap_year(2000) && is_leap_year(2400));
        assert!(!is_leap_year(1900) && !is_leap_year(2100) && !is_leap_year(2001));
    }

    #[test]
    fn the_century_rule_is_honoured_where_amigados_forgets_it() {
        // 2100-02-28 and the day after. The Gregorian answer is 03-01;
        // an implementation using only the divisible-by-4 rule reports a
        // 29 February here, and every date after it is a day out.
        let feb28 = DateStamp::from_calendar(CalendarDate {
            year: 2100,
            month: 2,
            day: 28,
            hour: 0,
            minute: 0,
            second: 0,
            tick: 0,
        })
        .unwrap();
        let next = DateStamp {
            days: feb28.days + 1,
            ..feb28
        }
        .to_calendar();
        assert_eq!((next.year, next.month, next.day), (2100, 3, 1));
        // The date the naive rule would invent cannot be constructed.
        assert!(DateStamp::from_calendar(CalendarDate {
            year: 2100,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0,
            tick: 0,
        })
        .is_none());
    }

    #[test]
    fn a_recent_date_round_trips() {
        // 2024-02-29 12:34:56.25 -- a real leap day, a real time.
        let want = CalendarDate {
            year: 2024,
            month: 2,
            day: 29,
            hour: 12,
            minute: 34,
            second: 56,
            tick: 25,
        };
        let stamp = DateStamp::from_calendar(want).unwrap();
        assert_eq!(stamp.to_calendar(), want);
        // 1978-01-01 to 2024-02-29 inclusive of leap days.
        assert_eq!(stamp.days, 16_860);
        assert_eq!(stamp.mins, 12 * 60 + 34);
        assert_eq!(stamp.ticks, 56 * 50 + 25);
    }

    #[test]
    fn every_day_for_two_centuries_round_trips() {
        // The cheapest possible proof that the two conversions are
        // inverses and that no month boundary is off by one.
        for days in 0..73_000u32 {
            let s = DateStamp {
                days,
                mins: 1,
                ticks: 1,
            };
            let c = s.to_calendar();
            assert!((1..=12).contains(&c.month) && c.day >= 1);
            assert!(c.day <= days_in_month(c.year, c.month));
            assert_eq!(DateStamp::from_calendar(c), Some(s), "day {days}");
        }
    }

    #[test]
    fn out_of_contract_stamps_carry_rather_than_wrap() {
        // 1440 minutes is the next day, not minute -1 of this one.
        let c = DateStamp {
            days: 0,
            mins: MINUTES_PER_DAY,
            ticks: TICKS_PER_MINUTE,
        }
        .to_calendar();
        assert_eq!((c.year, c.month, c.day), (1978, 1, 2));
        assert_eq!((c.hour, c.minute, c.second), (0, 1, 0));

        // And the far end of the range does not panic.
        let c = DateStamp {
            days: u32::MAX,
            mins: u32::MAX,
            ticks: u32::MAX,
        }
        .to_calendar();
        assert!(c.year > 11_000_000);

        // Before the epoch is not representable, and says so.
        assert!(DateStamp::from_calendar(CalendarDate {
            year: 1977,
            month: 12,
            day: 31,
            hour: 0,
            minute: 0,
            second: 0,
            tick: 0,
        })
        .is_none());
    }
}
