use super::*;
use crate::search::keyword::gbk_units::GbkUnits;

fn scan_all(set: &RegexMatcherSet, buffer: &[u8]) -> Vec<ByteHit> {
    let mut hits = Vec::new();
    set.scan_chunk(buffer, 0, &mut hits);
    hits
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn ascii_regex_hits_raw_bytes() {
    let (set, failures) = RegexMatcherSet::build(
        &[r"pass\d+".to_string()],
        true,
        &SearchEncoding::ALL.to_vec(),
        3,
    );
    assert!(failures.is_empty(), "{failures:?}");
    let hits = scan_all(&set, b"login pass123 ok");
    // ASCII / UTF-8 / GBK 编码在纯 ASCII 缓冲区下字节/解码结果一致，跨编码
    // 去重剩 ASCII 侧 1 条。
    assert_eq!(hits.len(), 1);
    assert!(hits
        .iter()
        .all(|hit| hit.start == 6 && hit.len == 7 && hit.keyword_index == 3));
    assert_eq!(hits[0].encoding, SearchEncoding::Ascii);
}

#[test]
fn ascii_pattern_builds_one_byte_regex_when_ascii_is_selected() {
    let (set, _) = RegexMatcherSet::build(
        &[r"pass\d+".to_string()],
        true,
        &[SearchEncoding::Ascii, SearchEncoding::Utf8],
        0,
    );
    assert_eq!(set.byte_matchers.len(), 1);
    let (set, _) =
        RegexMatcherSet::build(&[r"pass\d+".to_string()], true, &[SearchEncoding::Utf8], 0);
    assert_eq!(set.byte_matchers.len(), 1);
    assert_eq!(
        scan_all(&set, b"xx pass42")[0].encoding,
        SearchEncoding::Utf8
    );
}

#[test]
fn gbk_regex_maps_back_to_byte_offsets() {
    // 字符类在 GBK 下必须整体成类（字节拆分会变成单字节集合）
    let (set, failures) =
        RegexMatcherSet::build(&["密[码马]".to_string()], true, &[SearchEncoding::Gbk], 0);
    assert!(failures.is_empty(), "{failures:?}");
    let mut sample = Vec::new();
    sample.extend_from_slice(b"xx");
    sample.extend_from_slice(&SearchEncoding::Gbk.encode("密码"));
    sample.extend_from_slice(b"yy");
    let hits = scan_all(&set, &sample);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 2);
    assert_eq!(hits[0].len, 4);
    assert_eq!(hits[0].encoding, SearchEncoding::Gbk);
}

#[test]
fn gbk_channel_ignores_a_leading_byte_order_mark() {
    // 块恰好以 FF FE 开头时不能被当成 UTF-16 解码（BOM 嗅探会让偏移映射失效）。
    // GBK 下 FE 是双字节首字节，吞掉随后的一个 `x`，第二个 `x` 起重新对齐。
    let (set, failures) =
        RegexMatcherSet::build(&["密码".to_string()], true, &[SearchEncoding::Gbk], 0);
    assert!(failures.is_empty(), "{failures:?}");
    let mut sample = vec![0xFF, 0xFE, b'x', b'x'];
    sample.extend_from_slice(&SearchEncoding::Gbk.encode("密码"));
    let hits = scan_all(&set, &sample);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].start, hits[0].len), (4, 4));
}

#[test]
fn utf16_regex_maps_back_to_byte_offsets() {
    let (set, failures) = RegexMatcherSet::build(
        &[r"pass\w+".to_string()],
        true,
        &[SearchEncoding::Utf16Le],
        0,
    );
    assert!(failures.is_empty(), "{failures:?}");
    let mut sample = Vec::new();
    sample.extend_from_slice(&[b'!', 0x00]);
    for unit in "pass_word".encode_utf16() {
        sample.extend_from_slice(&unit.to_le_bytes());
    }
    let hits = scan_all(&set, &sample);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 2);
    assert_eq!(hits[0].len, "pass_word".len() * 2);
    assert_eq!(hits[0].encoding, SearchEncoding::Utf16Le);
}

#[test]
fn case_insensitive_utf16_regex_passes_its_prefilter() {
    // `(?i)` 的大小写变体展开进预筛字面量，混合大小写的 UTF-16 文本仍命中。
    let (set, failures) = RegexMatcherSet::build(
        &[r"secret\d".to_string()],
        false,
        &[SearchEncoding::Utf16Be],
        0,
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert!(set.text_groups[0].matchers[0].prefilter.is_some());
    let sample = SearchEncoding::Utf16Be.encode("..SeCrEt7..");
    let hits = scan_all(&set, &sample);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].start, hits[0].len), (4, 14));
}

#[test]
fn utf16_span_mapping_counts_surrogate_pairs() {
    // 增补平面字符占 2 码元（4 字节）：偏移映射必须按码元而非字符数推进。
    // 区间是解码文本（UTF-8）字节偏移：a=0..1、😀=1..5、b=5..6。
    let decoded = "a😀b密码";
    let spans = [(5usize, 6usize)]; // 「b」
    let mut emitted = Vec::new();
    map_char_spans(
        decoded,
        &spans,
        |ch| ch.len_utf16() * 2,
        |start, len| emitted.push((start, len)),
    );
    // a(2 字节) + 😀(4 字节) = 6；b 2 字节
    assert_eq!(emitted, vec![(6, 2)]);
}

