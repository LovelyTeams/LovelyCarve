//! 文本「原类型视图 / 文本视图」的字符编码检测 + 解码。前端此前一律按 UTF-8
//! 解码原始字节，GBK/GB18030/Big5/Shift-JIS/EUC-* 等非 UTF-8 文本会被解成乱码。
//! 这里改为先用 chardetng（Firefox 的遗留内容编码检测器）猜出编码，再用
//! encoding_rs 解码为 UTF-8 String 回传，前端直接渲染。
//!
//! 检测策略：BOM 权威优先（UTF-8 / UTF-16LE / UTF-16BE 由文件头声明，胜过统计
//! 猜测），无 BOM 时交给 chardetng 统计检测。chardetng 1.0 不给数值置信度，故以
//! `from_bom`（是否 BOM 声明）+ `had_errors`（解码是否遇到无法映射的字节）作为
//! 「这次猜测有多可信」的信号——前端据此弱化提示并允许用户手动改编码。
//!
//! 读取的常是文件的一段而非全部：样本在文件中途结束时，末尾被截断的多字节字符
//! 不能算作错误（否则 GBK 等多字节编码会被排除，检测退回 windows-1252），解码时
//! 也留给下一段，`next_offset` 从该字符的起点继续。

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{CoderResult, Decoder, Encoding};
use serde::Serialize;

/// The smallest read that holds any character of a supported encoding.
pub const MIN_SPAN_BYTES: usize = 4;

/// 一次文本预览的解码结果。`text` 已是 UTF-8，前端文本/代码/Markdown/结构化
/// 视图直接渲染；`encoding` 等字段驱动「编码」徽标与覆盖入口。
#[derive(Debug, Serialize)]
pub struct DecodedText {
    /// 用检测出的（或 BOM 声明的）编码解码而成的 UTF-8 文本。
    pub text: String,
    /// 实际使用的编码 WHATWG 规范名，如 `UTF-8` / `GBK` / `Big5` / `Shift_JIS`。
    pub encoding: String,
    /// 编码来自文件头 BOM（权威）而非统计检测——UI 据此把「自动检测」表述为「BOM 声明」。
    pub from_bom: bool,
    /// 解码过程中出现过该编码无法映射的字节（已用 U+FFFD 替换）。检测误判或本是
    /// 二进制内容时为 true，是「若仍乱码请手动改编码」的提示信号。
    pub had_errors: bool,
}

/// One decoded run of a file and where the next one starts: the fields of
/// [`DecodedText`], then the offsets. Not flattened, so that the CLI renames
/// its fields as it renames every struct's.
#[derive(Debug, Serialize)]
pub struct TextPage {
    pub text: String,
    pub encoding: String,
    pub from_bom: bool,
    pub had_errors: bool,
    /// The byte offset in the file the text starts at.
    pub offset: u64,
    /// Where the next run starts, at a character boundary; `None` at the end
    /// of the file.
    pub next_offset: Option<u64>,
}

impl TextPage {
    pub fn new(decoded: DecodedText, offset: u64, next_offset: Option<u64>) -> Self {
        Self {
            text: decoded.text,
            encoding: decoded.encoding,
            from_bom: decoded.from_bom,
            had_errors: decoded.had_errors,
            offset,
            next_offset,
        }
    }
}

/// Where a run of bytes sits in its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// The whole file.
    Whole,
    /// The start of the file, with more following.
    Head,
    /// Bytes after the start, with more following.
    Middle,
    /// The end of the file, after its start.
    Tail,
}

impl Span {
    /// Where a byte order mark can appear.
    fn starts_file(self) -> bool {
        matches!(self, Self::Whole | Self::Head)
    }

    /// Whether a character cut at the end is incomplete rather than continued
    /// by the following bytes.
    fn ends_file(self) -> bool {
        matches!(self, Self::Whole | Self::Tail)
    }
}

/// An encoding to decode with, and whether a byte order mark declared it.
#[derive(Clone, Copy, Debug)]
pub struct Guess {
    pub encoding: &'static Encoding,
    pub from_bom: bool,
}

/// The encoding a WHATWG label names (`gbk`, `gb18030`, `big5`, `shift_jis`,
/// `utf-16le`, …).
pub fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.trim().as_bytes())
}

