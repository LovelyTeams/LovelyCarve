use chrono::{DateTime, Utc};

use crate::table::Row;

mod from;
mod into;

pub(crate) const ARRAY_VALUE_KEY: &str = "!ARRAY_VALUE";

// Should match [`crate::spec::ColumnType`]
/// A single value from a Realm database. Represents one row in one column.
#[derive(Debug, Clone)]
pub enum Value {
    /// A signed integer value. Integers may be nullable in Realm, in which case
    /// they are represented as [`None`].
    Int(i64),
    /// A boolean value. Booleans may be nullable in Realm, in which case
    /// they are represented as [`None`].
    Bool(bool),
    /// A string value. Strings may be nullable in Realm, in which case
    /// they are represented as [`None`].
    String(String),
    /// Currently unsupported.
    #[doc(hidden)]
    OldStringEnum(String),
    /// A binary blob value. Binary blobs may be nullable in Realm, in which case
    /// they are represented as [`None`].
    Binary(Vec<u8>),
    /// A subtable value. Tables in Realm may have a column that contains an
    /// entire table. In that case, upon loading the row, the subtable and all
    /// its rows are loaded.
    Table(Vec<Row<'static>>),
    /// Currently unsupported.
    #[doc(hidden)]
    OldMixed,
    /// Currently unsupported.
    #[doc(hidden)]
    OldDateTime,
    /// A timestamp value, represented using the `chrono` crate. Timestamps may
    /// be nullable in Realm, in which case they are represented as [`None`].
    Timestamp(DateTime<Utc>),
    /// A floating-point value.
    Float(f32),
    /// A double-precision floating-point value.
    Double(f64),
    /// Currently unsupported.
    #[doc(hidden)]
    Reserved4,
    /// A link to a row in a given table. If the link is null, it is represented
    /// as [`None`].
    Link(Link),
    /// A list of links to rows in a given table. If a row has no links, this
    /// will be an empty list.
    LinkList(Vec<Link>),
    /// A backlink. In cases where table A maintains a link (see [`Link`] or
    /// [`LinkList`](`Self::LinkList`)), table B maintains a backlink to table
    /// A. You can use this to navigate back to the parent row in a has-one
    /// relationship.
    ///
    /// Backlinks may be nullable in Realm, in which case they are represented
    /// as [`None`].
    ///
    /// Backlinks are special, as their containing columns are unnamed, and thus
    /// cannot be retrieved using [`Row::get`](`crate::Row::get`). Instead, you
    /// can use [`Row::backlinks`](`crate::Row::backlinks`) to retrieve the
    /// backlinks. See the documentation for
    /// [`realm_model!`](`crate::realm_model`) for how to incorporate backlinks
    /// into your model classes.
    BackLink(Backlink),

    /// A null value.
    None,
}

impl Value {
    /// Returns true if the value is [`None`].
    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }
}

/// A link to a single row in a given table.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Link {
    /// The table number of the target table, in the Realm
    /// [`Group`](`crate::Group`).
    pub target_table_number: usize,
    /// The row number this link points to.
    pub row_number: usize,
}

impl Link {
    /// Create a new link to a row in a table.
    pub fn new(target_table_number: usize, row_number: usize) -> Self {
        Self {
            target_table_number,
            row_number,
        }
    }
}

/// A backlink to one or more rows in a given table. This is the opposite end of
/// a [`Link`]. Note that [`row_numbers`](`Self::row_numbers`) is guaranteed to
/// be non-empty. An empty backlink would be represented as [`Value::None`].
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Backlink {
    /// The table number of the origin table, in the Realm
    /// [`Group`](`crate::Group`).
    pub origin_table_number: usize,
    /// The column number of the origin column, in the
    /// [`Table`](`crate::Table`).
    pub origin_column_number: usize,
    /// The row numbers this backlink points to, i.e. the rows in the origin
    /// table that have [`Link`]s to this row.
    pub row_numbers: Vec<usize>,
}

impl Backlink {
    pub fn new(
        origin_table_number: usize,
        origin_column_number: usize,
        row_numbers: Vec<usize>,
    ) -> Self {
        Self {
            origin_table_number,
            origin_column_number,
            row_numbers,
        }
    }
}
