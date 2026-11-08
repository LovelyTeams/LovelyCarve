//! Digit-aware natural-order comparison for file names.
//!
//! Mirrors the external `natsort.dll` SQLite collation used by the reference
//! metadata index design: embedded digit runs compare numerically so that
//! `file2` sorts before `file10`, everything else compares by Unicode code
//! point. Registered as the `natsort` collation on every filesystem-index
//! connection in `core::catalog::filedb::filedb_file`.

/// Compare two strings in natural order.
///
/// This is the SQLite collation callback behind every `COLLATE natsort`
/// index, so it runs tens of millions of times per index build and must stay
/// allocation-free. It works on UTF-8 bytes: the only runs the character-wise
/// definition steps over are identical ones, so both strings stay aligned up
/// to the first differing byte. If neither byte there is an ASCII digit, UTF-8
/// byte order equals code-point order and decides; otherwise the digit runs
/// containing that byte (possibly starting inside the shared prefix) decide
/// numerically. On-disk indexes were built with the character-wise definition
/// kept in the tests, so the ordering must stay identical to it.
pub fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let shared = left
        .iter()
        .zip(right)
        .position(|(left_byte, right_byte)| left_byte != right_byte)
        .unwrap_or_else(|| left.len().min(right.len()));
    let (Some(&left_byte), Some(&right_byte)) = (left.get(shared), right.get(shared)) else {
        // One side is a prefix of the other. Extending a string, even inside
        // a digit run, always sorts it later.
        return left.len().cmp(&right.len());
    };
    let run_start = shared
        - left[..shared]
            .iter()
            .rev()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
    let left_digit = left_byte.is_ascii_digit();
    let right_digit = right_byte.is_ascii_digit();
    let inside_numbers =
        (left_digit && right_digit) || (run_start < shared && (left_digit || right_digit));
    if !inside_numbers {
        return left_byte.cmp(&right_byte);
    }
    compare_numeric_runs(
        &left[run_start..digit_run_end(left, shared)],
        &right[run_start..digit_run_end(right, shared)],
    )
}

/// End (exclusive) of the ASCII digit run continuing at `from`.
fn digit_run_end(bytes: &[u8], from: usize) -> usize {
    from + bytes[from..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

/// Compare ASCII digit runs numerically: by trimmed length first (more digits
/// means a larger number once leading zeros are ignored), then by digits.
fn compare_numeric_runs(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    let left_trimmed = trim_leading_zeros(left);
    let right_trimmed = trim_leading_zeros(right);
    left_trimmed
        .len()
        .cmp(&right_trimmed.len())
        .then_with(|| left_trimmed.cmp(right_trimmed))
        // Equal numeric value but different zero padding ("007" vs "7"):
        // keep the comparison deterministic by shorter-run-first.
        .then_with(|| left.len().cmp(&right.len()))
}

fn trim_leading_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&byte| byte == b'0').count();
    &digits[zeros..]
}

