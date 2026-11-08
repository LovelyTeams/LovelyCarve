//! 正则搜索通道:多编码匹配与字节偏移映射。
//!
//! 关键词通道(`keyword_search::MatcherSet`)是字面值的 Aho-Corasick；本模块补上
//! 正则模式，两条通道在 [`crate::search::keyword::keyword_search::MatcherSet`] 内合并扫描。
//!
//! - **ASCII / UTF-8**：模式文本的字节就是这两种编码下的目标字节，直接在
//!   原始缓冲区上跑 `regex::bytes::Regex`，命中即字节偏移，正则语法完整
//!   保留。纯 ASCII 模式在两个通道上是同一台正则，只建 ASCII 那台。
//! - **GBK / UTF-16LE / UTF-16BE**：多字节编码下正则元字符与编码字节穿插
//!   （UTF-16 高字节 0x00；GBK 字符类 `[码马]` 按字节拆开会变成单字节
//!   集合），字节级正则不可表达。改为把缓冲区按该编码有损解码成文本、
//!   在文本上跑 `regex::Regex`，再把字符区间映射回原字节偏移：
//!   UTF-16 按码元计数（增补平面 2 码元，其余 1，字节 = 码元 × 2），
//!   GBK 按解码器实际消耗的字节数（[`crate::search::keyword::gbk_units`]：单字节 0x80 的
//!   「€」、四字节序列与各类错误的字节数都不能从字符本身推出）。同一编码的
//!   全部正则共用一次解码。
//! - **必含字面量预筛**：解码整块是文本通道的主要开销。从正则的语法树推出
//!   「任何命中都至少包含其中之一」的字面量集合（[`required_literals`]），按
//!   目标编码编码后先在原始字节上找；一个都不出现的块不可能命中，跳过解码与
//!   匹配。只用可逐字节对应回原文的字面量：UTF-16 任意字符（替换字符除外），
//!   GBK 只取 ASCII 片段——非 ASCII 字符在 GB18030 解码器下不止一种字节形式。
//! - **跨块语义**：与关键词通道同一条 carry 规则——完全落在 carry 区内
//!   (`start + len <= carry_len`) 的命中上一块已报告，丢弃；跨越边界的命中
//!   本块首次可见，保留。变长正则没有「最长模式」上界，carry 取固定窗口
//!   [`REGEX_CARRY_BYTES`]；超过该窗口长度的跨块匹配可能漏报（流式正则的
//!   固有限制）。
//! - **零长命中丢弃**（`a*` 空匹配对偏移定位无意义）；重叠语义为 regex 默认
//!   的最左优先非重叠（与关键词通道的重叠全报不同，正则业界通行）。

use std::collections::HashSet;

use aho_corasick::AhoCorasick;
use regex::bytes::Regex as ByteRegex;
use regex::Regex as TextRegex;
use regex_syntax::hir::{Class, Hir, HirKind};

use crate::search::keyword::gbk_units::GbkUnits;
use crate::search::keyword::keyword_search::{ByteHit, SearchEncoding};

/// 正则通道的固定 carry 窗口（字节）。64 KiB 覆盖绝大多数跨块正则命中，
/// 相对 4 MiB 扫描块的重复扫描开销约 1.5%。
pub const REGEX_CARRY_BYTES: usize = 64 * 1024;

/// 必含字面量集合的元素上限（大小写变体按字符类展开后的组合数也受它约束）。
const MAX_REQUIRED_LITERALS: usize = 64;
/// 单个必含字面量的字符数上限。
const MAX_REQUIRED_LITERAL_CHARS: usize = 32;
/// 按字面量展开的字符类大小上限（覆盖 `(?i)` 的大小写变体）。
const MAX_CLASS_LITERALS: usize = 8;

/// ASCII / UTF-8 通道的一台正则：直接在原始字节上匹配。
struct ByteMatcher {
    encoding: SearchEncoding,
    regex_index: usize,
    re: ByteRegex,
}

/// 一种文本通道编码下的全部正则，每块共用一次解码。
struct TextGroup {
    encoding: SearchEncoding,
    matchers: Vec<TextMatcher>,
}

