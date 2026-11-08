//! Pure-Rust **writer** that rebuilds a valid `SQLite` database file from carved
//! deleted records (the inverse of the reader in [`crate`]).
//!
//! [`build_recovered_db`] takes a set of [`RebuildRow`]s and returns the bytes of
//! a single-table `SQLite` database (`recovered_records`) holding one row per
//! carved record, each cell stored in its **native** storage class — an
//! `INTEGER`/`REAL`/`TEXT`/`BLOB` is written as itself, so a recovered BLOB is
//! preserved losslessly rather than stringified. The table b-tree is **bulk
//! loaded**: leaves are packed in rowid order and interior pages built bottom-up,
//! because every row is known up front (no insertion/splitting). Cells larger
//! than the usable page size spill onto **overflow-page chains** per the file
//! format (§1.6), so a large recovered BLOB/TEXT survives intact.
//!
//! [`build_recovered_db_with_fragments`] additionally emits a **second** table,
//! `recovered_fragments`, in the same file — the Tier-2 partial rows kept
//! structurally separate from the full rows (a fragment is never mixed into
//! `recovered_records`). Both tables are built from one generic internal table
//! spec, so the page allocation, bulk-load and overflow handling are shared;
//! passing no fragment set reproduces the single-table bytes exactly.
//!
//! The output re-opens with [`crate::Database::open`] (the independent reader)
//! and is read identically by the real `sqlite3` engine — the writer's two
//! oracles. No new dependencies, no unsafe, panic-free.

use crate::{enc_varint_into, local_payload_len, Value};

/// One carved record to materialize as a row of the rebuilt `recovered_records`
/// table. The CLI maps its `CarvedRecord` onto this; the writer owns the
/// `SQLite`-format encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct RebuildRow {
    /// 1-based source page the record was carved from (stored in `_page`).
    pub page: u32,
    /// Byte offset of the cell within that page (stored in `_offset`).
    pub offset: usize,
    /// Carved rowid, or `None` when unknown/destroyed (stored as `_rowid` NULL).
    pub rowid: Option<i64>,
    /// Recovery-source label (stored in `_source`).
    pub source: String,
    /// Heuristic confidence (stored in `_confidence`).
    pub confidence: f32,
    /// Decoded cells, in column order, stored natively in `c0..cN`.
    pub cells: Vec<Value>,
}

/// One carved **fragment** (a Tier-2 partial row) to materialize as a row of the
/// rebuilt `recovered_fragments` table. A fragment has no rowid (it was clobbered)
/// and only a *subset* of its columns survived; each survivor is placed at its own
/// native column index, with every other column left NULL. The CLI maps its
/// `CarvedFragment` onto this; the writer owns the `SQLite`-format encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct FragmentRow {
    /// 1-based source page the fragment was salvaged from (stored in `_page`).
    pub page: u32,
    /// Byte offset of the failed cell's anchor within that page (stored in
    /// `_offset`).
    pub offset: usize,
    /// Number of the row's columns that did NOT decode (stored in `_missing`).
    pub missing: usize,
    /// Heuristic confidence (stored in `_confidence`).
    pub confidence: f32,
    /// `(column_index, value)` for each column that decoded cleanly. Each value is
    /// stored natively at `c{column_index}`; columns not listed are NULL.
    pub surviving: Vec<(usize, Value)>,
}

/// One table to materialize in a **multi-table** rebuilt database
/// ([`build_recovered_db_tables`]): an arbitrary (SQL-identifier-quoted) table
/// name, its full ordered column names, and its rows already projected to
/// exactly `columns.len()` values each.
///
/// This is the general surface the CLI's table-attribution feature targets: it
/// emits one `recovered_<table>` per attributed live table (with that table's
/// real column names), plus `recovered_inferred`, `recovered_unattributed`, and
/// `recovered_fragments`. The caller owns the column layout and row projection;
/// the writer owns the `SQLite`-format encoding and identifier quoting.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredTable {
    /// Table name (quoted as a SQL identifier in the emitted `CREATE TABLE`).
    pub name: String,
    /// Full ordered column names (quoted as SQL identifiers). Each row carries
    /// exactly this many values.
    pub columns: Vec<String>,
    /// Rows, each already projected to `columns.len()` values in column order.
    pub rows: Vec<Vec<Value>>,
}

