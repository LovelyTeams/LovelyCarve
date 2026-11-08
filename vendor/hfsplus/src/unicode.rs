//! HFS+ Unicode comparison utilities.
//!
//! HFSX (case-sensitive) uses binary comparison of UTF-16BE values.
//! HFS+ (case-insensitive) uses Apple's FastUnicodeCompare with a
//! case-folding table defined in Apple TN1150.

/// Compare two HFS+ Unicode names using binary comparison (HFSX / case-sensitive).
pub fn compare_binary(a: &[u16], b: &[u16]) -> std::cmp::Ordering {
    let len = a.len().min(b.len());
    for i in 0..len {
        match a[i].cmp(&b[i]) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    a.len().cmp(&b.len())
}

mod decomposition;
mod folding;

/// Apple's fixed HFS case table maps ignorable characters to zero.
fn case_fold(character: u16) -> u16 {
    if character < 256 {
        return folding::LATIN_FOLD[usize::from(character)];
    }
    let page = usize::from(folding::LOWER_FOLD[usize::from(character >> 8)]);
    if page == 0 {
        character
    } else {
        folding::LOWER_FOLD[page + usize::from(character & 0xff)]
    }
}

/// Compare HFS names using the format's fixed tables, skipping ignorable characters.
pub fn compare_case_insensitive(a: &[u16], b: &[u16]) -> std::cmp::Ordering {
    a.iter()
        .copied()
        .map(case_fold)
        .filter(|value| *value != 0)
        .cmp(b.iter().copied().map(case_fold).filter(|value| *value != 0))
}

/// Normalize a caller's path to the HFS variant of Unicode 3.2 NFD.
/// Compatibility ideographs and U+2000–U+2FFF are intentionally left intact.
fn normalize_name(name: &str) -> Vec<char> {
    let mut output = Vec::new();
    for character in name.chars() {
        let codepoint = character as u32;
        if (0xac00..=0xd7a3).contains(&codepoint) {
            let syllable = codepoint - 0xac00;
            // These arithmetic ranges are valid Unicode scalar values.
            output.extend(char::from_u32(0x1100 + syllable / 588));
            output.extend(char::from_u32(0x1161 + syllable % 588 / 28));
            if !syllable.is_multiple_of(28) {
                output.extend(char::from_u32(0x11a7 + syllable % 28));
            }
        } else if let Some(mapping) = decomposition::decomposition(character) {
            output.extend(mapping.chars());
        } else {
            output.push(character);
        }
    }
    // Stable canonical ordering within each combining sequence.
    for index in 1..output.len() {
        let class = decomposition::combining_class(output[index]);
        if class == 0 {
            continue;
        }
        let mut position = index;
        while position > 0 && decomposition::combining_class(output[position - 1]) > class {
            output.swap(position, position - 1);
            position -= 1;
        }
    }
    output
}

/// Convert a UTF-16BE byte slice to a `Vec<u16>` of code points
pub fn utf16be_to_u16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_be_bytes(*chunk))
        .collect()
}

/// Convert a `Vec<u16>` of UTF-16 code points to a Rust String
pub fn utf16_to_string(code_points: &[u16]) -> String {
    String::from_utf16_lossy(code_points)
}

/// Encode a Rust string to HFS+ UTF-16BE name
pub fn string_to_utf16(s: &str) -> Vec<u16> {
    normalize_name(s)
        .into_iter()
        .flat_map(|character| {
            let mut buffer = [0u16; 2];
            character.encode_utf16(&mut buffer).to_vec()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_compare() {
        let a = string_to_utf16("abc");
        let b = string_to_utf16("abd");
        assert_eq!(compare_binary(&a, &b), std::cmp::Ordering::Less);

        let a = string_to_utf16("abc");
        let b = string_to_utf16("abc");
        assert_eq!(compare_binary(&a, &b), std::cmp::Ordering::Equal);

        let a = string_to_utf16("abc");
        let b = string_to_utf16("ab");
        assert_eq!(compare_binary(&a, &b), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_case_insensitive_compare() {
        let a = string_to_utf16("ABC");
        let b = string_to_utf16("abc");
        assert_eq!(compare_case_insensitive(&a, &b), std::cmp::Ordering::Equal);

        let a = string_to_utf16("Hello");
        let b = string_to_utf16("hello");
        assert_eq!(compare_case_insensitive(&a, &b), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_utf16_roundtrip() {
        let original = "Hello, World!";
        let utf16 = string_to_utf16(original);
        let back = utf16_to_string(&utf16);
        assert_eq!(original, back);
    }
}
