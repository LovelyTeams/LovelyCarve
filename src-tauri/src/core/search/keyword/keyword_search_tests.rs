use super::*;

#[test]
fn case_insensitive_ascii_keyword_matches_any_case() {
    let matcher = MatcherSet::build(&["password".to_string()], false);
    // UTF-16 变体最长：8 字符 × 2 字节
    assert_eq!(matcher.max_pattern_len(), 16);

    let mut hits = Vec::new();
    matcher.scan_chunk(b"login: Password ok", 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 7);
    assert_eq!(hits[0].len, 8);
    assert_eq!(hits[0].keyword_index, 0);
    assert_eq!(hits[0].encoding, SearchEncoding::Ascii);

    hits.clear();
    matcher.scan_chunk(b"PASSWORD", 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].encoding, SearchEncoding::Ascii);
}

#[test]
fn gbk_keyword_hits_gbk_bytes() {
    let pattern = SearchEncoding::Gbk.encode("密码");
    assert_eq!(pattern.len(), 4);

    let matcher = MatcherSet::build(&["密码".to_string()], true);
    let mut sample = Vec::new();
    sample.extend_from_slice(b"abc");
    sample.extend_from_slice(&pattern);
    sample.extend_from_slice(b"def");
    let mut hits = Vec::new();
    matcher.scan_chunk(&sample, 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 3);
    assert_eq!(hits[0].len, 4);
    assert_eq!(hits[0].encoding, SearchEncoding::Gbk);
    assert_eq!(hits[0].keyword_index, 0);
}

#[test]
fn utf16le_keyword_hits_interleaved_bytes() {
    let matcher = MatcherSet::build(&["password".to_string()], false);
    let mut sample = Vec::new();
    for unit in "Password".encode_utf16() {
        sample.extend_from_slice(&unit.to_le_bytes());
    }
    let mut hits = Vec::new();
    matcher.scan_chunk(&sample, 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 0);
    assert_eq!(hits[0].len, "password".len() * 2);
    assert_eq!(hits[0].encoding, SearchEncoding::Utf16Le);
}

#[test]
fn keyword_spanning_chunk_boundary_reported_once() {
    // 「password」跨 chunk1/chunk2 边界；carry 内还有一段 x*9 验证完全落入
    // carry 的重复命中（"xx"）被丢弃、不重复报告。
    let matcher = MatcherSet::build(&["password".to_string(), "xx".to_string()], true);
    let carry_len = matcher.max_pattern_len() - 1;

    let mut chunk1 = vec![b'x'; 100];
    chunk1.extend_from_slice(b"passwo");
    let mut chunk2 = vec![b'r', b'd'];
    chunk2.extend(std::iter::repeat(b'y').take(100));

    // 第一块：carry 为空
    let mut hits = Vec::new();
    matcher.scan_chunk(&chunk1, 0, &mut hits);
    // x*100 中 "xx" 重叠命中 99 条，无跨块信息
    assert_eq!(hits.len(), 99);
    assert!(hits.iter().all(|hit| hit.keyword_index == 1));

    // 第二块：carry = 上一块末尾 carry_len 字节
    let carry = &chunk1[chunk1.len() - carry_len..];
    let mut buffer = Vec::with_capacity(carry.len() + chunk2.len());
    buffer.extend_from_slice(carry);
    buffer.extend_from_slice(&chunk2);

    hits.clear();
    matcher.scan_chunk(&buffer, carry.len(), &mut hits);
    // 只剩跨边界的 "password"："xx" 命中全部完全落入 carry，被丢弃
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, carry.len() - 6);
    assert_eq!(hits[0].len, 8);
    assert_eq!(hits[0].keyword_index, 0);
    assert_eq!(hits[0].encoding, SearchEncoding::Ascii);
}

#[test]
fn overlapping_matches_all_reported() {
    let matcher = MatcherSet::build(&["aa".to_string()], true);
    let mut hits = Vec::new();
    matcher.scan_chunk(b"aaa", 0, &mut hits);
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|hit| hit.start == 0 && hit.len == 2));
    assert!(hits.iter().any(|hit| hit.start == 1 && hit.len == 2));
    assert!(hits.iter().all(|hit| hit.encoding == SearchEncoding::Ascii));
}

#[test]
fn ascii_utf8_duplicate_reported_once_as_ascii() {
    // ASCII 与 UTF-8 自动机命中同一字节区间，只报 1 条、标记为 ASCII
    let matcher = MatcherSet::build(&["password".to_string()], true);
    let mut hits = Vec::new();
    matcher.scan_chunk(b"password", 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].start, 0);
    assert_eq!(hits[0].len, 8);
    assert_eq!(hits[0].encoding, SearchEncoding::Ascii);
}

#[test]
fn empty_keyword_list_scans_nothing() {
    let matcher = MatcherSet::build(&[], true);
    assert_eq!(matcher.max_pattern_len(), 0);
    let mut hits = Vec::new();
    matcher.scan_chunk(b"anything at all", 0, &mut hits);
    assert!(hits.is_empty());

    let blank = MatcherSet::build(&["  ".to_string()], true);
    let mut hits = Vec::new();
    blank.scan_chunk(b"", 0, &mut hits);
    assert!(hits.is_empty());
}