/// 文本通道的一台正则。
struct TextMatcher {
    regex_index: usize,
    re: TextRegex,
    /// 必含字面量的本编码字节序列；原始缓冲区一个都不含时本块不可能命中。
    /// `None` = 推不出可用字面量，每块都解码匹配。
    prefilter: Option<AhoCorasick>,
}

/// 一组正则的多编码匹配器。
pub struct RegexMatcherSet {
    byte_matchers: Vec<ByteMatcher>,
    /// 按 [`SearchEncoding::ALL`] 优先级排列（跨编码去重保留先到者）。
    text_groups: Vec<TextGroup>,
    pattern_base: usize,
}

impl RegexMatcherSet {
    /// 编译一组正则（调用方已去重）× 选定编码。编译失败的正则逐条返回错误
    /// 信息（按 `正则: 错误` 去重——同一正则在多个编码下的失败只报一次）；
    /// 其余正则仍可扫描，调用方决定是否把失败当作用户输入错误拒绝整次搜索。
    pub fn build(
        patterns: &[String],
        case_sensitive: bool,
        encodings: &[SearchEncoding],
        pattern_base: usize,
    ) -> (Self, Vec<String>) {
        let flags = format!("{}(?s)", if case_sensitive { "" } else { "(?i)" });
        let mut byte_matchers = Vec::new();
        let mut text_groups: Vec<TextGroup> = Vec::new();
        let mut failures = Vec::new();
        let mut seen_failures = HashSet::new();
        let ascii_selected = encodings.contains(&SearchEncoding::Ascii);
        let required: Vec<Option<Vec<String>>> = patterns
            .iter()
            .map(|pattern| required_literals(&format!("{flags}{pattern}")))
            .collect();
        for encoding in encodings {
            for (regex_index, pattern) in patterns.iter().enumerate() {
                let source = format!("{flags}{pattern}");
                let built: Result<(), String> = match encoding {
                    // ASCII 通道只接受 ASCII 模式；非 ASCII 模式在该编码下无字节
                    // 表达，由 UTF-8 / GBK / UTF-16 侧覆盖。
                    SearchEncoding::Ascii if !pattern.is_ascii() => Ok(()),
                    // 纯 ASCII 模式在 UTF-8 通道上与 ASCII 通道是同一台正则、同一批
                    // 命中（跨编码去重会全部丢弃），不再重复扫描。
                    SearchEncoding::Utf8 if ascii_selected && pattern.is_ascii() => Ok(()),
                    SearchEncoding::Ascii | SearchEncoding::Utf8 => ByteRegex::new(&source)
                        .map(|re| {
                            byte_matchers.push(ByteMatcher {
                                encoding: *encoding,
                                regex_index,
                                re,
                            });
                        })
                        .map_err(|err| err.to_string()),
                    SearchEncoding::Gbk | SearchEncoding::Utf16Le | SearchEncoding::Utf16Be => {
                        TextRegex::new(&source)
                            .map(|re| {
                                let prefilter = required[regex_index]
                                    .as_deref()
                                    .and_then(|literals| encoded_prefilter(literals, *encoding));
                                let matcher = TextMatcher {
                                    regex_index,
                                    re,
                                    prefilter,
                                };
                                match text_groups
                                    .iter_mut()
                                    .find(|group| group.encoding == *encoding)
                                {
                                    Some(group) => group.matchers.push(matcher),
                                    None => text_groups.push(TextGroup {
                                        encoding: *encoding,
                                        matchers: vec![matcher],
                                    }),
                                }
                            })
                            .map_err(|err| err.to_string())
                    }
                };
                if let Err(message) = built {
                    let failure = format!("{pattern}: {message}");
                    if seen_failures.insert(failure.clone()) {
                        failures.push(failure);
                    }
                }
            }
        }
        (
            Self {
                byte_matchers,
                text_groups,
                pattern_base,
            },
            failures,
        )
    }

