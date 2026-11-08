//! Best-effort, **dependency-free** attribution primitives that reconnect a
//! carved deleted row to the live table it most plausibly belonged to.
//!
//! A deleted row in free space has lost its hard table linkage (the b-tree that
//! owned it no longer points at it). This module supplies the two structural
//! facts the forensic layer needs to reattach it honestly:
//!
//! 1. [`column_names`] — a hand-rolled `CREATE TABLE` column-name extractor (no
//!    SQL-parser dependency). It returns the declared column names when it can
//!    parse them with confidence, or `None` so the caller falls back to generic
//!    `c0..cN` rather than emit wrong names.
//! 2. [`column_affinity`] — each column's declared *affinity* (file-format
//!    §3.1), the shape used to match a freed row whose page linkage is gone.
//!
//! Pure string/structure work: panic-free, `forbid(unsafe)`, no new deps.

/// A live (schema-present) table, as read from `sqlite_master`: its name,
/// b-tree rootpage, and per-column shape used to attribute a freed row whose
/// page linkage is gone.
///
/// `column_names` is `Some` only when the `CREATE TABLE` statement parsed with
/// confidence AND its column count equals `affinities.len()`; otherwise `None`,
/// so a caller falls back to generic `c0..cN` rather than risk wrong names. The
/// affinities are always present (one per parsed column) and drive shape
/// matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTable {
    /// Table name from `sqlite_master.name`.
    pub name: String,
    /// 1-based b-tree rootpage from `sqlite_master.rootpage`.
    pub rootpage: u32,
    /// Declared column names, or `None` when low-confidence (caller uses `c0..`).
    pub column_names: Option<Vec<String>>,
    /// Declared column affinity for each column, in column order.
    pub affinities: Vec<Affinity>,
    /// The table's `CREATE TABLE` statement (`sqlite_master.sql`), threaded so a
    /// consumer can re-derive schema properties (e.g. `AUTOINCREMENT`-ness via
    /// [`crate::is_autoincrement`]) without re-reading the schema. Empty when the
    /// schema row carried no/invalid sql (a damaged schema).
    pub create_sql: String,
}

/// Column type **affinity** as defined by the `SQLite` file format (§3.1, "Type
/// Affinity"). Derived from a column's declared type by the documented
/// substring rules, in priority order. A column with no declared type is
/// `Blob` (SQLite's "BLOB or no datatype" affinity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    /// Declared type contains "INT" → INTEGER affinity.
    Integer,
    /// Declared type contains "CHAR", "CLOB", or "TEXT" → TEXT affinity.
    Text,
    /// Declared type contains "BLOB", or no declared type → BLOB affinity.
    Blob,
    /// Declared type contains "REAL", "FLOA", or "DOUB" → REAL affinity.
    Real,
    /// Anything else → NUMERIC affinity.
    Numeric,
}

