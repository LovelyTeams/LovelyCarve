//! SQLite-aware preview for files inside a forensic image.
//!
//! Reads the requested file (and, when present, its `-wal` sidecar) into a
//! process-local SQLite VFS, opens it via `rusqlite` in read-only mode, and
//! returns a [`SqliteDigest`] describing every table / view: column
//! definitions, row count, and a head sample of rows. SQLite replays the
//! WAL's committed frames itself, so the preview shows the database as its
//! application last saw it. A `-shm` is not needed for that, and a
//! `-journal` is left out: a hot journal would make the read-only open fail,
//! and the deleted-record view reads it instead. The VFS entries are private
//! to the process and are dropped when the cached preview session is
//! replaced.
//!
//! Errors come in two flavors:
//! - whole-database failures (not a SQLite file, locked, encrypted)
//!   bubble up as `Err`. The caller turns them into a string the UI
//!   shows in place of the preview.
//! - per-table failures (broken page, virtual table without a backing
//!   module, `PRAGMA table_info` failing on an exotic schema) are
//!   captured into [`SqliteTable::error`] so one bad table doesn't
//!   shadow the rest of the database.

mod session;
pub use session::cached_session;
#[cfg(test)]
use session::*;
pub(crate) use session::{read_node_prefix, sidecar_nodes};
mod connection;
use connection::*;
mod query;
use query::*;
mod schema;
pub(crate) use schema::truncate_text;
use schema::*;

use std::sync::Arc;
#[cfg(test)]
use std::{fs::File, io::Read, path::Path};

use anyhow::Context;
#[cfg(test)]
use rusqlite::OpenFlags;
use rusqlite::{types::ValueRef, Connection};
use serde::{Deserialize, Serialize};

use crate::filesystem::tree::fs_tree::{self, FsNode, NodeKind};
use crate::preview::sqlite::sqlite_vfs;

/// Hard ceiling on the SQLite payload we'll attempt to open. Past this
/// we still import the prefix into the preview VFS, but if rusqlite
/// can't open it we don't bother retrying without the cap. Forensic
/// SQLite stores tend to live well under this — Chrome `History` is
/// ~50 MB on a heavy user, WeChat MM.sqlite ~200 MB.
const MAX_PREVIEW_BYTES: u64 = 512 * 1024 * 1024;

/// Per-table head-sample row cap. `LIMIT N` against the user's table.
const DEFAULT_ROW_SAMPLE: usize = 50;

/// Hard ceiling on a single paged `fs_sqlite_table_rows` request. The
/// frontend asks for a page at a time; this clamps a misbehaving caller
/// from pulling an unbounded result set into one JSON payload.
const MAX_PAGE_ROWS: i64 = 1000;

/// Per-cell text length cap. Long blobs / strings get truncated with
/// an ellipsis tag so JSON payloads stay small.
const MAX_CELL_TEXT: usize = 200;

