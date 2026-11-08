//! GBK 文本（WHATWG gb18030 解码器）里每个输出字符消耗的源字节数。
//!
//! 正则的 GBK 通道用 `encoding_rs::GBK.decode_without_bom_handling` 把缓冲区解成
//! 文本、在文本上匹配，再把字符区间映射回原字节偏移。GBK 字符的字节数不能从字符
//! 本身推出：单字节 0x80 解成「€」（与双字节 A2 E3 是同一个字符），四字节序列解成
//! 一个字符，错误按 WHATWG 规则消耗 1–4 字节，还可能把随后的字节放回去重解。
//! [`GbkUnits`] 用解码器同一套状态机逐个产出每个输出字符消耗的字节数，与
//! encoding_rs 的输出逐字符对齐；双字节组合的结果直接取自 encoding_rs 自己的解码。

use std::sync::OnceLock;

/// 首字节 0x81..=0xFE 后跟任意次字节时，解码器第一个输出字符消耗的字节数：
/// 1 = 首字节单独成一个错误、次字节放回重解；2 = 两字节一起消耗（有映射的字符，
/// 或连同非 ASCII 次字节一起报一个错误）。次字节 0x30..=0x39 开启四字节序列，
/// 由 [`four_byte_len`] 处理，表里不用。
fn pair_lengths() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = vec![2u8; 126 * 256];
        for lead in 0x81u8..=0xFE {
            for trail in 0u8..=0xFF {
                let pair = [lead, trail];
                let (text, _) = encoding_rs::GBK.decode_without_bom_handling(&pair);
                if text.chars().count() > 1 {
                    table[usize::from(lead - 0x81) * 256 + usize::from(trail)] = 1;
                }
            }
        }
        table
    })
}

/// 首字节后跟 0x30..=0x39 时第一个输出字符消耗的字节数（`rest` 从首字节起）。
fn four_byte_len(rest: &[u8]) -> usize {
    match (rest.get(2), rest.get(3)) {
        // 流在第二字节后结束：挂起的两个字节合成一个错误。
        (None, _) => 2,
        // 第三字节不是 0x81..=0xFE：首字节单独报错，第二、三字节放回重解。
        (Some(third), _) if !(0x81..=0xFE).contains(third) => 1,
        // 流在第三字节后结束：挂起的三个字节合成一个错误。
        (Some(_), None) => 3,
        // 完整四字节：有映射是一个字符，无映射是一个错误，都消耗四字节。
        (Some(_), Some(fourth)) if (0x30..=0x39).contains(fourth) => 4,
        // 第四字节不是数字：首字节单独报错，其余放回重解。
        (Some(_), Some(_)) => 1,
    }
}

/// 逐个产出 `GBK.decode_without_bom_handling(bytes)` 每个输出字符消耗的源字节数。
pub(crate) struct GbkUnits<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> GbkUnits<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
}

impl Iterator for GbkUnits<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let rest = self.bytes.get(self.position..)?;
        let first = *rest.first()?;
        let len = match first {
            // ASCII、0x80（€）与 0xFF（错误）都是单字节。
            0x00..=0x80 | 0xFF => 1,
            0x81..=0xFE => match rest.get(1) {
                // 流在首字节后结束：挂起的首字节报一个错误。
                None => 1,
                Some(0x30..=0x39) => four_byte_len(rest),
                Some(&trail) => usize::from(
                    pair_lengths()[usize::from(first - 0x81) * 256 + usize::from(trail)],
                ),
            },
        };
        self.position += len;
        Some(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 逐字符核对：单元数与字符数一致、单元长度之和等于输入长度；能单独解码的
    /// 单元（ASCII、0x80、有映射的双/四字节）单独解码结果与该位置的字符一致。
    fn assert_aligned(bytes: &[u8]) {
        let (text, _) = encoding_rs::GBK.decode_without_bom_handling(bytes);
        let lengths: Vec<usize> = GbkUnits::new(bytes).collect();
        assert_eq!(
            lengths.len(),
            text.chars().count(),
            "unit count for {bytes:02x?}"
        );
        assert_eq!(lengths.iter().sum::<usize>(), bytes.len());
        let mut position = 0usize;
        for (len, ch) in lengths.iter().zip(text.chars()) {
            let unit = &bytes[position..position + len];
            if ch != '\u{FFFD}' {
                let (alone, _) = encoding_rs::GBK.decode_without_bom_handling(unit);
                assert_eq!(
                    alone.chars().collect::<Vec<_>>(),
                    vec![ch],
                    "unit {unit:02x?}"
                );
            }
            position += len;
        }
    }

    #[test]
    fn single_byte_euro_and_four_byte_sequences_are_counted_exactly() {
        let mut bytes = vec![0x80, b'a', 0x81, 0x30, 0x81, 0x30]; // €、a、U+0080（四字节）
        bytes.extend_from_slice(&encoding_rs::GBK.encode("密码").0);
        assert_eq!(
            GbkUnits::new(&bytes).collect::<Vec<_>>(),
            vec![1, 1, 4, 2, 2]
        );
        assert_aligned(&bytes);
    }

    #[test]
    fn errors_consume_what_the_decoder_consumes() {
        // 首字节 + ASCII 次字节（无映射时次字节放回）、首字节 + 0xFF、无映射的完整
        // 四字节、断在第三/第四字节的四字节序列、结尾挂起的首字节。
        for bytes in [
            &[0x81, 0x7F, b'x'][..],
            &[0x81, 0xFF, b'x'],
            &[0x84, 0x31, 0xA5, 0x30, b'x'],
            &[0x81, 0x30, b'x', b'y'],
            &[0x81, 0x30, 0x81, b'x'],
            &[b'x', 0x81, 0x30, 0x81],
            &[b'x', 0x81, 0x30],
            &[b'x', 0xFE],
        ] {
            assert_aligned(bytes);
        }
    }

    #[test]
    fn pseudo_random_bytes_stay_aligned() {
        let mut state = 0x9E37_79B9_u32;
        let bytes: Vec<u8> = (0..200_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect();
        assert_aligned(&bytes);
    }
}