/// Build the bytes of a valid `SQLite` database holding **N arbitrary tables**,
/// each with its own name and column set, every identifier safely quoted.
///
/// The general multi-table writer behind the table-attribution feature. Each
/// [`RecoveredTable`] becomes one b-tree (bulk-loaded, overflow-spilling exactly
/// like the records/fragments path) with a `sqlite_master` schema row. Table and
/// column names are emitted as quoted SQL identifiers, so a column named `order`
/// or `first name` is legal. Rows are written with a synthetic rowid `1..=N`
/// (the recovered rows have no meaningful rowid of their own — provenance lives
/// in the `_rowid` column when present).
#[must_use]
pub fn build_recovered_db_tables(tables: &[RecoveredTable]) -> Vec<u8> {
    let specs: Vec<TableSpec> = tables
        .iter()
        .map(|t| TableSpec {
            name: t.name.clone(),
            create_sql: create_table_sql(&t.name, &t.columns),
            rows: t.rows.clone(),
        })
        .collect();
    build_from_specs(&specs)
}

/// Logical page size of the rebuilt database. 4096 is `SQLite`'s modern default
/// and a power of two in the legal range; the reader/oracle accept any valid
/// size, so this is an internal constant, never a knob.
const PAGE_SIZE: usize = 4096;

/// Reserved bytes per page (none) → usable size equals the page size.
const USABLE: usize = PAGE_SIZE;

/// Bytes available for content on an overflow page (4-byte next-page pointer
/// prefix, then payload) — file-format §1.6.
const OVERFLOW_PAYLOAD: usize = USABLE - 4;

/// The number of fixed leading columns every rebuilt `recovered_records` row
/// carries before its recovered cells: `_page, _offset, _rowid, _source,
/// _confidence`.
const LEAD_COLS: usize = 5;

/// One table to materialize in the rebuilt file: its name, its `CREATE TABLE`
/// statement (stored verbatim in `sqlite_master`), and its rows already projected
/// to full column-value lists (lead columns + cells, missing cells as NULL). Both
/// `recovered_records` and `recovered_fragments` are built from this single spec,
/// so the b-tree bulk-load and overflow handling are shared.
struct TableSpec {
    name: String,
    create_sql: String,
    rows: Vec<Vec<Value>>,
}

/// Build the bytes of a valid single-table `SQLite` database holding every
/// [`RebuildRow`] as a row of `recovered_records`. See the module docs for the
/// schema and the bulk-load / overflow guarantees.
///
/// This is the zero-fragment path: byte-identical to
/// [`build_recovered_db_with_fragments`] called with `None`.
#[must_use]
pub fn build_recovered_db(rows: &[RebuildRow]) -> Vec<u8> {
    build_recovered_db_with_fragments(rows, None)
}

/// Build the bytes of a valid `SQLite` database holding the full carved records in
/// `recovered_records` and, when `fragments` is `Some`, the Tier-2 partial rows in
/// a **separate** `recovered_fragments` table.
///
/// The two sets stay structurally separate — a fragment is never written into
/// `recovered_records`. `fragments == None` omits the second table entirely and
/// reproduces the single-table [`build_recovered_db`] bytes exactly; `Some(&[])`
/// still creates an empty-but-queryable `recovered_fragments` table with the right
/// schema. Each surviving `(col_index, value)` lands natively at `c{col_index}`
/// (NULL elsewhere); `M = max surviving col_index` sets the fragment table width.
#[must_use]
pub fn build_recovered_db_with_fragments(
    rows: &[RebuildRow],
    fragments: Option<&[FragmentRow]>,
) -> Vec<u8> {
    let mut specs = vec![records_spec(rows)];
    if let Some(frags) = fragments {
        specs.push(fragments_spec(frags));
    }

    // Page 1 is the schema (sqlite_master) leaf; each table's b-tree starts after
    // it. Build the table b-trees first (so we know each root page) while emitting
    // overflow pages after the b-trees.
    build_from_specs(&specs)
}

