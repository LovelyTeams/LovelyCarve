//! Presentation formatting shared by the views.

/// File sizes in the file list and results tables: `3.13 MB`.
pub fn bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    let units = ["KB", "MB", "GB", "TB", "PB"];
    let mut amount = value as f64 / 1024.0;
    let mut unit = 0;
    while amount >= 1024.0 && unit < units.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    format!("{amount:.2} {}", units[unit])
}

/// Evidence capacity with IEC units matching the 1024 divisor: `58.00 GiB`.
pub fn binary_bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    let units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut amount = value as f64 / 1024.0;
    let mut unit = 0;
    while amount >= 1024.0 && unit < units.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    format!("{amount:.2} {}", units[unit])
}

/// `1234567` → `1,234,567`.
pub fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Backend ISO-8601 timestamps (`2026-04-03T11:10:48+00:00`) shown as
/// `2026-04-03 11:10:48`; anything else as it is, `—` when missing.
pub fn timestamp(value: Option<&str>) -> String {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return "—".to_owned();
    };
    let bytes = value.as_bytes();
    let shaped = bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && bytes[13] == b':'
        && bytes[16] == b':';
    match (shaped, value.get(..10), value.get(11..19)) {
        (true, Some(date), Some(time)) => format!("{date} {time}"),
        _ => value.to_owned(),
    }
}

/// Unix milliseconds as local `MM-DD HH:MM`, the task list's short form.
pub fn short_local_time(unix_ms: i64) -> String {
    use chrono::TimeZone as _;
    match chrono::Local.timestamp_millis_opt(unix_ms) {
        chrono::LocalResult::Single(time) | chrono::LocalResult::Ambiguous(time, _) => {
            time.format("%m-%d %H:%M").to_string()
        }
        chrono::LocalResult::None => "—".to_owned(),
    }
}

/// Seconds as `1 分 05 秒` / `3.2 秒`.
pub fn duration_secs(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_owned();
    }
    if seconds < 60.0 {
        return format!("{seconds:.1} 秒");
    }
    let whole = seconds.round() as u64;
    let (hours, minutes, secs) = (whole / 3600, whole % 3600 / 60, whole % 60);
    if hours > 0 {
        format!("{hours} 时 {minutes:02} 分")
    } else {
        format!("{minutes} 分 {secs:02} 秒")
    }
}

/// A byte offset as fixed-width hexadecimal: `0x000000002BD956000`.
pub fn hex_offset(value: u64) -> String {
    format!("0x{value:012X}")
}

/// The last path component of a host or evidence path.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

/// Percent of `done` in `total`, clamped to 0–100.
pub fn percent(done: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_two_decimals() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(3_282_000), "3.13 MB");
        assert_eq!(binary_bytes(62_277_025_792), "58.00 GiB");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
    }

    #[test]
    fn timestamps_drop_the_offset() {
        assert_eq!(
            timestamp(Some("2026-04-03T11:10:48+00:00")),
            "2026-04-03 11:10:48"
        );
        assert_eq!(
            timestamp(Some("2024-02-06T09:21:43.123Z")),
            "2024-02-06 09:21:43"
        );
        assert_eq!(timestamp(Some("unknown")), "unknown");
        assert_eq!(timestamp(None), "—");
    }

    #[test]
    fn file_names_split_both_separators() {
        assert_eq!(file_name("D:\\cases\\disk.E01"), "disk.E01");
        assert_eq!(file_name("/var/log/"), "log");
        assert_eq!(file_name("assembled://md0"), "md0");
    }
}
