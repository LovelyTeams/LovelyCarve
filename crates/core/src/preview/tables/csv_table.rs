//! CSV / TSV → 行列二维数组解析，供「原类型视图」表格渲染（前端 CsvViewer）。
//! 解析在后端：镜像内文件经 `fs_tree::read_file_limited` 取字节后在此解析成结构化
//! 表格，前端只负责渲染表格与搜索/排序交互（与 SQLite / PE / GGUF 原类型视图同构）。
//!
//! 解析遵循 RFC 4180 的常见宽松实现（行为对齐原前端 `src/lib/table/csv.ts`）：
//! - 字段以分隔符分隔，记录以 `\n` / `\r\n` / 裸 `\r` 分隔；
//! - 字段可用双引号包裹，引号内的分隔符 / 换行为字面量，`""` 表示一个字面引号；
//! - 引号只在字段起始处才开启包裹（`a"b` 中的引号为字面量，贴近表格软件行为）；
//! - 末尾单个换行不产出多余空记录（但中间空行保留为单空字段行，取证忠实）；
//! - 字段不做 trim（忠实保留原始空白）。

use serde::Serialize;

/// 支持的分隔符候选。`\t` = TSV，其余为 CSV 变体（含欧式分号、竖线）。
/// 顺序即平局时的优先级（嗅探打平取靠前者）。
pub const DELIMITERS: [char; 4] = [',', ';', '\t', '|'];

/// 单次返回的最大数据行数；超出标记 `truncated_rows`，`rows` 只截前 `MAX_ROWS` 行
/// （`total_rows` 仍报读取窗口内的真实总数）。与前端渲染上限解耦——前端可再窗口化，
/// 但 IPC payload 必须有界。
pub const MAX_ROWS: usize = 50_000;

/// 解析结果。字段以 camelCase 序列化，直接对应前端 `CsvTable` 接口。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvTable {
    /// 实际使用的分隔符（单字符字符串）。
    pub delimiter: String,
    /// 分隔符是否为自动嗅探（true）还是调用方明确指定（false）。
    pub sniffed: bool,
    /// 返回的行（含可能的表头行），按文件序；ragged（行可不等长，前端按
    /// `col_count` 右侧补空）。至多 `MAX_ROWS` 行。
    pub rows: Vec<Vec<String>>,
    /// 返回行内的最大列宽，渲染时用来补齐成矩形。
    pub col_count: usize,
    /// 读取窗口内解析到的总行数（用于「共 N 行」提示）。
    pub total_rows: usize,
    /// 总行数超过 `MAX_ROWS`，`rows` 只含前 `MAX_ROWS` 行。
    pub truncated_rows: bool,
    /// 文件大于读取上限，仅解析了前缀字节（由调用方按读取结果置位）。
    pub truncated_bytes: bool,
}

/// 把分隔符字符串（前端传入，如 "," / "\t"）解析为单个字符；空串 / `"auto"`
/// 视为「自动嗅探」（返回 None）。多字符时取首字符。
pub fn delimiter_from_str(s: Option<&str>) -> Option<char> {
    match s {
        None => None,
        Some(s) if s.is_empty() || s == "auto" => None,
        Some(s) => s.chars().next(),
    }
}

/// 嗅探最可能的分隔符：在前若干行内统计各候选「引号外」的出现次数，取最多者。
/// 全为 0（如只有一列、无分隔符）时回退逗号。平局按 `DELIMITERS` 顺序取靠前者。
fn sniff_delimiter(chars: &[char]) -> char {
    let mut counts = [0usize; DELIMITERS.len()];
    let mut in_quotes = false;
    let mut lines = 0usize;
    // 只看头部即可定分隔符；大文件不必整篇扫描（上限约 64K 字符）。
    let cap = chars.len().min(64 * 1024);
    let mut i = 0;
    while i < cap {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    i += 2; // 跳过转义引号的两个字符
                    continue;
                }
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_quotes = true;
            i += 1;
            continue;
        }
        if c == '\n' {
            lines += 1;
            if lines >= 20 {
                break;
            }
            i += 1;
            continue;
        }
        for (k, d) in DELIMITERS.iter().enumerate() {
            if c == *d {
                counts[k] += 1;
                break;
            }
        }
        i += 1;
    }
    let mut best = 0usize;
    let mut best_n = 0usize;
    for (k, &n) in counts.iter().enumerate() {
        if n > best_n {
            best_n = n;
            best = k;
        }
    }
    if best_n > 0 {
        DELIMITERS[best]
    } else {
        ','
    }
}