/// Shared back end: bulk-load each [`TableSpec`]'s b-tree, then write page 1's
/// `sqlite_master` leaf with one schema row per table. Used by both the
/// records/fragments path and the general [`build_recovered_db_tables`] path.
fn build_from_specs(specs: &[TableSpec]) -> Vec<u8> {
    // Page 1 is the schema (sqlite_master) leaf; each table's b-tree starts after
    // it. Build the table b-trees first (so we know each root page) while emitting
    // overflow pages after the b-trees.
    let mut builder = Builder::new();
    let _schema_page = builder.alloc_page(); // page 1
    let placed: Vec<(&str, u32, &str)> = specs
        .iter()
        .map(|spec| {
            let root = builder.build_table(&spec.rows);
            (spec.name.as_str(), root, spec.create_sql.as_str())
        })
        .collect();

    // Page 1: the file header + a sqlite_master leaf row per materialized table.
    let schema_leaf = build_schema_leaf(&placed, builder.page_count());
    builder.set_page(1, schema_leaf);

    builder.finish()
}

/// Quote a string as a SQL identifier: wrap in double quotes and double any
/// embedded double quote (file-format / SQL standard delimited identifiers). So
/// a column named `order` becomes `"order"` and `a"b` becomes `"a""b"`.
fn quote_ident(ident: &str) -> String {
    let mut out = String::with_capacity(ident.len() + 2);
    out.push('"');
    for ch in ident.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// A `CREATE TABLE "name" ("c0", "c1", …)` statement with every identifier
/// quoted, so arbitrary table/column names (keywords, spaces, quotes) are legal.
/// No declared types — recovered cells keep their native storage class
/// regardless, and an untyped column has BLOB affinity (accepts anything).
fn create_table_sql(name: &str, columns: &[String]) -> String {
    let mut sql = String::from("CREATE TABLE ");
    sql.push_str(&quote_ident(name));
    sql.push_str(" (");
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str(&quote_ident(col));
    }
    sql.push(')');
    sql
}

/// Project the full carved records into the `recovered_records` [`TableSpec`].
fn records_spec(rows: &[RebuildRow]) -> TableSpec {
    let max_cells = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let values = rows.iter().map(|r| record_values(r, max_cells)).collect();
    TableSpec {
        name: "recovered_records".to_string(),
        create_sql: create_records_sql(max_cells),
        rows: values,
    }
}

/// Project the carved fragments into the `recovered_fragments` [`TableSpec`]. The
/// table is `M+1` cell columns wide, where `M` is the largest surviving column
/// index across all fragments (0 columns when there are no fragments).
fn fragments_spec(frags: &[FragmentRow]) -> TableSpec {
    let max_col = frags
        .iter()
        .flat_map(|f| f.surviving.iter().map(|(idx, _)| *idx))
        .max();
    // Cell columns c0..=max_col; none when no fragment has any surviving cell.
    let cell_cols = max_col.map_or(0, |m| m + 1);
    let values = frags
        .iter()
        .map(|f| fragment_values(f, cell_cols))
        .collect();
    TableSpec {
        name: "recovered_fragments".to_string(),
        create_sql: create_fragments_sql(cell_cols),
        rows: values,
    }
}

/// The `CREATE TABLE recovered_records (...)` statement stored in `sqlite_master`.
/// The fixed forensic columns precede `c0..c{max_cells-1}` (the recovered cells).
fn create_records_sql(max_cells: usize) -> String {
    let mut sql = String::from(
        "CREATE TABLE recovered_records (\n  _page INTEGER, _offset INTEGER, \
         _rowid INTEGER, _source TEXT, _confidence REAL",
    );
    append_cell_columns(&mut sql, max_cells);
    sql.push(')');
    sql
}

/// The `CREATE TABLE recovered_fragments (...)` statement. Fragments carry NO
/// rowid (clobbered) and NO source label here; the forensic columns are `_page,
/// _offset, _missing, _confidence`, then `c0..c{cell_cols-1}` for surviving cells.
fn create_fragments_sql(cell_cols: usize) -> String {
    let mut sql = String::from(
        "CREATE TABLE recovered_fragments (\n  _page INTEGER, _offset INTEGER, \
         _missing INTEGER, _confidence REAL",
    );
    append_cell_columns(&mut sql, cell_cols);
    sql.push(')');
    sql
}

