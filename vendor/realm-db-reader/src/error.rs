use std::error::Error;

use thiserror::Error;

use crate::{Row, Value};

/// Errors that occur while reading a Realm file, such as I/O errors or invalid
/// file formats.
#[derive(Debug, Error)]
pub enum RealmFileError {
    /// Error occurred while reading the file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The Realm file is invalid, e.g. due to corrupted data.
    #[error("Invalid Realm file detected: {reason}")]
    InvalidRealmFile {
        /// Reason for detecting the invalid file.
        reason: String,
    },

    /// The Realm file uses a feature that is not supported by this version of
    /// the library.
    #[error("Unsupported Realm feature: {reason}")]
    Unsupported {
        /// Reason for the unsupported feature.
        reason: String,
    },
}

/// Errors that occur while reading a table, such as invalid column names or
/// missing columns.
#[derive(Debug, Error)]
pub enum TableError {
    /// A file error occurred. See [`RealmFileError`].
    #[error("Realm file error: {0}")]
    FileError(#[from] RealmFileError),

    /// Tried to access a table that does not exist.
    #[error("Table not found with name '{name}'")]
    TableNotFound {
        /// Name of the table that was not found.
        name: String,
    },

    /// Tried to access a column that does not exist.
    #[error("Column not found with name '{name}'")]
    ColumnNotFound {
        /// Name of the column that was not found.
        name: String,
    },

    /// Tried to query a column (using
    /// [`find_row_from_indexed_column`](crate::Table::find_row_from_indexed_column)
    /// or
    /// [`find_row_number_from_indexed_column`](crate::Table::find_row_number_from_indexed_column)),
    /// but the column is not indexed.
    #[error("Column '{name}' is not indexed")]
    ColumnNotIndexed {
        /// Name of the column that is not indexed.
        name: String,
    },
}

/// Errors related to value conversions, usually when converting to a model
/// using [`realm_model`](crate::realm_model).
#[derive(Debug, Error)]
pub enum ValueError {
    /// Expected a Table value, found something else. This can happen when
    /// trying to convert a [`Value`] into a `Vec<T>`, if the data is not
    /// structured as expected.
    #[error("Expected a Table value, found {found:?}")]
    ExpectedTable { found: Value },

    /// Expected an array row, but found something else. This can happen when
    /// trying to convert a [`Row`] into a target type. In Realm, if a model has
    /// a field that's the equivalent of, e.g. `Vec<String>`, that is
    /// represented as a subtable, where each row has a single column of type
    /// `String`, with name [`field`](Self::ExpectedArrayRow::field). If this
    /// error occurs, it means the row in the subtable does not have the
    /// expected field.
    #[error("Expected a row with field '{field}', found {found:?}")]
    ExpectedArrayRow {
        /// The name of the field that was expected.
        field: &'static str,
        /// The row that was found, which did not have the expected field.
        found: Row<'static>,
    },

    /// Expected a different type. This could happen if your
    /// [`realm_model`](crate::realm_model) definition is incorrect, and a
    /// column has a different type than expected.
    #[error("Unexpected type: expected {expected:?}, found {found:?}")]
    UnexpectedType {
        /// The expected type.
        expected: &'static str,
        /// The actual value.
        found: Value,
    },

    /// Failed to convert a [`Row`] from a subtable into a `Vec<T>`, because the
    /// underlying `T: TryFrom<Row>>` failed.
    #[error("Failed to convert value in row to Vec<{element_type}>: {source}")]
    VecConversionError {
        /// The type of the elements in the vector.
        element_type: &'static str,
        /// The error that occurred during conversion.
        source: Box<dyn Error>,
    },

    /// Missing field when converting a [`Row`] into a struct. This can happen
    /// if your [`realm_model`](crate::realm_model) definition is incorrect, and
    /// a column doesn't exist, or has a different name than expected.
    #[error(
        "Missing field '{field}' when converting row into '{target_type}' (remaining fields: '{remaining_fields:?}"
    )]
    MissingField {
        /// The name of the missing field.
        field: &'static str,
        /// The type of the target struct.
        target_type: &'static str,
        /// The remaining fields in the row. Note that if the missing field is
        /// not the first field, some fields may be missing from the overall
        /// row, as they were already converted before the missing field was
        /// encountered.
        remaining_fields: Row<'static>,
    },
}

/// Convenience type alias for `Result<T, RealmFileError>`.
pub type RealmResult<T> = std::result::Result<T, RealmFileError>;

/// Convenience type alias for `Result<T, TableError>`.
pub type TableResult<T> = std::result::Result<T, TableError>;

/// Convenience type alias for `Result<T, ValueError>`.
pub type ValueResult<T> = std::result::Result<T, ValueError>;