    /// 是否一台匹配器都没有构建（编码集与正则集无可交叠表达）。
    pub fn is_empty(&self) -> bool {
        self.byte_matchers.is_empty() && self.text_groups.is_empty()
    }

    /// 扫描一个缓冲区（carry ∪ 新块），追加正则命中。同一 `(start, len,
    /// 正则)` 只报一次（跨编码去重）；不同正则命中同一区间各自成条。
    /// 空匹配丢弃。
    pub fn scan_chunk(&self, buffer: &[u8], carry_len: usize, out: &mut Vec<ByteHit>) {
        let mut seen: HashSet<(usize, usize, usize)> = HashSet::new();
        for matcher in &self.byte_matchers {
            let keyword_index = self.pattern_base + matcher.regex_index;
            for hit in matcher.re.find_iter(buffer) {
                let start = hit.start();
                let len = hit.end() - start;
                if len == 0 || start + len <= carry_len {
                    continue;
                }
                if seen.insert((start, len, keyword_index)) {
                    out.push(ByteHit {
                        start,
                        len,
                        keyword_index,
                        encoding: matcher.encoding,
                    });
                }
            }
        }
        for group in &self.text_groups {
            let mut decoded: Option<String> = None;
            for matcher in &group.matchers {
                if let Some(prefilter) = &matcher.prefilter {
                    if !prefilter.is_match(buffer) {
                        continue;
                    }
                }
                let text = decoded.get_or_insert_with(|| group.encoding.decode_to_string(buffer));
                let keyword_index = self.pattern_base + matcher.regex_index;
                // find_iter 输出非重叠、按起点排序的区间。
                let spans: Vec<(usize, usize)> = matcher
                    .re
                    .find_iter(text)
                    .map(|hit| (hit.start(), hit.end()))
                    .filter(|(start, end)| end > start)
                    .collect();
                if spans.is_empty() {
                    continue;
                }
                let mut emit = |start: usize, len: usize| {
                    // 映射逐字符精确；越界只可能来自解码与字节账不一致，丢弃而不是
                    // 报出错位的偏移。
                    if start + len <= carry_len || start + len > buffer.len() {
                        return;
                    }
                    if seen.insert((start, len, keyword_index)) {
                        out.push(ByteHit {
                            start,
                            len,
                            keyword_index,
                            encoding: group.encoding,
                        });
                    }
                };
                if group.encoding == SearchEncoding::Gbk {
                    let mut units = GbkUnits::new(buffer);
                    map_char_spans(text, &spans, |_| units.next().unwrap_or(1), &mut emit);
                } else {
                    map_char_spans(text, &spans, |ch| ch.len_utf16() * 2, &mut emit);
                }
            }
        }
    }
}

/// 把必含字面量编码成 `encoding` 下的原始字节预筛。每个字面量取能逐字节对应
/// 回原文的最长片段（片段是字面量的子串，含字面量的文本必含片段，预筛因此
/// 仍是命中的必要条件）：UTF-16 去掉替换字符，GBK 只取 ASCII。任何一个字面量
/// 取不出片段时整个集合作废（不能丢掉集合里的成员）。
fn encoded_prefilter(literals: &[String], encoding: SearchEncoding) -> Option<AhoCorasick> {
    let mut needles: Vec<Vec<u8>> = Vec::new();
    for literal in literals {
        let keep: fn(char) -> bool = match encoding {
            SearchEncoding::Gbk => |ch| ch.is_ascii(),
            SearchEncoding::Utf16Le | SearchEncoding::Utf16Be => |ch| ch != '\u{FFFD}',
            SearchEncoding::Ascii | SearchEncoding::Utf8 => return None,
        };
        let fragment = longest_run(literal, keep);
        if fragment.is_empty() {
            return None;
        }
        let bytes = encoding.encode(fragment);
        if !needles.contains(&bytes) {
            needles.push(bytes);
        }
    }
    match AhoCorasick::new(&needles) {
        Ok(prefilter) => Some(prefilter),
        Err(err) => {
            tracing::warn!("keyword_regex: {encoding:?} prefilter build failed: {err}");
            None
        }
    }
}