/// Append `, c0, c1, …, c{n-1}` cell-column declarations to a `CREATE TABLE` body.
fn append_cell_columns(sql: &mut String, n: usize) {
    for i in 0..n {
        use std::fmt::Write as _;
        let _ = write!(sql, ", c{i}");
    }
}

/// Project a [`RebuildRow`] to its full column-value list: the lead columns
/// (`_page, _offset, _rowid, _source, _confidence`) then `max_cells` recovered
/// cells, missing trailing cells materialized as NULL.
fn record_values(row: &RebuildRow, max_cells: usize) -> Vec<Value> {
    let mut values: Vec<Value> = Vec::with_capacity(LEAD_COLS + max_cells);
    values.push(Value::Integer(i64::from(row.page)));
    values.push(Value::Integer(row.offset as i64));
    values.push(match row.rowid {
        Some(id) => Value::Integer(id),
        None => Value::Null,
    });
    values.push(Value::Text(row.source.clone()));
    values.push(Value::Real(f64::from(row.confidence)));
    for i in 0..max_cells {
        values.push(row.cells.get(i).cloned().unwrap_or(Value::Null));
    }
    values
}

/// Project a [`FragmentRow`] to its full column-value list: the lead columns
/// (`_page, _offset, _missing, _confidence`) then `cell_cols` cell columns, each
/// surviving `(idx, value)` placed at `c{idx}` and every other cell column NULL.
fn fragment_values(frag: &FragmentRow, cell_cols: usize) -> Vec<Value> {
    let mut cells = vec![Value::Null; cell_cols];
    for (idx, v) in &frag.surviving {
        // A surviving index is always < cell_cols by construction (cell_cols is
        // max index + 1), so this assignment never falls off the end.
        if let Some(slot) = cells.get_mut(*idx) {
            *slot = v.clone();
        }
    }
    let mut values: Vec<Value> = Vec::with_capacity(FRAG_LEAD_COLS + cell_cols);
    values.push(Value::Integer(i64::from(frag.page)));
    values.push(Value::Integer(frag.offset as i64));
    values.push(Value::Integer(frag.missing as i64));
    values.push(Value::Real(f64::from(frag.confidence)));
    values.extend(cells);
    values
}

/// The number of fixed leading columns every rebuilt `recovered_fragments` row
/// carries before its surviving cells: `_page, _offset, _missing, _confidence`.
const FRAG_LEAD_COLS: usize = 4;

/// A growing database image: a vector of fixed-size pages, 1-based externally.
struct Builder {
    pages: Vec<Vec<u8>>, // pages[i] is page (i+1)
}

impl Builder {
    fn new() -> Self {
        Self { pages: Vec::new() }
    }

    /// Reserve a fresh zeroed page and return its 1-based number.
    fn alloc_page(&mut self) -> u32 {
        self.pages.push(vec![0u8; PAGE_SIZE]);
        self.pages.len() as u32
    }

    /// Overwrite an already-allocated page (1-based) with `content` (zero-padded
    /// / truncated to the page size).
    fn set_page(&mut self, page: u32, mut content: Vec<u8>) {
        content.resize(PAGE_SIZE, 0);
        let idx = page as usize - 1;
        self.pages[idx] = content;
    }

    /// Append a fully-formed page image and return its 1-based number.
    fn push_page(&mut self, mut content: Vec<u8>) -> u32 {
        content.resize(PAGE_SIZE, 0);
        self.pages.push(content);
        self.pages.len() as u32
    }

    fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Build a table b-tree from `rows` (each already a full column-value list),
    /// returning its root page.
    ///
    /// Each row is encoded as a record, assigned a sequential rowid `1..=N`, and
    /// packed into leaf pages; a record larger than a leaf can hold spills onto an
    /// overflow chain. When more than one leaf results, interior pages are built
    /// bottom-up over the leaves.
    fn build_table(&mut self, rows: &[Vec<Value>]) -> u32 {
        // Encode every row's payload up front; assign rowids 1..=N.
        let payloads: Vec<(i64, Vec<u8>)> = rows
            .iter()
            .enumerate()
            .map(|(i, values)| (i as i64 + 1, encode_record(values)))
            .collect();

        // Pack records into leaf pages (greedy; bulk load, no splitting).
        let leaves = self.pack_leaves(&payloads);
        debug_assert!(!leaves.is_empty());

        // Single leaf → it is the root. Otherwise build interior pages over it.
        if leaves.len() == 1 {
            leaves[0].0
        } else {
            self.build_interiors(&leaves)
        }
    }