#[test]
fn utf16_round_trips_both_endiannesses() {
    let text = "Hello 世界";
    let be = SearchEncoding::Utf16Be.encode(text);
    let le = SearchEncoding::Utf16Le.encode(text);
    assert_eq!(SearchEncoding::Utf16Be.decode_to_string(&be), text);
    assert_eq!(SearchEncoding::Utf16Le.decode_to_string(&le), text);
    assert_ne!(be, le);
}

#[test]
fn gbk_round_trips() {
    let text = "密码测试";
    let bytes = SearchEncoding::Gbk.encode(text);
    assert_eq!(SearchEncoding::Gbk.decode_to_string(&bytes), text);
}

#[test]
fn decode_tolerates_partial_multibyte_tail() {
    // 半个 UTF-8 字符的尾巴以替换字符解码，不 panic
    let decoded = SearchEncoding::Utf8.decode_to_string(&[b'a', 0xE5]);
    assert!(decoded.starts_with('a'));
    assert!(decoded.contains('\u{FFFD}'));
}

#[test]
fn labels_cover_all_encodings() {
    assert_eq!(SearchEncoding::ALL.len(), 5);
    assert_eq!(SearchEncoding::Ascii.label(), "ASCII");
    assert_eq!(SearchEncoding::Utf8.label(), "UTF-8");
    assert_eq!(SearchEncoding::Utf16Le.label(), "UTF-16LE");
    assert_eq!(SearchEncoding::Utf16Be.label(), "UTF-16BE");
    assert_eq!(SearchEncoding::Gbk.label(), "GBK");
}

#[test]
fn keyword_indices_refer_to_deduped_list() {
    // "alpha" 重复出现只算一条；"beta" 是第 2 条
    let matcher = MatcherSet::build(
        &["alpha".to_string(), "alpha".to_string(), "beta".to_string()],
        true,
    );
    let mut hits = Vec::new();
    matcher.scan_chunk(b"beta alpha", 0, &mut hits);
    assert_eq!(hits.len(), 2);
    hits.sort_by_key(|hit| hit.start);
    assert_eq!(hits[0].start, 0);
    assert_eq!(hits[0].keyword_index, 1);
    assert_eq!(hits[1].start, 5);
    assert_eq!(hits[1].keyword_index, 0);
}

#[test]
fn from_label_parses_codepages() {
    assert_eq!(
        SearchEncoding::from_label("UTF-8"),
        Some(SearchEncoding::Utf8)
    );
    assert_eq!(
        SearchEncoding::from_label("utf-16le"),
        Some(SearchEncoding::Utf16Le)
    );
    assert_eq!(
        SearchEncoding::from_label(" GBK "),
        Some(SearchEncoding::Gbk)
    );
    assert_eq!(SearchEncoding::from_label("latin1"), None);
}

#[test]
fn selected_encodings_restrict_matchers() {
    // 仅选 GBK：UTF-16LE 字节流不再命中
    let matcher = MatcherSet::build_plan(&["密码".to_string()], &[], true, &[SearchEncoding::Gbk])
        .expect("build plan");
    assert_eq!(matcher.keyword_count(), 1);
    let utf16 = SearchEncoding::Utf16Le.encode("密码");
    let mut hits = Vec::new();
    matcher.scan_chunk(&utf16, 0, &mut hits);
    assert!(hits.is_empty());

    let gbk = SearchEncoding::Gbk.encode("密码");
    matcher.scan_chunk(&gbk, 0, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].encoding, SearchEncoding::Gbk);
}

#[test]
fn plan_merges_keyword_and_regex_hits() {
    let matcher = MatcherSet::build_plan(
        &["password".to_string()],
        &[r"sekret\d+".to_string()],
        true,
        &[SearchEncoding::Ascii],
    )
    .expect("build plan");
    assert_eq!(matcher.keyword_count(), 1);
    // 正则通道启用后 carry 窗口取正则固定值
    assert_eq!(
        matcher.carry_len(),
        crate::core::search::keyword::keyword_regex::REGEX_CARRY_BYTES
    );

    let mut hits = Vec::new();
    matcher.scan_chunk(b"password sekret7 ok", 0, &mut hits);
    hits.sort_by_key(|hit| hit.start);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].keyword_index, 0); // password（关键词）
    assert_eq!(hits[1].keyword_index, 1); // sekret7（正则，基址=关键词数）
}

#[test]
fn invalid_regex_rejects_whole_plan() {
    let err = MatcherSet::build_plan(
        &["ok".to_string()],
        &["[bad".to_string()],
        true,
        &SearchEncoding::ALL.to_vec(),
    )
    .expect_err("must fail");
    assert!(err.contains("[bad"));
}

#[test]
fn carry_window_without_regex_is_pattern_bound() {
    let matcher = MatcherSet::build(&["password".to_string()], true);
    assert_eq!(matcher.carry_len(), matcher.max_pattern_len() - 1);
}