/// `text` 中全部字符满足 `keep` 的最长连续片段。
fn longest_run(text: &str, keep: fn(char) -> bool) -> &str {
    let mut best = "";
    let mut run_start: Option<usize> = None;
    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        let at_end = index == text.len();
        match (at_end || !keep(ch), run_start) {
            (true, Some(start)) => {
                if index - start > best.len() {
                    best = &text[start..index];
                }
                run_start = None;
            }
            (false, None) => run_start = Some(index),
            (true, None) | (false, Some(_)) => {}
        }
    }
    best
}

/// 一段正则可匹配文本的字面量刻画。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TextReq {
    /// 匹配文本恰是其中之一（可与相邻节点拼接成更长的字面量）。
    Exact(Vec<String>),
    /// 匹配文本至少包含其中之一。
    Contains(Vec<String>),
    /// 推不出。
    Unknown,
}

/// 正则的必含字面量：任何命中文本都至少包含其中之一。推不出（或集合里有
/// 空串）时返回 `None`，此时文本通道不做预筛。
fn required_literals(pattern: &str) -> Option<Vec<String>> {
    let hir = regex_syntax::parse(pattern).ok()?;
    let literals = match text_req(&hir) {
        TextReq::Exact(literals) | TextReq::Contains(literals) => literals,
        TextReq::Unknown => return None,
    };
    (!literals.is_empty() && literals.iter().all(|literal| !literal.is_empty())).then_some(literals)
}

fn text_req(hir: &Hir) -> TextReq {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => TextReq::Exact(vec![String::new()]),
        HirKind::Literal(literal) => match std::str::from_utf8(&literal.0) {
            Ok(text) => TextReq::Exact(vec![text.to_string()]),
            Err(_) => TextReq::Unknown,
        },
        HirKind::Class(Class::Unicode(class)) => {
            let mut chars = Vec::new();
            for range in class.ranges() {
                for ch in range.start()..=range.end() {
                    if chars.len() == MAX_CLASS_LITERALS {
                        return TextReq::Unknown;
                    }
                    chars.push(ch.to_string());
                }
            }
            if chars.is_empty() {
                TextReq::Unknown
            } else {
                TextReq::Exact(chars)
            }
        }
        HirKind::Class(Class::Bytes(_)) => TextReq::Unknown,
        HirKind::Capture(capture) => text_req(&capture.sub),
        HirKind::Repetition(repetition) => {
            if repetition.min == 0 {
                return TextReq::Unknown;
            }
            match text_req(&repetition.sub) {
                TextReq::Exact(literals) if repetition.max == Some(1) => TextReq::Exact(literals),
                TextReq::Exact(literals) | TextReq::Contains(literals) => {
                    TextReq::Contains(literals)
                }
                TextReq::Unknown => TextReq::Unknown,
            }
        }
        HirKind::Concat(children) => concat_req(children),
        HirKind::Alternation(children) => alternation_req(children),
    }
}

/// 串接：相邻的「恰是」节点按笛卡尔积拼成更长的字面量；其余节点把连续段切开。
/// 至少一次的重复（`x+`、`x{2,}`）文本以子节点文本开头、也以其结尾，因此能
/// 收尾前一段、再开启后一段。整串都是「恰是」时结果仍是「恰是」，否则在各段
/// 候选中取最有选择性的一个作为「包含」。
fn concat_req(children: &[Hir]) -> TextReq {
    let mut candidates: Vec<Vec<String>> = Vec::new();
    let mut run = vec![String::new()];
    let mut exact = true;
    for child in children {
        if let HirKind::Repetition(repetition) = child.kind() {
            if repetition.min >= 1 && repetition.max != Some(1) {
                if let TextReq::Exact(literals) = text_req(&repetition.sub) {
                    extend_run(&mut run, &literals, &mut candidates);
                    candidates.push(std::mem::replace(&mut run, literals));
                    exact = false;
                    continue;
                }
            }
        }
        match text_req(child) {
            TextReq::Exact(literals) => {
                if !extend_run(&mut run, &literals, &mut candidates) {
                    exact = false;
                }
            }
            TextReq::Contains(literals) => {
                candidates.push(std::mem::replace(&mut run, vec![String::new()]));
                candidates.push(literals);
                exact = false;
            }
            TextReq::Unknown => {
                candidates.push(std::mem::replace(&mut run, vec![String::new()]));
                exact = false;
            }
        }
    }
    if exact {
        return TextReq::Exact(run);
    }
    candidates.push(run);
    candidates
        .into_iter()
        .filter(|literals| {
            !literals.is_empty() && literals.iter().all(|literal| !literal.is_empty())
        })
        .max_by_key(|literals| {
            let shortest = literals
                .iter()
                .map(|literal| literal.chars().count())
                .min()
                .unwrap_or(0);
            (shortest, std::cmp::Reverse(literals.len()))
        })
        .map_or(TextReq::Unknown, TextReq::Contains)
}