    /// Greedily pack `(rowid, payload)` records into table-leaf pages, spilling
    /// oversize payloads to overflow chains. Returns each leaf's
    /// `(page_no, max_rowid_on_leaf)`, in order. Always returns at least one leaf
    /// (an empty table is a single empty leaf).
    fn pack_leaves(&mut self, payloads: &[(i64, Vec<u8>)]) -> Vec<(u32, i64)> {
        let mut leaves: Vec<(u32, i64)> = Vec::new();
        // Accumulate cells for the current leaf, then flush.
        let mut cur: Vec<LeafCell> = Vec::new();
        let mut cur_max_rowid = 0i64;

        for (rowid, payload) in payloads {
            let cell = self.make_leaf_cell(*rowid, payload);
            // Leaf budget: 8-byte b-tree header + 2-byte pointer per cell + cell
            // bytes must fit in the page. Each cell needs 2 (pointer) + on-page
            // footprint.
            let used: usize = 8 + cur.iter().map(|c| 2 + c.on_page.len()).sum::<usize>();
            let need = 2 + cell.on_page.len();
            if !cur.is_empty() && used + need > PAGE_SIZE {
                let page = self.flush_leaf(&cur);
                leaves.push((page, cur_max_rowid));
                cur.clear();
            }
            cur_max_rowid = *rowid;
            cur.push(cell);
        }
        // Flush the final (or only/empty) leaf.
        let page = self.flush_leaf(&cur);
        leaves.push((page, cur_max_rowid));
        leaves
    }

    /// Turn a record payload into a leaf cell, allocating overflow pages for the
    /// part that does not fit locally. `on_page` is the exact byte sequence that
    /// goes into the leaf page's cell-content area.
    fn make_leaf_cell(&mut self, rowid: i64, payload: &[u8]) -> LeafCell {
        let total = payload.len();
        let local = local_payload_len(total, USABLE);
        let mut on_page = Vec::new();
        on_page.extend_from_slice(&enc_varint_into(total));
        on_page.extend_from_slice(&enc_varint_into(rowid_as_usize(rowid)));
        if local >= total {
            on_page.extend_from_slice(payload);
        } else {
            on_page.extend_from_slice(&payload[..local]);
            let first_overflow = self.write_overflow_chain(&payload[local..]);
            on_page.extend_from_slice(&first_overflow.to_be_bytes());
        }
        LeafCell { on_page }
    }

    /// Write `rest` across a chain of overflow pages and return the first page
    /// number. Each page is `[next u32 BE][up to OVERFLOW_PAYLOAD content bytes]`;
    /// the last page's next pointer is 0.
    fn write_overflow_chain(&mut self, rest: &[u8]) -> u32 {
        // Pre-allocate the pages so each can point at its successor.
        let n_pages = rest.len().div_ceil(OVERFLOW_PAYLOAD);
        let first = self.page_count() + 1;
        let mut chunks = rest.chunks(OVERFLOW_PAYLOAD);
        for i in 0..n_pages {
            let chunk = chunks.next().unwrap_or(&[]);
            let next = if i + 1 < n_pages {
                first + i as u32 + 1
            } else {
                0
            };
            let mut page = Vec::with_capacity(PAGE_SIZE);
            page.extend_from_slice(&next.to_be_bytes());
            page.extend_from_slice(chunk);
            self.push_page(page);
        }
        first
    }

    /// Assemble the leaf-page image for `cells` and append it, returning its page.
    /// Cell content grows down from the end of the page; the pointer array grows
    /// up from just after the 8-byte b-tree header.
    fn flush_leaf(&mut self, cells: &[LeafCell]) -> u32 {
        let mut page = vec![0u8; PAGE_SIZE];
        page[0] = 0x0d; // table-leaf b-tree page
        write_u16(&mut page, 1, 0); // first freeblock = 0
        write_u16(&mut page, 3, cells.len() as u16); // cell count
                                                     // Place cells from the end of the page downward.
        let mut content_start = PAGE_SIZE;
        let header_end = 8 + cells.len() * 2;
        for (i, cell) in cells.iter().enumerate() {
            content_start -= cell.on_page.len();
            page[content_start..content_start + cell.on_page.len()].copy_from_slice(&cell.on_page);
            write_u16(&mut page, 8 + i * 2, content_start as u16);
        }
        // Cell content area start (0 means 65536, but our pages are 4096).
        write_u16(&mut page, 5, content_start as u16);
        debug_assert!(header_end <= content_start);
        page[7] = 0; // fragmented free bytes
        self.push_page(page)
    }