/// Compute a column's [`Affinity`] from its declared type string per the
/// file-format §3.1 rules, applied in the documented priority order:
/// INT → TEXT → BLOB/none → REAL → NUMERIC. Case-insensitive; an empty (no)
/// declared type is [`Affinity::Blob`].
#[must_use]
pub fn column_affinity(declared_type: &str) -> Affinity {
    let t = declared_type.to_ascii_uppercase();
    if t.contains("INT") {
        Affinity::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        Affinity::Text
    } else if t.is_empty() || t.contains("BLOB") {
        Affinity::Blob
    } else if t.contains("REAL") || t.contains("FLOA") || t.contains("DOUB") {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

/// Best-effort extraction of `(column_name, declared_type)` for each column
/// declared in a `CREATE TABLE` statement, **without** a SQL-parser dependency.
///
/// Algorithm (deliberately conservative — wrong names are worse than no names):
/// 1. Take the outermost parenthesized list (first `(` to its matching `)`,
///    tracking nesting and skipping over `'…'`, `"…"`, `` `…` ``, and `[…]`
///    so a comma or paren inside a string/identifier/`CHECK(...)` never splits).
/// 2. Split that list on **top-level** commas (depth 0).
/// 3. For each part, read the first identifier — handling `"quoted"`,
///    `` `backtick` ``, `[bracketed]`, and bare names. Skip a part whose first
///    word is a table-level constraint keyword (`CONSTRAINT`, `PRIMARY`,
///    `UNIQUE`, `CHECK`, `FOREIGN`, `KEY`) — those declare constraints, not
///    columns.
/// 4. The remaining tokens of the part (up to the next top-level comma) form the
///    declared type; an empty remainder is no declared type.
///
/// Returns `None` when no parenthesized list is found or no column survives —
/// the low-confidence signal the caller turns into a `c0..cN` fallback.
#[must_use]
pub fn column_defs(create_sql: &str) -> Option<Vec<(String, String)>> {
    let inner = outermost_paren_body(create_sql)?;
    let parts = split_top_level(inner);
    let mut cols = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut tokens = ColumnTokens::new(part);
        let Some(name) = tokens.next_identifier() else {
            continue; // cov:unreachable: a non-empty part always has a leading token
        };
        if is_table_constraint_keyword(&name) {
            continue;
        }
        let declared_type = tokens.rest_as_type();
        cols.push((name, declared_type));
    }
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

/// Just the column **names** from [`column_defs`] (the common need). `None` when
/// the body cannot be parsed with confidence.
#[must_use]
pub fn column_names(create_sql: &str) -> Option<Vec<String>> {
    Some(
        column_defs(create_sql)?
            .into_iter()
            .map(|(n, _)| n)
            .collect(),
    )
}

/// The substring between the first top-level `(` and its matching `)`,
/// respecting string/identifier quoting so a delimiter inside `'…'` / `"…"` /
/// `` `…` `` / `[…]` does not affect nesting. `None` if unbalanced or absent.
fn outermost_paren_body(sql: &str) -> Option<&str> {
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    let mut start = None;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => quote = Some(c),
            b'[' => quote = Some(b']'),
            b'(' => {
                if start.is_none() {
                    start = Some(i + 1);
                    depth = 1;
                } else {
                    depth += 1;
                }
            }
            b')' if start.is_some() => {
                depth -= 1;
                if depth == 0 {
                    return sql.get(start?..i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split a column-list body on **top-level** commas (depth 0), respecting nested
/// parens and the four quote styles so a comma inside `CHECK(a,b)` or a quoted
/// string never splits a column.
fn split_top_level(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' | b'`' => quote = Some(c),
            b'[' => quote = Some(b']'),
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                if let Some(p) = body.get(start..i) {
                    parts.push(p);
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(p) = body.get(start..) {
        parts.push(p);
    }
    parts
}

/// Whether `word` (already extracted as a part's first identifier) names a
/// table-level constraint clause rather than a column.
fn is_table_constraint_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN" | "KEY"
    )
}

/// A tiny cursor over one column-definition part that yields the leading
/// identifier (quoted/bracketed/backtick/bare) and then the remaining tokens as
/// the declared type.
struct ColumnTokens<'a> {
    rest: &'a str,
}

impl<'a> ColumnTokens<'a> {
    fn new(part: &'a str) -> Self {
        Self { rest: part }
    }

    /// Read and consume the leading identifier of the part, unescaping the
    /// quote styles. `SQLite` tolerates single-quoted identifiers in
    /// `CREATE TABLE` (seen in real fixtures), so `'…'` is unquoted like `"…"` /
    /// `` `…` ``. `None` when the part has no identifier start.
    fn next_identifier(&mut self) -> Option<String> {
        let s = self.rest.trim_start();
        let bytes = s.as_bytes();
        let first = *bytes.first()?;
        let (name, consumed) = match first {
            b'"' | b'`' | b'\'' => read_quoted(s, char::from(first)),
            b'[' => read_quoted(s, ']'),
            _ => read_bare(s),
        }?;
        self.rest = s.get(consumed..).unwrap_or("");
        Some(name)
    }

    /// The remainder of the part (after the column name), trimmed, as the
    /// declared type. We keep the whole remainder so `VARCHAR(20)` and
    /// `DOUBLE PRECISION` survive. An empty remainder means no declared type.
    fn rest_as_type(&self) -> String {
        self.rest.trim().to_string()
    }
}

/// Read a quoted identifier opened by the first byte (closing delimiter
/// `close`), returning `(unescaped_name, bytes_consumed_including_delimiters)`.
/// A doubled closing delimiter (`""` inside `"…"`) is an escaped literal per
/// SQL rules (not applicable to `[...]`).
fn read_quoted(s: &str, close: char) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let close_b = close as u8;
    let mut name = String::new();
    let mut i = 1usize; // skip opening delimiter
    while i < bytes.len() {
        let c = bytes[i];
        if c == close_b {
            if close_b != b']' && bytes.get(i + 1) == Some(&close_b) {
                name.push(close);
                i += 2;
                continue;
            }
            return Some((name, i + 1));
        }
        name.push(char::from(c));
        i += 1;
    }
    None // unterminated quote → low confidence
}

/// Read a bare (unquoted) identifier: every byte up to the first whitespace or
/// `(`. Returns `(name, bytes_consumed)`.
fn read_bare(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() || c == b'(' {
            break;
        }
        i += 1;
    }
    if i == 0 {
        return None; // cov:unreachable: callers trim and check non-empty before this
    }
    Some((s.get(..i)?.to_string(), i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affinity_rules_match_spec_priority() {
        assert_eq!(column_affinity("INTEGER"), Affinity::Integer);
        assert_eq!(column_affinity("BIGINT"), Affinity::Integer);
        assert_eq!(column_affinity("VARCHAR(255)"), Affinity::Text);
        assert_eq!(column_affinity("CLOB"), Affinity::Text);
        assert_eq!(column_affinity("TEXT"), Affinity::Text);
        assert_eq!(column_affinity("BLOB"), Affinity::Blob);
        assert_eq!(column_affinity(""), Affinity::Blob);
        assert_eq!(column_affinity("REAL"), Affinity::Real);
        assert_eq!(column_affinity("DOUBLE"), Affinity::Real);
        assert_eq!(column_affinity("FLOAT"), Affinity::Real);
        assert_eq!(column_affinity("NUMERIC"), Affinity::Numeric);
        assert_eq!(column_affinity("DATETIME"), Affinity::Numeric);
        assert_eq!(column_affinity("BOOLEAN"), Affinity::Numeric);
    }

    #[test]
    fn plain_columns() {
        let cols = column_names("CREATE TABLE t (id INTEGER, name TEXT, age INT)").unwrap();
        assert_eq!(cols, vec!["id", "name", "age"]);
    }

    #[test]
    fn quoted_bracketed_backtick_identifiers() {
        let sql = r#"CREATE TABLE "My Tbl" ("first name" TEXT, [second] INTEGER, `third` BLOB)"#;
        let cols = column_names(sql).unwrap();
        assert_eq!(cols, vec!["first name", "second", "third"]);
    }

    #[test]
    fn skips_table_level_constraints() {
        let sql = "CREATE TABLE t (\
            id INTEGER PRIMARY KEY, \
            a TEXT, \
            b REAL, \
            PRIMARY KEY (id), \
            UNIQUE (a), \
            CONSTRAINT fk FOREIGN KEY (b) REFERENCES other(x), \
            CHECK (a <> b))";
        let cols = column_names(sql).unwrap();
        assert_eq!(cols, vec!["id", "a", "b"]);
    }

    #[test]
    fn check_constraint_with_commas_does_not_oversplit() {
        let sql = "CREATE TABLE t (x INTEGER, y INTEGER, CHECK (x IN (1, 2, 3)))";
        let cols = column_names(sql).unwrap();
        assert_eq!(cols, vec!["x", "y"]);
    }

    #[test]
    fn typed_columns_with_parenthesized_and_multiword_types() {
        let sql = "CREATE TABLE t (a VARCHAR(20), b DOUBLE PRECISION, c DECIMAL(10,2))";
        let defs = column_defs(sql).unwrap();
        assert_eq!(defs[0], ("a".to_string(), "VARCHAR(20)".to_string()));
        assert_eq!(defs[1], ("b".to_string(), "DOUBLE PRECISION".to_string()));
        assert_eq!(defs[2], ("c".to_string(), "DECIMAL(10,2)".to_string()));
        assert_eq!(column_affinity(&defs[0].1), Affinity::Text);
        assert_eq!(column_affinity(&defs[1].1), Affinity::Real);
        assert_eq!(column_affinity(&defs[2].1), Affinity::Numeric);
    }

    #[test]
    fn no_parens_is_low_confidence_none() {
        assert_eq!(column_names("CREATE TABLE t"), None);
        assert_eq!(column_names("not even ddl"), None);
    }

    #[test]
    fn unterminated_quote_yields_none() {
        assert_eq!(column_names(r#"CREATE TABLE t ("oops)"#), None);
    }

    #[test]
    fn empty_column_list_is_none() {
        assert_eq!(column_names("CREATE TABLE t ()"), None);
    }

    #[test]
    fn single_quoted_identifiers_are_unquoted() {
        // SQLite tolerates single-quoted column names in CREATE TABLE (seen in
        // the real nemetz 0D-01 fixture). The real names are id/name, not 'id'.
        let sql = "CREATE TABLE users (\n\t'id' INT NOT NULL,\n\t'name' TEXT NULL)";
        let cols = column_names(sql).unwrap();
        assert_eq!(cols, vec!["id", "name"]);
    }
}
