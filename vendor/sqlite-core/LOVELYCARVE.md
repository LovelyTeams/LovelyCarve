# LovelyCarve integration

Reader crate `sqlite-core` 0.12.0 from
<https://github.com/SecurityRonin/sqlite-forensic> (workspace member `core`),
commit `df511309e086825fb194ea8a721e94bbd0033fe0`. Apache-2.0; see LICENSE.
Workspace manifest fields are expanded for a standalone crate. The workspace's
`forensic` analyzer crate (reports, timelines, CLI), fuzz targets and hosted
CI are not included.

The application uses `core/preview/sqlite/sqlite_recovery.rs` for the SQLite browser's
「已删除记录」 view. The crate parses the bytes itself, so freelist pages,
in-page free space, freeblock remnants, overflow chains, uncheckpointed WAL
frames and a rollback journal are visible to it; the browser's table view
still goes through SQLite.

Local changes:

- `src/recovery.rs` is new: the carving and attribution layer ported from the
  `forensic` crate at the same commit (`carve_all_deleted_records`,
  `attribute_records`, `recover_dropped_schemas`, `carve_rollback_journal` and
  their types). On the corpus below the port first reproduced upstream exactly
  (689 of 689 records); it then took two fixes:
  - A freeblock-reconstructed record has lost its rowid, and an
    `INTEGER PRIMARY KEY` alias column then reads NULL where the live row reads
    the rowid. Upstream compared whole value tuples against the live rows, so
    stale copies of live rows were reported as deleted (4 in the corpus). Such
    a record is now matched against live rows on its remaining columns.
  - A carved record is a prior version only when the table that owns its page
    has a live row with that rowid. Upstream accepted a rowid match in any
    table, labelling 119 rows of a dropped table as prior versions.
- `src/sqlcipher.rs` detects `cipher_compatibility` 2 and 1 besides 4 and 3
  (SHA1, 1 KiB pages, 4,000 iterations; version 1 has no page HMAC, so a
  coherent decrypted page-1 header is the key check), and `decrypt_with`
  takes explicit `CipherParams` for non-default databases such as WeChat's.
- The `forensicnomicon` dependency is replaced by local header-offset
  constants, and the `state-history` adapter (`to_temporal_cohort`) is removed
  with its test.
- Clippy cleanups for current Rust (`is_multiple_of`, `as_chunks`,
  `is_none_or`); `rust-version` is raised to 1.88.

Corpus: `tests/data/lovelycarve/make_corpus.py` writes the scenarios with
SQLite itself and records the deleted rows as ground truth;
`tests/lovelycarve_recovery.rs` covers the two fixes. Recall per scenario
after the fixes: persist journal 100%, drop_table 92%, basic 75%,
autovacuum 74%, updates 67%, wal 65%, utf16 44%, overflow 27%,
secure_delete 0% (the bytes are zeroed). No live row is reported in any
scenario.

Tests: `just test-sqlite-core` runs the crate tests; the `sqlite3` oracle
tests skip unless a `sqlite3` binary is on `PATH` or in `SQLITE3_BIN`.
`just test sqlite_recovery` covers the application adapter against databases
written by SQLite and SQLCipher.