    /// Build interior table pages over `leaves` bottom-up and return the root.
    ///
    /// Each interior cell is `[left-child u32 BE][rowid-key varint]` where the key
    /// is the largest rowid in the left subtree; the rightmost child pointer lives
    /// in the page header (offset 8). Levels are reduced until one page remains.
    fn build_interiors(&mut self, leaves: &[(u32, i64)]) -> u32 {
        let mut level: Vec<(u32, i64)> = leaves.to_vec();
        while level.len() > 1 {
            let mut next_level: Vec<(u32, i64)> = Vec::new();
            // Greedily group children into interior pages.
            let mut group: Vec<(u32, i64)> = Vec::new();
            for child in &level {
                // Interior cell footprint: 2 (pointer) + 4 (left child) + varint.
                let used: usize = 12
                    + group
                        .iter()
                        .map(|c| 2 + 4 + enc_varint_into(rowid_as_usize(c.1)).len())
                        .sum::<usize>();
                let need = 2 + 4 + enc_varint_into(rowid_as_usize(child.1)).len();
                if !group.is_empty() && used + need > PAGE_SIZE {
                    next_level.push(self.flush_interior(&group));
                    group.clear();
                }
                group.push(*child);
            }
            if !group.is_empty() {
                next_level.push(self.flush_interior(&group));
            }
            level = next_level;
        }
        level[0].0
    }

    /// Assemble an interior table page over `children` (each `(page, max_rowid)`),
    /// the last child becoming the rightmost pointer. Returns `(page, max_rowid)`
    /// so the level above can key on this subtree's largest rowid.
    fn flush_interior(&mut self, children: &[(u32, i64)]) -> (u32, i64) {
        let mut page = vec![0u8; PAGE_SIZE];
        page[0] = 0x05; // table-interior b-tree page
        write_u16(&mut page, 1, 0); // first freeblock = 0
                                    // The last child is the rightmost pointer; the rest are keyed cells.
        let Some((rightmost, keyed)) = children.split_last() else {
            // cov:unreachable: build_interiors only flushes non-empty groups.
            return (self.push_page(page), 0);
        };
        write_u32(&mut page, 8, rightmost.0); // rightmost child pointer
        write_u16(&mut page, 3, keyed.len() as u16); // cell count

        let mut content_start = PAGE_SIZE;
        for (i, (child, key)) in keyed.iter().enumerate() {
            let mut cell = Vec::with_capacity(4 + 9);
            cell.extend_from_slice(&child.to_be_bytes());
            cell.extend_from_slice(&enc_varint_into(rowid_as_usize(*key)));
            content_start -= cell.len();
            page[content_start..content_start + cell.len()].copy_from_slice(&cell);
            write_u16(&mut page, 12 + i * 2, content_start as u16);
        }
        write_u16(&mut page, 5, content_start as u16);
        page[7] = 0;
        let page_no = self.push_page(page);
        (page_no, rightmost.1)
    }

    /// Serialize all pages into the final database image, stamping the in-header
    /// page count.
    fn finish(mut self) -> Vec<u8> {
        let count = self.page_count();
        // The in-header db size (offset 28) is only honored by the reader when it
        // equals the change counter (offset 24); the schema leaf writes both.
        let _ = count;
        let mut out = Vec::with_capacity(self.pages.len() * PAGE_SIZE);
        for page in self.pages.drain(..) {
            out.extend_from_slice(&page);
        }
        out
    }
}

/// A table-leaf cell ready to place on a page: the exact on-page byte sequence
/// (payload-len varint, rowid varint, local payload, and a 4-byte overflow
/// pointer when the record spilled).
struct LeafCell {
    on_page: Vec<u8>,
}