/// 把 CSV/TSV 字符序列解析成行列二维数组（ragged，行可不等长）。空输入返回空。
fn parse_rows(chars: &[char], delimiter: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    // 当前字段是否仍在起始处（尚未追加任何字符）——决定一个引号是开启包裹还是字面量。
    let mut field_start = true;
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    field.push('"');
                    i += 2;
                    continue;
                }
                in_quotes = false;
                i += 1;
                continue;
            }
            field.push(c);
            i += 1;
            continue;
        }
        if c == '"' && field_start {
            in_quotes = true;
            field_start = false;
            i += 1;
            continue;
        }
        if c == delimiter {
            row.push(std::mem::take(&mut field));
            field_start = true;
            i += 1;
            continue;
        }
        if c == '\n' {
            row.push(std::mem::take(&mut field));
            field_start = true;
            rows.push(std::mem::take(&mut row));
            i += 1;
            continue;
        }
        if c == '\r' {
            row.push(std::mem::take(&mut field));
            field_start = true;
            rows.push(std::mem::take(&mut row));
            i += if chars.get(i + 1) == Some(&'\n') {
                2
            } else {
                1
            }; // \r\n 与裸 \r 都算一个记录分隔
            continue;
        }
        field.push(c);
        field_start = false;
        i += 1;
    }

    // 收尾：仅当尚有挂起内容时补最后一条记录——末尾单个换行不产出空行。
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// 解析文件字节为结构化表格。`delimiter` 为 None 时自动嗅探；`truncated_bytes`
/// 由调用方传入（文件是否已被读取上限截断）。
pub fn parse(data: &[u8], delimiter: Option<char>, truncated_bytes: bool) -> CsvTable {
    // 字节 → 文本：UTF-8 lossy（非法字节 → U+FFFD，对齐前端 TextDecoder 的非 fatal
    // 行为），并去掉可能的前导 BOM。
    let mut text = String::from_utf8_lossy(data).into_owned();
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    let chars: Vec<char> = text.chars().collect();

    let (delim, sniffed) = match delimiter {
        Some(d) => (d, false),
        None => (sniff_delimiter(&chars), true),
    };

    let mut rows = parse_rows(&chars, delim);
    let total_rows = rows.len();
    let truncated_rows = total_rows > MAX_ROWS;
    if truncated_rows {
        rows.truncate(MAX_ROWS);
    }
    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);

    CsvTable {
        delimiter: delim.to_string(),
        sniffed,
        rows,
        col_count,
        total_rows,
        truncated_rows,
        truncated_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, delim: char) -> Vec<Vec<String>> {
        parse_rows(&text.chars().collect::<Vec<_>>(), delim)
    }

    fn sniff(text: &str) -> char {
        sniff_delimiter(&text.chars().collect::<Vec<_>>())
    }

    #[test]
    fn parses_simple_comma_table() {
        assert_eq!(
            rows("a,b,c\n1,2,3", ','),
            vec![vec!["a", "b", "c"], vec!["1", "2", "3"]]
        );
    }

    #[test]
    fn no_trailing_empty_row_for_final_newline() {
        assert_eq!(
            rows("a,b\n1,2\n", ','),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn handles_crlf_and_final_crlf() {
        assert_eq!(
            rows("a,b\r\n1,2\r\n", ','),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn keeps_delimiters_and_newlines_inside_quotes() {
        assert_eq!(
            rows("\"a,1\",\"line\nbreak\",z", ','),
            vec![vec!["a,1", "line\nbreak", "z"]]
        );
    }

    #[test]
    fn unescapes_doubled_quotes() {
        assert_eq!(
            rows("\"she said \"\"hi\"\"\",ok", ','),
            vec![vec!["she said \"hi\"", "ok"]]
        );
    }

    #[test]
    fn quote_not_at_field_start_is_literal() {
        assert_eq!(rows("a\"b,c", ','), vec![vec!["a\"b", "c"]]);
    }

    #[test]
    fn empty_fields_and_ragged_rows() {
        assert_eq!(rows("a,,c\nd", ','), vec![vec!["a", "", "c"], vec!["d"]]);
    }

    #[test]
    fn keeps_blank_middle_line_as_single_empty_field() {
        assert_eq!(rows("a\n\nb", ','), vec![vec!["a"], vec![""], vec!["b"]]);
    }

    #[test]
    fn parses_tsv() {
        assert_eq!(
            rows("a\tb\n1\t2", '\t'),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(rows("", ',').is_empty());
    }

    #[test]
    fn sniffs_comma_semicolon_tab() {
        assert_eq!(sniff("a,b,c\n1,2,3"), ',');
        assert_eq!(sniff("a;b;c\n1;2;3"), ';');
        assert_eq!(sniff("a\tb\tc\n1\t2\t3"), '\t');
    }

    #[test]
    fn sniff_ignores_delimiters_inside_quotes() {
        assert_eq!(sniff("\"a,a,a\";b\n\"c,c,c\";d"), ';');
    }

    #[test]
    fn sniff_falls_back_to_comma_for_single_column() {
        assert_eq!(sniff("single-column\nvalue"), ',');
    }

    #[test]
    fn parse_strips_bom_and_reports_sniffed() {
        let table = parse("\u{feff}a,b\n1,2".as_bytes(), None, false);
        assert_eq!(table.delimiter, ",");
        assert!(table.sniffed);
        assert_eq!(table.rows, vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(table.col_count, 2);
        assert_eq!(table.total_rows, 2);
        assert!(!table.truncated_rows);
    }

    #[test]
    fn parse_honors_explicit_delimiter() {
        let table = parse("a;b\n1;2".as_bytes(), delimiter_from_str(Some(";")), false);
        assert_eq!(table.delimiter, ";");
        assert!(!table.sniffed);
        assert_eq!(table.rows, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn parse_caps_rows_and_reports_total() {
        let mut text = String::new();
        for i in 0..(MAX_ROWS + 100) {
            text.push_str(&format!("r{i}\n"));
        }
        let table = parse(text.as_bytes(), Some(','), false);
        assert_eq!(table.rows.len(), MAX_ROWS);
        assert_eq!(table.total_rows, MAX_ROWS + 100);
        assert!(table.truncated_rows);
    }
}