/// The encoding of a file whose first bytes are `sample`, a [`Span::Whole`]
/// or [`Span::Head`]: a byte order mark, else chardetng's guess.
pub fn detect(sample: &[u8], span: Span) -> Guess {
    if let Some((encoding, _)) = Encoding::for_bom(sample) {
        return Guess {
            encoding,
            from_bom: true,
        };
    }
    // 允许 UTF-8 与 ISO-2022-JP 作为结果——这是取证查看器而非会执行脚本的浏览器，
    // 不存在「检测 UTF-8 会被网页滥用」的顾虑，检测面越宽越好。只有样本到达文件
    // 末尾时才告诉检测器输入已结束。
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(sample, span.ends_file());
    Guess {
        encoding: detector.guess(None, Utf8Detection::Allow),
        from_bom: false,
    }
}

/// Decode `input` into `text`; whether any byte was malformed.
fn feed(decoder: &mut Decoder, mut input: &[u8], text: &mut String, last: bool) -> bool {
    let mut had_errors = false;
    loop {
        text.reserve(
            decoder
                .max_utf8_buffer_length(input.len())
                .unwrap_or(input.len().saturating_mul(3))
                .max(4),
        );
        let (result, read, errors) = decoder.decode_to_string(input, text, last);
        had_errors |= errors;
        input = &input[read..];
        match result {
            CoderResult::InputEmpty => return had_errors,
            CoderResult::OutputFull => {}
        }
    }
}

/// Decode `bytes`, which sit at `span` in their file, as `guess` says, with
/// the number of bytes the text covers. A byte order mark is removed only at
/// the start of the file. When more of the file follows, a character cut at
/// the end stays out of the text, and the covered bytes end where it begins.
pub fn decode_span(bytes: &[u8], span: Span, guess: Guess) -> (DecodedText, usize) {
    let mut decoder = if span.starts_file() {
        guess.encoding.new_decoder_with_bom_removal()
    } else {
        guess.encoding.new_decoder_without_bom_handling()
    };
    let mut text = String::new();
    let decoded = |text, had_errors| DecodedText {
        text,
        encoding: guess.encoding.name().to_owned(),
        from_bom: guess.from_bom,
        had_errors,
    };
    if span.ends_file() {
        let had_errors = feed(&mut decoder, bytes, &mut text, true);
        return (decoded(text, had_errors), bytes.len());
    }
    // No character is longer than four bytes: feed the last ones one at a
    // time and cover the bytes up to the last one that completed a character.
    let split = bytes.len().saturating_sub(MIN_SPAN_BYTES);
    let mut had_errors = feed(&mut decoder, &bytes[..split], &mut text, false);
    let mut covered = split;
    for (index, byte) in bytes.iter().enumerate().skip(split) {
        let before = text.len();
        had_errors |= feed(&mut decoder, std::slice::from_ref(byte), &mut text, false);
        if text.len() > before {
            covered = index + 1;
        }
    }
    if covered == 0 && !bytes.is_empty() {
        // Nothing complete at all: decode the bytes as they are, rather than
        // offer the same offset again.
        let span = if span.starts_file() {
            Span::Whole
        } else {
            Span::Tail
        };
        return decode_span(bytes, span, guess);
    }
    (decoded(text, had_errors), covered)
}