#[derive(Debug, Serialize)]
pub struct SqliteDigest {
    pub header: SqliteHeader,
    pub tables: Vec<SqliteTable>,
    pub indexes: Vec<SqliteIndex>,
    /// True when the database was opened through SQLCipher (the on-disk
    /// bytes were encrypted and a key successfully decrypted them). The
    /// UI shows a "decrypted" badge so the examiner knows the preview
    /// came from ciphertext.
    pub encrypted: bool,
    /// Filename of the `-wal` sidecar imported alongside the main DB, when
    /// one was found. The UI shows it so the user knows the preview
    /// reflects WAL state, not the last checkpoint only.
    pub sidecars: Vec<String>,
    pub bytes_copied: u64,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct SqliteHeader {
    pub page_size: u32,
    pub user_version: i64,
    pub application_id: i64,
    pub schema_version: i64,
    pub encoding: String,
    pub journal_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SqliteTable {
    pub name: String,
    /// `"table"` / `"view"`. Mirrors `sqlite_master.type`.
    pub kind: String,
    pub columns: Vec<SqliteColumn>,
    pub row_count: Option<i64>,
    pub sample_rows: Vec<Vec<SqliteCell>>,
    pub error: Option<String>,
    /// The CREATE statement. Useful when the preview UI wants to
    /// expose the full schema without re-querying.
    pub create_sql: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SqliteColumn {
    pub name: String,
    pub type_name: String,
    pub notnull: bool,
    pub default_value: Option<String>,
    pub pk: i64,
}

#[derive(Debug, Serialize)]
pub struct SqliteIndex {
    pub name: String,
    pub table: String,
    pub unique: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum SqliteCell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// Length in bytes. The actual blob is *not* serialised — the UI
    /// shows `<BLOB N B>` to keep the payload bounded.
    Blob {
        size: usize,
    },
}

/// One SQLite preview loaded into the private process-local VFS. Held in
/// [`AppState`] as a single-slot cache so paging / sorting the same
/// database reuses the imported bytes instead of re-reading the image
/// node every request. Dropping it removes the VFS entries.
pub struct SqliteSession {
    image_id: String,
    node_id: String,
    main_name: String,
    sidecars: Vec<String>,
    bytes_copied: u64,
    truncated: bool,
    head16: Option<[u8; 16]>,
}

impl SqliteSession {
    /// The evidence image this cached database was imported from.
    pub fn image_id(&self) -> &str {
        &self.image_id
    }
}

impl Drop for SqliteSession {
    fn drop(&mut self) {
        sqlite_vfs::delete_file(&self.main_name);
        // Imported, or created empty by SQLite for a WAL-mode header.
        sqlite_vfs::delete_file(&wal_vfs_name(&self.main_name));
    }
}

/// The VFS name SQLite derives for the WAL of the database `main_name`.
fn wal_vfs_name(main_name: &str) -> String {
    format!("{main_name}-wal")
}

/// A single page of a table's rows, served by `fs_sqlite_table_rows`.
#[derive(Debug, Serialize)]
pub struct SqliteRows {
    pub columns: Vec<SqliteColumn>,
    pub rows: Vec<Vec<SqliteCell>>,
    /// `COUNT(*)` over the whole table — drives the frontend paginator.
    pub total: i64,
    /// Echoed back (clamped) so the caller knows which slice this is.
    pub offset: i64,
    pub returned: usize,
}

/// Decryption material for an encrypted (SQLCipher) database. All fields
/// optional — an all-`None` value means "no key" (a plaintext database
/// opens fine; an encrypted one trips a `SQLITE_LOCKED` error the UI
/// turns into a key prompt). `passphrase` and `raw_hex_key` are mutually
/// exclusive; `raw_hex_key` wins if both are set. The compatibility /
/// page-size / kdf-iter knobs cover non-default and legacy databases
/// (e.g. WeChat: raw key + 1024-byte pages + low kdf_iter).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteKey {
    /// Text passphrase — applied as `PRAGMA key = '…'` (PBKDF2-derived).
    pub passphrase: Option<String>,
    /// 256-bit raw key as hex (64 chars), optionally followed by a
    /// 16-byte salt (96 chars total) — applied as `PRAGMA key = "x'…'"`.
    pub raw_hex_key: Option<String>,
    /// `PRAGMA cipher_compatibility = N` (1‑4); selects the SQLCipher
    /// default parameter set for that major version.
    pub compatibility: Option<u8>,
    /// `PRAGMA cipher_page_size` override (bytes).
    pub page_size: Option<u32>,
    /// `PRAGMA kdf_iter` override (PBKDF2 iterations).
    pub kdf_iter: Option<u32>,
}

impl SqliteKey {
    /// Whether any key material (passphrase or raw key) was supplied.
    fn has_material(&self) -> bool {
        self.passphrase.as_deref().is_some_and(|s| !s.is_empty())
            || self
                .raw_hex_key
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }
}

/// Probe the first 16 bytes of `bytes` to decide whether the file
/// looks like a SQLite 3 database. The standard header is the literal
/// ASCII string `"SQLite format 3\0"`.
pub fn looks_like_sqlite(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && &bytes[..16] == b"SQLite format 3\0"
}

/// Schema + head-sample digest, served by `fs_sqlite_preview`. The caller
/// retains the session but releases the cache lock before running queries.
/// `key` decrypts an encrypted (SQLCipher) database.
pub fn preview_session(cached: &SqliteSession, key: &SqliteKey) -> anyhow::Result<SqliteDigest> {
    let digest = read_digest(cached, key)?;
    Ok(SqliteDigest {
        header: digest.header,
        tables: digest.tables,
        indexes: digest.indexes,
        encrypted: digest.encrypted,
        sidecars: cached.sidecars.clone(),
        bytes_copied: cached.bytes_copied,
        truncated: cached.truncated,
    })
}

/// One page of a single table, with optional column sort. `order_by`
/// is validated against the table's real column names before it touches
/// SQL, so an arbitrary string can't be injected as an ORDER BY clause.
/// `key` decrypts an encrypted database (reuses the cached VFS import).
#[allow(clippy::too_many_arguments)]
pub fn table_rows_for_session(
    cached: &SqliteSession,
    key: &SqliteKey,
    table: &str,
    offset: i64,
    limit: i64,
    order_by: Option<&str>,
    descending: bool,
) -> anyhow::Result<SqliteRows> {
    read_rows(cached, key, table, offset, limit, order_by, descending)
}

#[derive(Debug)]
struct InnerDigest {
    header: SqliteHeader,
    tables: Vec<SqliteTable>,
    indexes: Vec<SqliteIndex>,
    encrypted: bool,
}

/// A `-wal` / `-shm` / `-journal` file next to a database.
pub(crate) struct SidecarNode {
    pub(crate) suffix: &'static str,
    pub(crate) node_id: String,
    pub(crate) name: String,
}

#[cfg(test)]
mod tests;
