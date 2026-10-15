//! filter implementation.
use super::*;

/// 过滤谓词——内存来源(卷快照 / 实时遍历)用它过滤;`filedb_query` 的
/// SQL WHERE 子句是它的逐条镜像,语义由 `filedb_query` 测试对齐。
pub fn flat_filter_keeps(entry: &FsWalkEntry, filter: &FlatFileFilter) -> bool {
    let node = &entry.node;
    if !is_flat_kind(node) {
        return false;
    }
    node_filter_keeps(entry, filter)
}

/// Shared metadata predicate, including navigation nodes.
pub fn node_filter_keeps(entry: &FsWalkEntry, filter: &FlatFileFilter) -> bool {
    let node = &entry.node;
    let path_low = entry.path.to_lowercase();
    if let Some(keyword) = &filter.keyword {
        let hit = node.name.to_lowercase().contains(keyword) || path_low.contains(keyword);
        if !hit {
            return false;
        }
    }
    if let Some(query) = &filter.name_query {
        if !name_query_matches(query, &node.name.to_lowercase()) {
            return false;
        }
    }
    if let Some(query) = &filter.path_query {
        let anchored = query.starts_with('/');
        let hit = if anchored {
            format!("{path_low}/").starts_with(query.as_str())
        } else {
            path_low.contains(query)
        };
        if !hit {
            return false;
        }
    }
    if let Some(query) = &filter.ext_query {
        if !flat_ext_of(node).contains(query) {
            return false;
        }
    }
    if let Some(query) = &filter.kind_query {
        let (chinese, english) = kind_filter_labels(node);
        if !chinese.contains(query) && !english.contains(query) {
            return false;
        }
    }
    if let Some(query) = &filter.link_query {
        let target = node.link_target.as_deref().unwrap_or_default();
        if !target.to_lowercase().contains(query) {
            return false;
        }
    }
    if let Some(query) = &filter.inode_query {
        let inode_text = node.inode.map(|v| v.to_string()).unwrap_or_default();
        if !inode_text.contains(query) {
            return false;
        }
    }
    if let Some(want_deleted) = filter.deleted {
        if node.deleted != want_deleted {
            return false;
        }
    }
    if let Some(bound) = filter.size_ge {
        if node.size < bound {
            return false;
        }
    }
    if let Some(bound) = filter.size_le {
        if node.size > bound {
            return false;
        }
    }
    if let Some(mod_) = &filter.size_mod {
        if node.size % mod_.divisor != mod_.remainder {
            return false;
        }
    }
    if !time_range_keeps(created_of(node), filter.time_created.as_ref())
        || !time_range_keeps(
            node.timestamps.accessed.as_deref(),
            filter.time_accessed.as_ref(),
        )
        || !time_range_keeps(
            node.timestamps.modified.as_deref(),
            filter.time_modified.as_ref(),
        )
    {
        return false;
    }
    if !filter.extensions.is_empty() || !filter.other_category_exts.is_empty() {
        let ext = flat_ext_of(node);
        let in_include = filter.extensions.contains(&ext);
        let in_other =
            !filter.other_category_exts.is_empty() && !filter.other_category_exts.contains(&ext);
        if !in_include && !in_other {
            return false;
        }
    }
    true
}

/// 时间区间判断,与前端 `matchesTimeFilter` 同口径:节点时间戳转显示串
/// (`YYYY-MM-DD HH:MM:SS`)后做字典序比较;缺失/无法解析的行在区间激活
/// 时一律排除(取证上「时间未知」不能蒙混进时间窗)。
pub(super) fn time_range_keeps(value: Option<&str>, range: Option<&FlatTimeRange>) -> bool {
    let Some(range) = range else {
        return true;
    };
    let Some(display) = display_timestamp(value) else {
        return false;
    };
    if let Some(from) = range.from.as_deref() {
        if display.as_str() < from {
            return false;
        }
    }
    if let Some(to) = range.to.as_deref() {
        if display.as_str() > to {
            return false;
        }
    }
    true
}

/// RFC3339 时间戳 → 显示口径 `YYYY-MM-DD HH:MM:SS`(与前端
/// `formatTimestamp` 的正则一致)。无法识别返回 `None`。
pub(crate) fn display_timestamp(value: Option<&str>) -> Option<String> {
    let value = value?;
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let digits_at = |positions: &[usize]| positions.iter().all(|&i| bytes[i].is_ascii_digit());
    let shape_ok = digits_at(&[0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18])
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !shape_ok {
        return None;
    }
    let mut display = String::with_capacity(19);
    display.push_str(&value[..10]);
    display.push(' ');
    display.push_str(&value[11..19]);
    Some(display)
}

/// 把显示口径的区间边界换算成 filedb 时间列可比的 RFC3339 前缀边界:
/// `from` 含下界(原样加 `T`),`to` 含上界换算成 +1 秒的排他上界——列值
/// 带毫秒后缀(`…:05.123Z`)时字典序仍然落在区间内,与谓词的显示串比较
/// 等价(边界同秒内混排格式的极端情形除外,见模块测试)。
pub(crate) fn time_range_sql_bounds(range: &FlatTimeRange) -> (Option<String>, Option<String>) {
    let from = range.from.as_deref().and_then(display_to_sql_prefix);
    let to = range.to.as_deref().and_then(|to| {
        let prefix = display_to_sql_prefix(to)?;
        let naive = chrono::NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%dT%H:%M:%S").ok()?;
        Some(
            (naive + chrono::Duration::seconds(1))
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
        )
    });
    (from, to)
}

/// `YYYY-MM-DD HH:MM:SS` → `YYYY-MM-DDTHH:MM:SS`(列值的可比前缀形态)。
pub(super) fn display_to_sql_prefix(value: &str) -> Option<String> {
    let mut prefix = display_timestamp(Some(value))?;
    prefix.replace_range(10..11, "T");
    Some(prefix)
}

/// 名称通配匹配,与前端 `nameQueryMatcher` 同语义:`*` 任意串、`?` 单字,
/// 无通配符退化为子串。入参双方均已小写。双指针实现避免正则回溯。
pub(super) fn name_query_matches(query: &str, lower_name: &str) -> bool {
    if !query.contains(['*', '?']) {
        return lower_name.contains(query);
    }
    let pattern: Vec<char> = query.chars().collect();
    let chars: Vec<char> = lower_name.chars().collect();
    let mut string_index = 0usize;
    let mut pattern_index = 0usize;
    let mut star_pattern_index: Option<usize> = None;
    let mut star_string_index = 0usize;
    while string_index < chars.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == chars[string_index])
        {
            pattern_index += 1;
            string_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_pattern_index = Some(pattern_index);
            pattern_index += 1;
            star_string_index = string_index;
        } else if let Some(star) = star_pattern_index {
            pattern_index = star + 1;
            star_string_index += 1;
            string_index = star_string_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// 名称漏斗的 SQL LIKE 形态(无通配符 → 包含;有通配符 → `*`→`%`、`?`→
/// `_`,先转义 `%`/`_`/`\`)。仅普通子串查询包 `%…%`，通配符保持整名匹配；均带 `ESCAPE '\'`。
pub(crate) fn name_query_like_pattern(query: &str) -> String {
    if !query.contains(['*', '?']) {
        return escape_like(query);
    }
    let mut pattern = String::with_capacity(query.len());
    for character in query.chars() {
        match character {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(character);
            }
            '*' => pattern.push('%'),
            '?' => pattern.push('_'),
            other => pattern.push(other),
        }
    }
    pattern
}

pub(super) fn escape_like(needle: &str) -> String {
    let mut escaped = String::with_capacity(needle.len());
    for character in needle.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