#[test]
fn gbk_span_mapping_counts_double_byte_chars() {
    // 区间是解码文本（UTF-8）字节偏移：a=0..1、密=1..4、码=4..7、b=7..8。
    let decoded = "a密码b";
    let bytes = SearchEncoding::Gbk.encode(decoded);
    let spans = [(7usize, 8usize)]; // 「b」
    let mut emitted = Vec::new();
    let mut units = GbkUnits::new(&bytes);
    map_char_spans(
        decoded,
        &spans,
        |_| units.next().unwrap_or(1),
        |start, len| emitted.push((start, len)),
    );
    // a(1) + 密(2) + 码(2) = 5 字节；b 1 字节
    assert_eq!(emitted, vec![(5, 1)]);
}

#[test]
fn gbk_offsets_account_for_single_byte_euro_and_four_byte_sequences() {
    // 0x80 单字节解成「€」、81 30 81 30 四字节解成 U+0080：按「非 ASCII 即两字节」
    // 推算会把后面的命中偏移推错（越过缓冲区末尾时直接越界）。
    let (set, failures) =
        RegexMatcherSet::build(&["密码".to_string()], true, &[SearchEncoding::Gbk], 0);
    assert!(failures.is_empty(), "{failures:?}");
    let mut sample = vec![0x80, 0x80, 0x80, b'x', 0x81, 0x30, 0x81, 0x30];
    sample.extend_from_slice(&SearchEncoding::Gbk.encode("密码"));
    let hits = scan_all(&set, &sample);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].start, hits[0].len), (8, 4));
}

#[test]
fn carry_rule_drops_old_hits_keeps_straddling() {
    let (set, failures) =
        RegexMatcherSet::build(&["abcd".to_string()], true, &[SearchEncoding::Ascii], 0);
    assert!(failures.is_empty(), "{failures:?}");
    let buffer = b"xxabcdyy";
    // 命中 [2,6) 完全落在 carry 区 [0,6) 内：上一块已报告，丢弃
    let mut hits = Vec::new();
    set.scan_chunk(buffer, 6, &mut hits);
    assert!(hits.is_empty());

    // 命中 [2,6) 跨 carry=3 边界：上一块看不到（缓冲区只到 3），本块首报
    let mut hits = Vec::new();
    set.scan_chunk(buffer, 3, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 2);
}

#[test]
fn zero_length_matches_are_dropped() {
    let (set, failures) =
        RegexMatcherSet::build(&["a*".to_string()], true, &[SearchEncoding::Ascii], 0);
    assert!(failures.is_empty(), "{failures:?}");
    assert!(scan_all(&set, b"bbb").is_empty());
}

#[test]
fn invalid_regex_reported_once_per_pattern() {
    let (_, failures) = RegexMatcherSet::build(
        &["[unclosed".to_string()],
        true,
        &SearchEncoding::ALL.to_vec(),
        0,
    );
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("[unclosed"));
}

#[test]
fn case_insensitive_flag_matches_any_case() {
    let (set, failures) =
        RegexMatcherSet::build(&["PASS".to_string()], false, &[SearchEncoding::Ascii], 0);
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(scan_all(&set, b"x pass y").len(), 1);
}

#[test]
fn required_literals_pick_the_most_selective_factor() {
    // 大字符类推不出字面量，两侧字面量取更长的一个。
    assert_eq!(
        required_literals("(?s)zqx[0-9]{3}jv"),
        Some(strings(&["zqx"]))
    );
    assert_eq!(
        required_literals(r"(?s)[a-z]+@[a-z]+\.com"),
        Some(strings(&[".com"]))
    );
    // 选择分支与后续字面量拼接成更长的字面量。
    assert_eq!(
        required_literals("(?s)(foo|bar)baz"),
        Some(strings(&["foobaz", "barbaz"]))
    );
    // 至少一次的重复收尾前一段、开启后一段。
    assert_eq!(required_literals("(?s)ab+c"), Some(strings(&["bc"])));
    // 可为空的分支推不出必含字面量。
    assert_eq!(required_literals("(?s)a|b*"), None);
    assert_eq!(required_literals(r"(?s)\d{11}"), None);
}

#[test]
fn case_insensitive_literals_expand_case_variants() {
    let literals = required_literals("(?i)(?s)pass").expect("literals");
    // p/a 各 2 种、s 含长 s（ſ）3 种：2 × 2 × 3 × 3。
    assert_eq!(literals.len(), 36);
    assert!(literals.contains(&"pass".to_string()));
    assert!(literals.contains(&"PaSs".to_string()));
    assert!(literals.contains(&"paſs".to_string()));
}

#[test]
fn encoded_prefilter_keeps_only_round_trippable_fragments() {
    // UTF-16：字面量原样编码，只认交错零字节的形式。
    let utf16 = encoded_prefilter(&strings(&["zqx"]), SearchEncoding::Utf16Le).expect("utf16");
    assert!(utf16.is_match(&SearchEncoding::Utf16Le.encode("..zqx..")));
    assert!(!utf16.is_match(b"..zqx.."));
    // GBK：非 ASCII 字面量取其最长 ASCII 片段；全非 ASCII 时放弃预筛。
    let gbk = encoded_prefilter(&strings(&["paſs"]), SearchEncoding::Gbk).expect("gbk");
    assert!(gbk.is_match(b"xxpaxx"));
    assert!(encoded_prefilter(&strings(&["密码"]), SearchEncoding::Gbk).is_none());
    assert!(encoded_prefilter(&strings(&["ok", "密码"]), SearchEncoding::Gbk).is_none());
}

#[test]
fn longest_run_splits_on_rejected_chars() {
    let ascii: fn(char) -> bool = |ch| ch.is_ascii();
    assert_eq!(longest_run("ab密cde", ascii), "cde");
    assert_eq!(longest_run("密码", ascii), "");
    assert_eq!(longest_run("abc", ascii), "abc");
}