/// Encode a sequence of values as a `SQLite` record (file-format §2): a header of
/// `[header_len varint][serial-type varints]` followed by the value bodies.
fn encode_record(values: &[Value]) -> Vec<u8> {
    let mut serials: Vec<i64> = Vec::with_capacity(values.len());
    let mut body: Vec<u8> = Vec::new();
    for v in values {
        let (serial, bytes) = encode_value(v);
        serials.push(serial);
        body.extend_from_slice(&bytes);
    }
    // header_len includes its own varint, so it is self-referential: compute the
    // serial-type bytes first, then the smallest header_len varint that, prefixed,
    // makes header_len equal the total header size.
    let mut serial_bytes: Vec<u8> = Vec::new();
    for &s in &serials {
        serial_bytes.extend_from_slice(&enc_varint_into(s as usize));
    }
    let header_len = resolve_header_len(serial_bytes.len());
    let mut out = Vec::with_capacity(header_len + body.len());
    out.extend_from_slice(&enc_varint_into(header_len));
    out.extend_from_slice(&serial_bytes);
    out.extend_from_slice(&body);
    out
}

/// Resolve the self-referential record `header_len` (file-format §2): the value
/// stored equals the byte length of the whole header, *including* its own varint.
/// `serial_len` is the byte length of the serial-type array. We grow the
/// candidate until its own varint width stabilizes (at most a 1-byte step).
fn resolve_header_len(serial_len: usize) -> usize {
    let mut header_len = serial_len + 1;
    loop {
        let with_varint = serial_len + enc_varint_into(header_len).len();
        if with_varint == header_len {
            return header_len;
        }
        header_len = with_varint;
    }
}

/// Encode one value as `(serial_type, body_bytes)` (file-format §2.1). Integers
/// use the narrowest serial that holds the value; the constant 0/1 serials (8/9)
/// carry no body. TEXT is stored as UTF-8 (the database encoding the header
/// declares); BLOB is stored byte-for-byte (no transformation — the lossless
/// path the feature exists for).
fn encode_value(v: &Value) -> (i64, Vec<u8>) {
    match v {
        Value::Null => (0, Vec::new()),
        Value::Integer(i) => encode_int(*i),
        Value::Real(r) => (7, r.to_bits().to_be_bytes().to_vec()),
        Value::Text(t) => (13 + 2 * t.len() as i64, t.as_bytes().to_vec()),
        Value::Blob(b) => (12 + 2 * b.len() as i64, b.clone()),
    }
}

/// Encode an integer with the narrowest `SQLite` serial type that represents it
/// (file-format §2.1): 0 and 1 use the bodyless serials 8 and 9; otherwise the
/// minimal big-endian width among 1/2/3/4/6/8 bytes.
fn encode_int(i: i64) -> (i64, Vec<u8>) {
    match i {
        0 => (8, Vec::new()),
        1 => (9, Vec::new()),
        _ if (i64::from(i8::MIN)..=i64::from(i8::MAX)).contains(&i) => (1, vec![i as u8]),
        _ if (i64::from(i16::MIN)..=i64::from(i16::MAX)).contains(&i) => {
            (2, (i as i16).to_be_bytes().to_vec())
        }
        _ if (-(1 << 23)..(1 << 23)).contains(&i) => (3, i.to_be_bytes()[5..].to_vec()),
        _ if (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&i) => {
            (4, (i as i32).to_be_bytes().to_vec())
        }
        _ if (-(1 << 47)..(1 << 47)).contains(&i) => (5, i.to_be_bytes()[2..].to_vec()),
        _ => (6, i.to_be_bytes().to_vec()),
    }
}

/// Map a rowid to the unsigned value the varint encoder expects. Rowids are
/// non-negative in this writer (sequential `1..=N`), so this is the identity on
/// the in-range domain; a negative rowid (never produced here) would saturate to
/// 0 rather than panic.
fn rowid_as_usize(rowid: i64) -> usize {
    usize::try_from(rowid).unwrap_or(0)
}