/// Register the `natsort` collation on a case database connection.
pub fn register_collation(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.create_collation("natsort", natural_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn digits_compare_numerically() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a9b1", "a9b2"), Ordering::Less);
    }

    #[test]
    fn equal_numbers_with_more_padding_sort_last() {
        // Numeric value wins first; equal values need a deterministic total
        // order for the SQL collation, so longer zero padding sorts after.
        assert_eq!(natural_cmp("7", "007"), Ordering::Less);
        assert_eq!(natural_cmp("007", "7"), Ordering::Greater);
        assert_eq!(natural_cmp("07", "007"), Ordering::Less);
    }

    #[test]
    fn plain_text_compares_by_code_point() {
        assert_eq!(natural_cmp("abc", "abd"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn chinese_names_sort_by_code_point() {
        assert_eq!(natural_cmp("证据1", "证据2"), Ordering::Less);
        assert_eq!(natural_cmp("档案10", "档案9"), Ordering::Greater);
    }

    #[test]
    fn mixed_digit_and_text_boundaries() {
        assert_eq!(natural_cmp("a1b", "a1c"), Ordering::Less);
        assert_eq!(natural_cmp("a1", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("1a", "1b"), Ordering::Less);
    }

    #[test]
    fn sqlite_collation_orders_naturally() {
        let conn = rusqlite::Connection::open_in_memory().expect("open memory db");
        register_collation(&conn).expect("register natsort");
        conn.execute_batch(
            "CREATE TABLE items (name TEXT NOT NULL);
             INSERT INTO items (name) VALUES ('file10'), ('file2'), ('file1'), ('file20');",
        )
        .expect("seed");
        let names: Vec<String> = conn
            .prepare("SELECT name FROM items ORDER BY name COLLATE natsort")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(names, vec!["file1", "file2", "file10", "file20"]);
    }

    /// The original character-wise definition every existing on-disk
    /// `COLLATE natsort` index was built with. `natural_cmp` must agree with it
    /// on every input, or SQLite would walk those indexes in a different order
    /// than they were written in.
    fn reference_natural_cmp(left: &str, right: &str) -> Ordering {
        let mut left_chars = left.chars().peekable();
        let mut right_chars = right.chars().peekable();
        loop {
            match (left_chars.peek().copied(), right_chars.peek().copied()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(l), Some(r)) => {
                    if l.is_ascii_digit() && r.is_ascii_digit() {
                        let left_digits: String = left_chars
                            .clone()
                            .take_while(char::is_ascii_digit)
                            .collect();
                        let right_digits: String = right_chars
                            .clone()
                            .take_while(char::is_ascii_digit)
                            .collect();
                        let ordering =
                            compare_numeric_runs(left_digits.as_bytes(), right_digits.as_bytes());
                        if ordering != Ordering::Equal {
                            return ordering;
                        }
                        for _ in 0..left_digits.len() {
                            left_chars.next();
                        }
                        for _ in 0..right_digits.len() {
                            right_chars.next();
                        }
                    } else {
                        let ordering = l.cmp(&r);
                        if ordering != Ordering::Equal {
                            return ordering;
                        }
                        left_chars.next();
                        right_chars.next();
                    }
                }
            }
        }
    }

    /// splitmix64: deterministic pseudo-random stream without a dependency.
    struct Sequence(u64);

    impl Sequence {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut mixed = self.0;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            mixed ^ (mixed >> 31)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn name(&mut self) -> String {
            const PIECES: &[&str] = &[
                "0",
                "1",
                "2",
                "9",
                "00",
                "007",
                "10",
                "a",
                "b",
                "Z",
                "_",
                "-",
                ".",
                " ",
                "(",
                "~",
                "é",
                "中",
                "证据",
                "\u{10000}",
                "/",
                "A1",
                "x0y",
                "9z",
                "!",
                "ß",
            ];
            (0..self.below(7))
                .map(|_| PIECES[self.below(PIECES.len())])
                .collect()
        }
    }

    #[test]
    fn byte_comparison_matches_reference_definition() {
        let mut sequence = Sequence(0x5EED);
        for _ in 0..300_000 {
            let left = sequence.name();
            // Half the pairs share a prefix so digit runs straddle the first
            // difference.
            let right = if sequence.below(2) == 0 {
                let cut = left
                    .char_indices()
                    .map(|(index, _)| index)
                    .nth(sequence.below(left.chars().count() + 1))
                    .unwrap_or(left.len());
                format!("{}{}", &left[..cut], sequence.name())
            } else {
                sequence.name()
            };
            assert_eq!(
                natural_cmp(&left, &right),
                reference_natural_cmp(&left, &right),
                "{left:?} vs {right:?}"
            );
        }
    }

    #[test]
    fn digit_runs_straddling_the_first_difference() {
        for (left, right) in [
            ("a1", "a01"),
            ("a10", "a9"),
            ("a1x", "a1"),
            ("a19", "a1!"),
            ("x05", "x050"),
            ("v2.10", "v2.9"),
            ("img_0010", "img_010"),
            ("9", "a"),
            ("1中", "1a"),
        ] {
            assert_eq!(
                natural_cmp(left, right),
                reference_natural_cmp(left, right),
                "{left:?} vs {right:?}"
            );
            assert_eq!(
                natural_cmp(right, left),
                reference_natural_cmp(right, left),
                "{right:?} vs {left:?}"
            );
        }
    }
}
