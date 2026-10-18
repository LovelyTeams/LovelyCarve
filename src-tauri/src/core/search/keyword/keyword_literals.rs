//! 多编码关键词在编译期合并：原样模式、纯 ASCII 忽略大小写模式各扫描一遍。
//!
//! 后者的编码字节只含 ASCII 与 UTF-16 的零字节，可安全使用自动机的 ASCII
//! 大小写等价表；非 ASCII 关键词始终走原样通道，避免折叠 GBK 尾字节或
//! UTF-16 高字节。相同字节模式只保留最高优先级编码的来源。

use std::collections::HashSet;

use super::keyword_search::{ByteHit, SearchEncoding};

const SCRATCH_RANGES_KEEP: usize = 1024;

/// 每块复用命中与区间去重缓冲，无须复制并折叠整个扫描窗口。
#[derive(Debug, Default)]
pub struct MatchScratch {
    hits: Vec<LiteralHit>,
    seen_ranges: HashSet<(usize, usize)>,
}

#[derive(Debug)]
struct LiteralHit {
    hit: ByteHit,
    priority: usize,
}

struct PatternSource {
    encoding: SearchEncoding,
    keyword_index: usize,
    priority: usize,
}

struct LiteralMatcher {
    ac: aho_corasick::AhoCorasick,
    sources: Vec<PatternSource>,
}

/// 原样与忽略 ASCII 大小写的两组多编码自动机。
#[derive(Default)]
pub(super) struct LiteralMatchers {
    exact: Option<LiteralMatcher>,
    caseless: Option<LiteralMatcher>,
    pub max_pattern_len: usize,
}

/// 生成一台自动机的 `(字节模式, 关键词下标)` 列表。模式按编码字节去重
/// （重复字节只留第一个，保持插入顺序）。
///
/// `folded` 时只收纯 ASCII 关键词，模式是小写 ASCII 的编码字节，由自动机的
/// ASCII 大小写等价表匹配；否则收全部关键词，大小写不敏感时对非 ASCII 关键词追加
/// `to_lowercase()` / `to_uppercase()` 变体（如「ÄPFEL」→「äpfel」；混合大小写
/// 不回退，保持字节精确）。
///
/// 大小写不敏感时，含字母的纯 ASCII 关键词不进常规自动机：ASCII 折叠是
/// 1:1 字节变换，折叠自动机（含该关键词的小写模式）覆盖原样命中且更全
/// （任意大小写组合），原样侧扫描纯粹是逐字节重复（区间去重会把重复
/// 命中丢掉，但扫描本身白跑）。无字母的纯 ASCII 关键词（如「123」）折叠是
/// 恒等变换、没有折叠模式，必须留在原样侧。
fn encode_patterns(
    deduped: &[String],
    encoding: SearchEncoding,
    case_sensitive: bool,
    folded: bool,
) -> (Vec<Vec<u8>>, Vec<usize>) {
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    let mut pattern_keyword: Vec<usize> = Vec::new();
    let mut seen_patterns = HashSet::new();
    for (keyword_index, keyword) in deduped.iter().enumerate() {
        let mut variants: Vec<String> = Vec::new();
        if folded {
            // 只有纯 ASCII 关键词有折叠表达；无字母的关键词折叠是恒等变换，
            // 跳过（原样模式已由常规自动机覆盖）。
            if keyword.is_ascii() && keyword.bytes().any(|byte| byte.is_ascii_alphabetic()) {
                variants.push(keyword.to_ascii_lowercase());
            }
        } else {
            // 含字母的纯 ASCII 关键词归折叠自动机（见函数注释）；其余
            // （非 ASCII、无字母符号）留在原样侧。
            if !case_sensitive
                && keyword.is_ascii()
                && keyword.bytes().any(|byte| byte.is_ascii_alphabetic())
            {
                continue;
            }
            variants.push(keyword.clone());
            if !case_sensitive && !keyword.is_ascii() {
                variants.push(keyword.to_lowercase());
                variants.push(keyword.to_uppercase());
            }
        }
        for variant in variants {
            // 非 ASCII 变体在 ASCII 自动机里没有字节表达（encode 会拿 UTF-8
            // 字节顶替，产生错误命中），严格检查后跳过该变体。
            if encoding == SearchEncoding::Ascii && !variant.is_ascii() {
                continue;
            }
            let bytes = encoding.encode(&variant);
            if bytes.is_empty() {
                continue;
            }
            if seen_patterns.insert(bytes.clone()) {
                patterns.push(bytes);
                pattern_keyword.push(keyword_index);
            }
        }
    }
    (patterns, pattern_keyword)
}