/// Build page 1: the 100-byte file header followed by the `sqlite_master`
/// table-leaf b-tree page carrying one schema row per materialized table.
///
/// Each entry is `(name, rootpage, create_sql)`. With a single table this is
/// byte-identical to the historical single-cell layout (one cell, rowid 1, content
/// packed from the page end). Schema records are small (well under a page) and
/// never spill.
fn build_schema_leaf(tables: &[(&str, u32, &str)], page_count: u32) -> Vec<u8> {
    let mut page = vec![0u8; PAGE_SIZE];
    write_file_header(&mut page, page_count);

    // The b-tree header for page 1 lives at offset 100 (after the file header).
    let h = 100;
    page[h] = 0x0d; // table-leaf
    write_u16(&mut page, h + 1, 0); // first freeblock
    write_u16(&mut page, h + 3, tables.len() as u16); // one cell per schema row
    page[h + 7] = 0; // fragmented free bytes

    // Cells grow down from the page end, in order; the pointer array grows up from
    // just after the 8-byte b-tree header at offset 100. sqlite_master rowids are
    // 1..=N (the schema objects in declaration order).
    let mut content_start = PAGE_SIZE;
    for (i, (name, root, sql)) in tables.iter().enumerate() {
        let cell = schema_cell(name, *root, sql, i as i64 + 1);
        content_start -= cell.len();
        page[content_start..content_start + cell.len()].copy_from_slice(&cell);
        write_u16(&mut page, h + 8 + i * 2, content_start as u16); // cell pointer[i]
    }
    debug_assert!(h + 8 + tables.len() * 2 <= content_start);
    write_u16(&mut page, h + 5, content_start as u16); // cell content area start
    page
}

/// One `sqlite_master` leaf cell for a table: the `(type, name, tbl_name,
/// rootpage, sql)` record at the given rowid, prefixed by its payload-len and
/// rowid varints.
fn schema_cell(name: &str, root: u32, create_sql: &str, rowid: i64) -> Vec<u8> {
    let schema_record = encode_record(&[
        Value::Text("table".into()),
        Value::Text(name.into()),
        Value::Text(name.into()),
        Value::Integer(i64::from(root)),
        Value::Text(create_sql.into()),
    ]);
    let mut cell = Vec::new();
    cell.extend_from_slice(&enc_varint_into(schema_record.len()));
    cell.extend_from_slice(&enc_varint_into(rowid_as_usize(rowid)));
    cell.extend_from_slice(&schema_record);
    cell
}

/// Write the 100-byte `SQLite` file header (file-format §1.3) into `page`.
fn write_file_header(page: &mut [u8], page_count: u32) {
    // Magic.
    page[..16].copy_from_slice(b"SQLite format 3\0");
    // Page size (offset 16, BE u16; our 4096 fits directly).
    write_u16(page, 16, PAGE_SIZE as u16);
    page[18] = 1; // file format write version = legacy (rollback journal)
    page[19] = 1; // file format read version = legacy
    page[20] = 0; // reserved space per page
    page[21] = 64; // max embedded payload fraction (spec-fixed)
    page[22] = 32; // min embedded payload fraction (spec-fixed)
    page[23] = 32; // leaf payload fraction (spec-fixed)
    write_u32(page, 24, 1); // file change counter
    write_u32(page, 28, page_count); // in-header database size, in pages
    write_u32(page, 32, 0); // first freelist trunk page (none)
    write_u32(page, 36, 0); // freelist page count
    write_u32(page, 40, 1); // schema cookie
    write_u32(page, 44, 4); // schema format number (4 = current)
    write_u32(page, 48, 0); // default page cache size
    write_u32(page, 52, 0); // largest root b-tree page (0 = not auto-vacuum)
    write_u32(page, 56, 1); // text encoding = UTF-8
    write_u32(page, 60, 0); // user version
    write_u32(page, 64, 0); // incremental-vacuum mode (off)
    write_u32(page, 68, 0); // application id
                            // bytes 72..92 reserved for expansion (zero)
    write_u32(page, 92, 1); // version-valid-for number (matches change counter)
    write_u32(page, 96, 3_045_000); // SQLITE_VERSION_NUMBER (cosmetic; 3.45.0)
}

/// Write a big-endian `u16` at `off`.
fn write_u16(buf: &mut [u8], off: usize, val: u16) {
    buf[off..off + 2].copy_from_slice(&val.to_be_bytes());
}

/// Write a big-endian `u32` at `off`.
fn write_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}