/// 检测 `bytes`（一个完整文件）的字符编码并解码为 UTF-8。空输入按空 UTF-8 处理。
pub fn decode(bytes: &[u8]) -> DecodedText {
    decode_span(bytes, Span::Whole, detect(bytes, Span::Whole)).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_ascii_is_utf8_clean() {
        let r = decode(b"hello, world\n");
        assert_eq!(r.text, "hello, world\n");
        assert_eq!(r.encoding, "UTF-8");
        assert!(!r.from_bom);
        assert!(!r.had_errors);
        assert_eq!(decode(b"").encoding, "UTF-8");
    }

    #[test]
    fn utf8_bom_is_detected_and_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("中文".as_bytes());
        let r = decode(&bytes);
        assert_eq!(r.text, "中文");
        assert_eq!(r.encoding, "UTF-8");
        assert!(r.from_bom);
        assert!(!r.had_errors);
    }

    #[test]
    fn gbk_chinese_decodes_without_mojibake() {
        // “你好世界” in GBK (each char 2 bytes), no BOM.
        let gbk = [0xC4, 0xE3, 0xBA, 0xC3, 0xCA, 0xC0, 0xBD, 0xE7];
        let r = decode(&gbk);
        assert_eq!(r.text, "你好世界");
        assert!(!r.from_bom);
        // GBK/GB18030 family.
        assert!(
            r.encoding.eq_ignore_ascii_case("gbk") || r.encoding.eq_ignore_ascii_case("gb18030"),
            "unexpected encoding: {}",
            r.encoding
        );
    }

    #[test]
    fn utf16le_bom_decodes() {
        // "AB" in UTF-16LE with BOM.
        let bytes = [0xFF, 0xFE, b'A', 0x00, b'B', 0x00];
        let r = decode(&bytes);
        assert_eq!(r.text, "AB");
        assert!(r.from_bom);
        assert_eq!(r.encoding, "UTF-16LE");
    }

    /// A GBK log whose sample ends inside a character: as a whole file the
    /// cut character rules GBK out, as the head of a longer file it must not.
    #[test]
    fn a_character_cut_at_the_end_of_a_head_is_left_for_the_next_read() {
        let line = "2024-06-01 12:00:00 INFO 用户登录成功，来源地址 10.0.0.8\n";
        let text = line.repeat(40);
        let (gbk, _, _) = encoding_rs::GBK.encode(&text);
        let cut = gbk
            .iter()
            .rposition(|byte| *byte >= 0x81)
            .expect("multi-byte text");
        let head = &gbk[..cut];
        assert_ne!(detect(head, Span::Whole).encoding, encoding_rs::GBK);
        let guess = detect(head, Span::Head);
        assert_eq!(guess.encoding, encoding_rs::GBK);
        let (decoded, covered) = decode_span(head, Span::Head, guess);
        assert!(!decoded.had_errors, "{}", decoded.text);
        assert_eq!(covered, cut - 1);
        assert_eq!(
            encoding_rs::GBK.encode(&decoded.text).0.as_ref(),
            &gbk[..covered]
        );
        // The rest decodes from the boundary on, and the parts make the whole.
        let (rest, rest_covered) = decode_span(&gbk[covered..], Span::Tail, guess);
        assert_eq!(rest_covered, gbk.len() - covered);
        assert_eq!(format!("{}{}", decoded.text, rest.text), text);
    }

    #[test]
    fn spans_page_through_every_encoding_at_character_boundaries() {
        let text = "日志：résumé 🙂 测试 ok\r\n".repeat(30);
        for encoding in [
            encoding_rs::UTF_8,
            encoding_rs::GB18030,
            encoding_rs::BIG5,
            encoding_rs::SHIFT_JIS,
            encoding_rs::UTF_16LE,
            encoding_rs::UTF_16BE,
        ] {
            let bytes: Vec<u8> = if encoding == encoding_rs::UTF_16LE {
                text.encode_utf16().flat_map(u16::to_le_bytes).collect()
            } else if encoding == encoding_rs::UTF_16BE {
                text.encode_utf16().flat_map(u16::to_be_bytes).collect()
            } else {
                encoding.encode(&text).0.into_owned()
            };
            let reference = encoding.decode_without_bom_handling(&bytes).0;
            let guess = Guess {
                encoding,
                from_bom: false,
            };
            for page in [MIN_SPAN_BYTES, 5, 7, 64, 1000] {
                let mut offset = 0;
                let mut joined = String::new();
                while offset < bytes.len() {
                    let end = (offset + page).min(bytes.len());
                    let span = match (offset == 0, end == bytes.len()) {
                        (true, true) => Span::Whole,
                        (true, false) => Span::Head,
                        (false, false) => Span::Middle,
                        (false, true) => Span::Tail,
                    };
                    let (decoded, covered) = decode_span(&bytes[offset..end], span, guess);
                    assert!(covered > 0, "{} page {page} stalled", encoding.name());
                    joined.push_str(&decoded.text);
                    offset += covered;
                }
                assert_eq!(joined, reference, "{} page {page}", encoding.name());
            }
        }
    }

    #[test]
    fn labels_name_encodings_and_a_forced_bom_is_removed_only_at_the_start() {
        assert_eq!(encoding_for_label(" GBK "), Some(encoding_rs::GBK));
        assert_eq!(encoding_for_label("utf-16le"), Some(encoding_rs::UTF_16LE));
        assert_eq!(encoding_for_label("no-such-encoding"), None);
        let guess = Guess {
            encoding: encoding_rs::UTF_8,
            from_bom: false,
        };
        let bytes = [0xEF, 0xBB, 0xBF, b'a'];
        assert_eq!(decode_span(&bytes, Span::Whole, guess).0.text, "a");
        assert_eq!(decode_span(&bytes, Span::Tail, guess).0.text, "\u{feff}a");
    }
}
