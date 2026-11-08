//! Cell display text and spreadsheet number/date formatting.
use super::*;

/// 把 calamine 单元格值渲染为显示文本（移植 `xlsx.ts::cellText` 的分支）。
pub(super) fn render_cell(cell: &Data, numfmt: &str) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::Int(i) => {
            if numfmt.is_empty() {
                i.to_string()
            } else {
                format_number(*i as f64, numfmt)
            }
        }
        Data::Float(f) => {
            if numfmt.is_empty() {
                trim_float(*f)
            } else {
                format_number(*f, numfmt)
            }
        }
        Data::Error(e) => e.to_string(), // #REF! / #VALUE! / #DIV/0! …
        Data::DateTime(dt) => format_excel_datetime(dt, numfmt),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
    }
}

/// f64 → 最短往返字符串，对齐 JS `String(value)`（3.0→"3"，3.5→"3.5"）。
pub(super) fn trim_float(value: f64) -> String {
    value.to_string()
}

/// 数字按 numFmt 还原百分比 / 小数位 / 千分位 / 货币前缀（移植 `xlsx.ts::formatNumber`）。
pub(super) fn format_number(value: f64, num_fmt: &str) -> String {
    if num_fmt.is_empty() {
        return trim_float(value);
    }
    if num_fmt.ends_with('%') {
        return match frac_digits_before_percent(num_fmt) {
            Some(p) => format!("{:.*}%", p, value * 100.0),
            None => format!("{}%", trim_float(value * 100.0)),
        };
    }
    if num_fmt.contains('0') {
        let prefix = if num_fmt.starts_with('$') {
            "$"
        } else if num_fmt.starts_with("\"¥") || num_fmt.starts_with('¥') {
            "¥"
        } else {
            ""
        };
        if value == 0.0 && num_fmt.starts_with('_') {
            return "-".to_string();
        }
        let precision = decimal_precision(num_fmt);
        let mut result = format!("{value:.precision$}");
        if num_fmt.contains("#,##") {
            result = group_thousands(&result);
        }
        return format!("{prefix}{result}");
    }
    trim_float(value)
}

/// `.(\d+)%` 中 `\d+` 的位数（百分比小数精度）；无则 None。
pub(super) fn frac_digits_before_percent(fmt: &str) -> Option<usize> {
    let bytes = fmt.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'.' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b'%' {
                return Some(j - (i + 1));
            }
        }
    }
    None
}

/// `0\.(0+)(_|;|$)` 中 `0+` 的位数（小数精度）；无则 0。
pub(super) fn decimal_precision(fmt: &str) -> usize {
    let bytes = fmt.as_bytes();
    if bytes.len() < 2 {
        return 0;
    }
    for i in 0..bytes.len() - 1 {
        if bytes[i] == b'0' && bytes[i + 1] == b'.' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] == b'0' {
                j += 1;
            }
            let count = j - (i + 2);
            if count > 0 && (j == bytes.len() || bytes[j] == b'_' || bytes[j] == b';') {
                return count;
            }
        }
    }
    0
}

/// 整数部分三位分组（移植 `xlsx.ts` 的手写千分位；负号前不插逗号）。
pub(super) fn group_thousands(result: &str) -> String {
    let (int_part, frac_part) = match result.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (result, None),
    };
    let digits: Vec<char> = int_part.chars().rev().collect();
    let mut grouped: Vec<char> = Vec::with_capacity(digits.len() + digits.len() / 3);
    for (i, &d) in digits.iter().enumerate() {
        grouped.push(d);
        if (i + 1) % 3 == 0 && i < digits.len() - 1 && digits[i + 1] != '-' {
            grouped.push(',');
        }
    }
    let int_grouped: String = grouped.iter().rev().collect();
    match frac_part {
        Some(f) => format!("{int_grouped}.{f}"),
        None => int_grouped,
    }
}

/// calamine ExcelDateTime → 显示字符串（UTC 取值，纪元正确）；无法转日期则退回原始序列号。
pub(super) fn format_excel_datetime(dt: &ExcelDateTime, num_fmt: &str) -> String {
    match dt.as_datetime() {
        Some(ndt) => format_date(ndt, num_fmt),
        None => trim_float(dt.as_f64()),
    }
}

/// 日期格式化（移植 `xlsx.ts::formatDate`）：numFmt 仅含时间标记时输出 HH:MM:SS；
/// 含时间分量时附时间，否则只给 YYYY-MM-DD。NaiveDateTime 的分量即原 JS 的 UTC 分量。
pub(super) fn format_date(dt: NaiveDateTime, num_fmt: &str) -> String {
    let fmt = num_fmt.to_lowercase();
    let h = dt.hour();
    let mi = dt.minute();
    let s = dt.second();
    let has_time = fmt.contains('h') || fmt.contains('m') || fmt.contains('s');
    let has_date = fmt.contains('y') || fmt.contains('m') || fmt.contains('d');
    if has_time && !has_date {
        return format!("{h:02}:{mi:02}:{s:02}");
    }
    let date = format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day());
    if has_time || h != 0 || mi != 0 || s != 0 {
        return format!("{date} {h:02}:{mi:02}:{s:02}");
    }
    date
}
