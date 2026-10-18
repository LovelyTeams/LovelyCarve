//! 逻辑搜索(文件方式)的多编码关键词与正则匹配引擎。
//!
//! 对每个关键词按编码页（ASCII / UTF-8 / UTF-16LE / UTF-16BE / GBK，可由
//! 调用方选定子集）编译字节变体，合并到至多两台 Aho-Corasick 自动机；
//! 正则模式经 [`crate::core::search::keyword::keyword_regex`] 通道在同一遍扫描里合并。行为要点：
//!
//! - **多编码合并**：原样模式、纯 ASCII 忽略大小写模式各扫描一次。后者仅含
//!   ASCII 字节与 UTF-16 零字节，可用自动机内置的 ASCII 大小写等价表，无须
//!   复制并折叠缓冲区。非 ASCII 关键词仍按原样及整词大小写变体精确匹配，
//!   不折叠 GBK 尾字节或 UTF-16 高字节，见 [`super::keyword_literals`]。
//! - **流式分块**：调用方把上一块末尾的 carry 窗口（见 [`MatcherSet::carry_len`]）
//!   拼在新块前一起扫描。完全落在 carry 区内的命中是上一块已报告过的重复，
//!   丢弃；跨越 carry / 新块边界的命中（start+len > carry_len）只在本块出现，
//!   保留——保证每个命中恰好报告一次。
//! - **跨编码按区间去重**：同一 `(start, len)` 可能被多种编码的自动机同时命中
//!   （纯 ASCII 关键词在 ASCII 与 UTF-8 下字节完全相同）；按
//!   [`SearchEncoding::ALL`] 优先级只保留最高优先级编码的命中。关键词与正则
//!   之间互不去重（同一区间被两种通道命中即两条不同模式的记录）。

use std::collections::HashSet;

use super::keyword_literals::LiteralMatchers;
pub use super::keyword_literals::MatchScratch;

/// 关键词的一种字节编码。优先级顺序见 [`SearchEncoding::ALL`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchEncoding {
    Ascii,
    Utf8,
    Utf16Le,
    Utf16Be,
    Gbk,
}

impl SearchEncoding {
    /// 全部编码，按优先级排列：同一字节区间被多种编码命中时，靠前的编码获胜。
    pub const ALL: [SearchEncoding; 5] = [
        SearchEncoding::Ascii,
        SearchEncoding::Utf8,
        SearchEncoding::Utf16Le,
        SearchEncoding::Utf16Be,
        SearchEncoding::Gbk,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ascii => "ASCII",
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Gbk => "GBK",
        }
    }

    /// 解析编码页标签：ASCII / UTF-8 / GBK / UTF-16LE / UTF-16BE，大小写不敏感。
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "ascii" => Some(Self::Ascii),
            "utf-8" | "utf8" => Some(Self::Utf8),
            "utf-16le" | "utf16le" => Some(Self::Utf16Le),
            "utf-16be" | "utf16be" => Some(Self::Utf16Be),
            "gbk" => Some(Self::Gbk),
            _ => None,
        }
    }

    /// 把文本编码为该编码下的字节。ASCII 仅对全 ASCII 输入字节精确 —— 构建时
    /// 已用严格检查过滤，非 ASCII 关键词不会进入 ASCII 自动机（此处走 UTF-8
    /// 编码路径，对全 ASCII 输入与 ASCII 字节完全相同）。
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Ascii | Self::Utf8 => encoding_rs::UTF_8.encode(text).0.into_owned(),
            Self::Utf16Le => utf16_bytes(text, true),
            Self::Utf16Be => utf16_bytes(text, false),
            Self::Gbk => encoding_rs::GBK.encode(text).0.into_owned(),
        }
    }

    /// 把字节解码回字符串（命中上下文预览、正则文本通道用）。残缺序列以替换
    /// 字符容忍，不 panic：半个多字节字符的尾巴、奇数长度的 UTF-16 字节、残缺
    /// 代理对都安全。不做 BOM 嗅探：以 `FF FE` / `EF BB BF` 开头的块仍按本编码
    /// 解码，字符与原字节一一对应（正则通道按字符回推字节偏移依赖这一点）。
    pub fn decode_to_string(self, bytes: &[u8]) -> String {
        match self {
            Self::Ascii | Self::Utf8 => encoding_rs::UTF_8
                .decode_without_bom_handling(bytes)
                .0
                .into_owned(),
            Self::Utf16Le => decode_utf16_bytes(bytes, true),
            Self::Utf16Be => decode_utf16_bytes(bytes, false),
            Self::Gbk => encoding_rs::GBK
                .decode_without_bom_handling(bytes)
                .0
                .into_owned(),
        }
    }
}

