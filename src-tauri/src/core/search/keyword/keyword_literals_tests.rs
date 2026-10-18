use super::*;
use crate::core::search::keyword::keyword_search::MatcherSet;

/// 独立逐字节匹配，按原逐编码通道的优先级报告、按区间去重。
fn bytewise_hits(
    keywords: &[String],
    encodings: &[SearchEncoding],
    case_sensitive: bool,
    bytes: &[u8],
    carry: usize,
) -> Vec<ByteHit> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for &encoding in encodings {
        for folded in [false, true] {
            if folded && case_sensitive {
                continue;
            }
            let (patterns, indices) = encode_patterns(keywords, encoding, case_sensitive, folded);
            let mut hits = Vec::new();
            for (pattern, index) in patterns.iter().zip(indices) {
                for (start, window) in bytes.windows(pattern.len()).enumerate() {
                    let end = start + pattern.len();
                    let matches = if folded {
                        window.eq_ignore_ascii_case(pattern)
                    } else {
                        window == pattern
                    };
                    if matches && end > carry {
                        hits.push(ByteHit {
                            start,
                            len: pattern.len(),
                            keyword_index: index,
                            encoding,
                        });
                    }
                }
            }
            hits.sort_by_key(|hit| (hit.start + hit.len, std::cmp::Reverse(hit.len)));
            out.extend(
                hits.into_iter()
                    .filter(|hit| seen.insert((hit.start, hit.len))),
            );
        }
    }
    out
}

#[test]
fn combined_encodings_match_bytewise_oracle_for_all_subsets() {
    let keywords: Vec<String> = [
        "a", "Aa", "aA", "aa", "ba", "123", "密码", "登录", "Äpfel", "äPFEL", "䅁", "慡",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut bytes: Vec<u8> = (0..=255).collect();
    for encoding in SearchEncoding::ALL {
        bytes.extend(encoding.encode("aaABAa 123 密码 登录 Äpfel ÄPFEL äpfel 䅁 慡"));
        bytes.push(0xff);
    }
    // 包含非法、多字节尾与奇数 UTF-16 对齐的任意输入。
    let mut state = 0x1234_5678_u32;
    for _ in 0..512 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        bytes.push(state as u8);
    }
    let mut scratch = MatchScratch::default();
    for mask in 1..32 {
        let encodings: Vec<_> = SearchEncoding::ALL
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, encoding)| encoding)
            .collect();
        for case_sensitive in [false, true] {
            let matcher = MatcherSet::build_plan(&keywords, &[], case_sensitive, &encodings)
                .expect("build matcher");
            for carry in [0, 17, bytes.len() / 2, bytes.len()] {
                let expected = bytewise_hits(&keywords, &encodings, case_sensitive, &bytes, carry);
                let mut actual = Vec::new();
                matcher.scan_chunk_with(&bytes, carry, &mut actual, &mut scratch);
                assert_eq!(
                    actual, expected,
                    "mask={mask} case={case_sensitive} carry={carry}"
                );
            }
        }
    }
}

#[test]
fn exact_multibyte_patterns_do_not_fold_ascii_valued_bytes() {
    // U+4141 与 U+6161 的 UTF-16 字节分别是 AA / aa；GBK 的 U+4E04
    // (81 41) 与 U+4E64 (81 61) 也只差一个看似 ASCII 大小写的尾字节。
    let cases = [
        (SearchEncoding::Utf16Le, "䅁", vec![0x61, 0x61]),
        (SearchEncoding::Utf16Be, "䅁", vec![0x61, 0x61]),
        (SearchEncoding::Gbk, "丄", vec![0x81, 0x61]),
    ];
    for (encoding, keyword, false_match) in cases {
        let keywords = [keyword.to_string(), "password".to_string()];
        let matcher = MatcherSet::build_plan(&keywords, &[], false, &[encoding]).unwrap();
        let mut hits = Vec::new();
        matcher.scan_chunk(&false_match, 0, &mut hits);
        assert_eq!(hits, Vec::new(), "{encoding:?}");
        matcher.scan_chunk(&encoding.encode(keyword), 0, &mut hits);
        assert_eq!(
            hits,
            vec![ByteHit {
                start: 0,
                len: 2,
                keyword_index: 0,
                encoding,
            }]
        );
    }
}

#[test]
fn higher_priority_caseless_hit_wins_over_lower_priority_exact_hit() {
    // ASCII aa 的忽略大小写通道与 U+4141 的 UTF-16 原样通道都匹配 AA。
    let matcher = MatcherSet::build(&["aa".to_string(), "䅁".to_string()], false);
    let mut hits = Vec::new();
    matcher.scan_chunk(b"AA", 0, &mut hits);
    assert_eq!(
        hits,
        vec![ByteHit {
            start: 0,
            len: 2,
            keyword_index: 0,
            encoding: SearchEncoding::Ascii,
        }]
    );
}

#[test]
fn each_encoded_keyword_survives_every_possible_chunk_boundary() {
    let keywords = ["aA".to_string(), "密码".to_string()];
    let matcher = MatcherSet::build(&keywords, false);
    for encoding in SearchEncoding::ALL {
        let mut bytes = vec![0xfe; 3];
        bytes.extend(encoding.encode("aA"));
        if encoding != SearchEncoding::Ascii {
            bytes.extend(encoding.encode("密码"));
        }
        bytes.extend([0xfe; 3]);
        let mut expected = Vec::new();
        matcher.scan_chunk(&bytes, 0, &mut expected);
        expected.sort_by_key(|hit| (hit.start, hit.len));
        for boundary in 1..bytes.len() {
            let mut actual = Vec::new();
            matcher.scan_chunk(&bytes[..boundary], 0, &mut actual);
            let carry = matcher.carry_len().min(boundary);
            let window_start = boundary - carry;
            let mut next = Vec::new();
            matcher.scan_chunk(&bytes[window_start..], carry, &mut next);
            actual.extend(next.into_iter().map(|mut hit| {
                hit.start += window_start;
                hit
            }));
            actual.sort_by_key(|hit| (hit.start, hit.len));
            assert_eq!(actual, expected, "{encoding:?} boundary={boundary}");
        }
    }
}