impl LiteralMatcher {
    fn build(
        keywords: &[String],
        encodings: &[SearchEncoding],
        case_sensitive: bool,
        folded: bool,
    ) -> Option<Self> {
        let mut patterns = Vec::new();
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        for (rank, &encoding) in encodings.iter().enumerate() {
            let (encoded, keyword_indices) =
                encode_patterns(keywords, encoding, case_sensitive, folded);
            for (bytes, keyword_index) in encoded.into_iter().zip(keyword_indices) {
                if seen.insert(bytes.clone()) {
                    patterns.push(bytes);
                    sources.push(PatternSource {
                        encoding,
                        keyword_index,
                        // 与逐编码扫描一致：同一编码先原样、再忽略大小写。
                        priority: rank * 2 + usize::from(folded),
                    });
                }
            }
        }
        if patterns.is_empty() {
            return None;
        }
        let mut builder = aho_corasick::AhoCorasickBuilder::new();
        builder
            .match_kind(aho_corasick::MatchKind::Standard)
            .ascii_case_insensitive(folded);
        let ac = match builder
            .kind(Some(aho_corasick::AhoCorasickKind::DFA))
            .build(&patterns)
            .or_else(|_| builder.kind(None).build(&patterns))
        {
            Ok(ac) => ac,
            Err(err) => {
                tracing::warn!("keyword_search: combined matcher build failed: {err}");
                return None;
            }
        };
        Some(Self { ac, sources })
    }

    fn hits<'a>(
        &'a self,
        buffer: &'a [u8],
        carry_len: usize,
    ) -> impl Iterator<Item = LiteralHit> + 'a {
        self.ac
            .find_overlapping_iter(buffer)
            .filter_map(move |found| {
                if found.end() <= carry_len {
                    return None;
                }
                let source = &self.sources[found.pattern().as_usize()];
                Some(LiteralHit {
                    hit: ByteHit {
                        start: found.start(),
                        len: found.end() - found.start(),
                        keyword_index: source.keyword_index,
                        encoding: source.encoding,
                    },
                    priority: source.priority,
                })
            })
    }
}

impl LiteralMatchers {
    pub fn build(keywords: &[String], encodings: &[SearchEncoding], case_sensitive: bool) -> Self {
        let exact = LiteralMatcher::build(keywords, encodings, case_sensitive, false);
        let caseless = if case_sensitive {
            None
        } else {
            LiteralMatcher::build(keywords, encodings, case_sensitive, true)
        };
        let max_pattern_len = exact
            .iter()
            .chain(caseless.iter())
            .map(|matcher| matcher.ac.max_pattern_len())
            .max()
            .unwrap_or(0);
        Self {
            exact,
            caseless,
            max_pattern_len,
        }
    }

    pub fn scan(
        &self,
        buffer: &[u8],
        carry_len: usize,
        out: &mut Vec<ByteHit>,
        scratch: &mut MatchScratch,
    ) {
        scratch.hits.clear();
        if scratch.seen_ranges.capacity() > SCRATCH_RANGES_KEEP {
            scratch.seen_ranges = HashSet::new();
        } else {
            scratch.seen_ranges.clear();
        }
        // 单通道的字节模式在编译时已经去重，同一区间不可能命中两个模式，
        // 无须额外收集候选或逐命中哈希。纯 ASCII 词与区分大小写搜索走此路。
        if self.exact.is_none() || self.caseless.is_none() {
            let start = out.len();
            for matcher in self.exact.iter().chain(self.caseless.iter()) {
                out.extend(matcher.hits(buffer, carry_len).map(|found| found.hit));
            }
            out[start..].sort_by_key(|hit| hit.encoding as usize);
            return;
        }
        for matcher in self.exact.iter().chain(self.caseless.iter()) {
            scratch.hits.extend(matcher.hits(buffer, carry_len));
        }
        // 跨通道碰撞也按编码优先级决定归属，不能让低优先级原样命中抢在
        // 高优先级忽略大小写命中之前。稳定排序保留同编码内的重叠命中序。
        scratch.hits.sort_by_key(|found| found.priority);
        for found in &scratch.hits {
            if scratch.seen_ranges.insert((found.hit.start, found.hit.len)) {
                out.push(found.hit);
            }
        }
    }
}

#[cfg(test)]
#[path = "keyword_literals_tests.rs"]
mod tests;