/// 连续段乘上一个「恰是」节点。组合数或字面量长度超限时把当前段收为候选、
/// 从该节点重新开始并返回 `false`（连续段断开，整串不再是「恰是」）。
fn extend_run(
    run: &mut Vec<String>,
    literals: &[String],
    candidates: &mut Vec<Vec<String>>,
) -> bool {
    let too_many = run.len().saturating_mul(literals.len()) > MAX_REQUIRED_LITERALS;
    let too_long = run
        .iter()
        .map(|literal| literal.chars().count())
        .max()
        .unwrap_or(0)
        + literals
            .iter()
            .map(|literal| literal.chars().count())
            .max()
            .unwrap_or(0)
        > MAX_REQUIRED_LITERAL_CHARS;
    if too_many || too_long {
        candidates.push(std::mem::replace(run, literals.to_vec()));
        return false;
    }
    let mut product = Vec::with_capacity(run.len() * literals.len());
    for prefix in run.iter() {
        for literal in literals {
            let joined = format!("{prefix}{literal}");
            if !product.contains(&joined) {
                product.push(joined);
            }
        }
    }
    *run = product;
    true
}

/// 选择：每个分支都能刻画时取并集（全是「恰是」时仍是「恰是」）。
fn alternation_req(children: &[Hir]) -> TextReq {
    let mut union: Vec<String> = Vec::new();
    let mut exact = true;
    for child in children {
        let literals = match text_req(child) {
            TextReq::Exact(literals) => literals,
            TextReq::Contains(literals) => {
                exact = false;
                literals
            }
            TextReq::Unknown => return TextReq::Unknown,
        };
        for literal in literals {
            if !union.contains(&literal) {
                union.push(literal);
            }
        }
        if union.len() > MAX_REQUIRED_LITERALS {
            return TextReq::Unknown;
        }
    }
    if exact {
        TextReq::Exact(union)
    } else {
        TextReq::Contains(union)
    }
}

/// 把解码文本里的命中区间（非重叠、按起点排序；`regex::Match` 语义下的
/// **解码文本（UTF-8）字节偏移**）映射回原编码字节偏移：单次遍历，按
/// `char_bytes` 逐字符累计原编码字节长度（按文本顺序对每个字符调用一次）。
fn map_char_spans(
    decoded: &str,
    spans: &[(usize, usize)],
    mut char_bytes: impl FnMut(char) -> usize,
    mut emit: impl FnMut(usize, usize),
) {
    let mut next_span = 0usize;
    // 当前打开区间（起始原编码字节偏移, 区间下标）；非重叠保证同时至多一个。
    let mut open: Option<(usize, usize)> = None;
    let mut byte_index = 0usize;
    for (decoded_pos, ch) in decoded.char_indices() {
        while next_span < spans.len() && spans[next_span].0 == decoded_pos {
            open = Some((byte_index, next_span));
            next_span += 1;
        }
        byte_index += char_bytes(ch);
        if let Some((start, span_index)) = open {
            if spans[span_index].1 == decoded_pos + ch.len_utf8() {
                emit(start, byte_index - start);
                open = None;
            }
        }
    }
}

#[cfg(test)]
#[path = "keyword_regex_tests.rs"]
mod tests;