/// UTF-16 文本 → 字节（`little_endian` 决定字节序，按码元逐个展开）。
fn utf16_bytes(text: &str, little_endian: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        let bytes = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        out.extend_from_slice(&bytes);
    }
    out
}

/// UTF-16 字节 → 文本；奇数尾字节丢弃，残缺代理对以 U+FFFD 填充。
fn decode_utf16_bytes(bytes: &[u8], little_endian: bool) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        })
        .collect::<Vec<_>>();
    char::decode_utf16(units)
        .map(|result| result.unwrap_or('\u{FFFD}'))
        .collect()
}

/// 一次命中。`start` 是缓冲区相对偏移（buffer = carry ∪ 新块），文件绝对偏移
/// 由调用方自行换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteHit {
    pub start: usize,
    pub len: usize,
    pub keyword_index: usize,
    pub encoding: SearchEncoding,
}

/// 一组关键词（+可选正则）的多编码匹配器（内部不透明）。命中流的模式
/// 下标统一编号：关键词在前（去重后）、正则在后。
pub struct MatcherSet {
    literals: LiteralMatchers,
    /// 正则通道，模式下标接在关键词之后。
    regexes: Option<crate::core::search::keyword::keyword_regex::RegexMatcherSet>,
    /// 去重后的关键词数（正则下标换算的基址）。
    keyword_count: usize,
}

impl MatcherSet {
    /// 全编码、无正则的兼容入口（等价 `build_plan(keywords, &[], …, ALL)`）。
    pub fn build(keywords: &[String], case_sensitive: bool) -> Self {
        let (set, failures) = build_matchers(keywords, &[], case_sensitive, &SearchEncoding::ALL);
        debug_assert!(failures.is_empty(), "no regexes → no compile failures");
        let _ = failures;
        set
    }

    /// 按选定编码页构建「关键词 + 正则」合并匹配器。
    /// 正则编译失败逐条汇总返回（用户输入错误，
    /// 由调用方拒绝整次搜索）；关键词自动机构建失败仅 warn 降级。
    pub fn build_plan(
        keywords: &[String],
        regexes: &[String],
        case_sensitive: bool,
        encodings: &[SearchEncoding],
    ) -> Result<Self, String> {
        let (set, failures) = build_matchers(keywords, regexes, case_sensitive, encodings);
        if failures.is_empty() {
            Ok(set)
        } else {
            Err(failures.join("；"))
        }
    }

    /// 去重后的关键词数；命中流的模式下标 `>= keyword_count` 即为正则
    /// （`index - keyword_count` 指回正则列表）。
    pub fn keyword_count(&self) -> usize {
        self.keyword_count
    }

    /// 所有编码自动机中最长的模式字节数；无模式时返回 0。
    pub fn max_pattern_len(&self) -> usize {
        self.literals.max_pattern_len
    }

    /// 流式扫描的 carry 窗口大小：关键词侧取 `max_pattern_len - 1`，正则
    /// 通道取固定 [`REGEX_CARRY_BYTES`]（变长正则无最长模式上界）。
    pub fn carry_len(&self) -> usize {
        let literal = self.max_pattern_len().saturating_sub(1);
        if self.regexes.is_some() {
            literal.max(crate::core::search::keyword::keyword_regex::REGEX_CARRY_BYTES)
        } else {
            literal
        }
    }

    /// 扫描一个缓冲区（carry ∪ 新块）。`carry_len` 是 carry 字节数：
    /// 完全落在 carry 内的命中（start + len <= carry_len）是上一块的重复报告，
    /// 丢弃；跨越 carry / 新块边界的命中是新出现的，保留。跨编码按
    /// [`SearchEncoding::ALL`] 优先级对 `(start, len)` 去重。大小写不敏感时
    /// 由合并自动机直接匹配纯 ASCII 关键词的任意大小写组合。
    /// 正则通道（若有）在关键词之后追加，与关键词互不去重（同一区间被
    /// 关键词和正则同时命中时各报一条——它们是不同模式）。
    pub fn scan_chunk(&self, buffer: &[u8], carry_len: usize, out: &mut Vec<ByteHit>) {
        self.scan_chunk_with(buffer, carry_len, out, &mut MatchScratch::default());
    }

    /// [`Self::scan_chunk`]，候选命中与去重集合放在调用方逐块复用的 `scratch`
    /// 里——流式扫描每块（小文件密集时即每个文件）都要用到它们。
    pub fn scan_chunk_with(
        &self,
        buffer: &[u8],
        carry_len: usize,
        out: &mut Vec<ByteHit>,
        scratch: &mut MatchScratch,
    ) {
        self.literals.scan(buffer, carry_len, out, scratch);
        if let Some(regexes) = &self.regexes {
            regexes.scan_chunk(buffer, carry_len, out);
        }
    }
}

/// 手动 Debug：自动机/正则内部结构无意义，打印聚合计数即可。
impl std::fmt::Debug for MatcherSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MatcherSet")
            .field("has_regexes", &self.regexes.is_some())
            .field("keyword_count", &self.keyword_count)
            .field("max_pattern_len", &self.max_pattern_len())
            .finish()
    }
}

/// `MatcherSet` 的实际构建体（`build`/`build_plan` 共用）。返回构建结果与
/// 正则编译失败列表。编码页按 [`SearchEncoding::ALL`] 优先级归一化去重；
/// 关键词字节变体交给合并自动机，正则通道保留原有的多编码语义。
fn build_matchers(
    keywords: &[String],
    regexes: &[String],
    case_sensitive: bool,
    encodings: &[SearchEncoding],
) -> (MatcherSet, Vec<String>) {
    let mut deduped: Vec<String> = Vec::new();
    let mut seen_keywords = HashSet::new();
    for keyword in keywords {
        if keyword.trim().is_empty() {
            continue;
        }
        if seen_keywords.insert(keyword.clone()) {
            deduped.push(keyword.clone());
        }
    }
    let mut deduped_regexes: Vec<String> = Vec::new();
    let mut seen_regexes = HashSet::new();
    for pattern in regexes {
        if pattern.trim().is_empty() {
            continue;
        }
        if seen_regexes.insert(pattern.clone()) {
            deduped_regexes.push(pattern.clone());
        }
    }
    let encodings = normalize_encodings(encodings);

    let literals = LiteralMatchers::build(&deduped, &encodings, case_sensitive);
    let regex_matchers = if deduped_regexes.is_empty() {
        None
    } else {
        let (set, failures) = crate::core::search::keyword::keyword_regex::RegexMatcherSet::build(
            &deduped_regexes,
            case_sensitive,
            &encodings,
            deduped.len(),
        );
        if !failures.is_empty() {
            return (empty_matcher_set(), failures);
        }
        if set.is_empty() {
            // 选定编码下没有任何可表达的正则（如仅选 ASCII 却全是非 ASCII 模式）。
            tracing::warn!("keyword_search: regex matchers empty for selected encodings");
            None
        } else {
            Some(set)
        }
    };
    let keyword_count = deduped.len();
    (
        MatcherSet {
            literals,
            regexes: regex_matchers,
            keyword_count,
        },
        Vec::new(),
    )
}

/// 空匹配器（所有正则编译失败时的占位，调用方拿到 Err 后不会使用它）。
fn empty_matcher_set() -> MatcherSet {
    MatcherSet {
        literals: LiteralMatchers::default(),
        regexes: None,
        keyword_count: 0,
    }
}

/// 编码页列表按 [`SearchEncoding::ALL`] 优先级排序并去重（跨编码区间去重
/// 依赖该优先级；空列表视为全部编码）。
fn normalize_encodings(encodings: &[SearchEncoding]) -> Vec<SearchEncoding> {
    if encodings.is_empty() {
        return SearchEncoding::ALL.to_vec();
    }
    SearchEncoding::ALL
        .iter()
        .copied()
        .filter(|encoding| encodings.contains(encoding))
        .collect()
}

#[cfg(test)]
#[path = "keyword_search_tests.rs"]
mod tests;
