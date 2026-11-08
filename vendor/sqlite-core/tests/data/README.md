# Test Data — `tests/data/`

Committed SQLite fixtures shared by both workspace members. `core/tests/*.rs` and
`forensic/tests/*.rs` reach them with the symmetric relative path
`include_bytes!("../../tests/data/<file>")`.

For the per-repo index (classification + oracle methodology) see
[`../../docs/corpus-catalog.md`](../../docs/corpus-catalog.md); for the
deleted-record carving validation see
[`../../docs/validation.md`](../../docs/validation.md). The gitignored
independent-oracle corpus is documented separately in
[`../../tests-oracle-corpus/README.md`](../../tests-oracle-corpus/README.md).

The **loose top-level** files here are **SYNTHETIC** — built locally with the
real `sqlite3` engine (`SQLite 3.45.3 2024-04-15`, REAL engine / synthetic data).
The generator command IS the provenance; there is no download URL.

The **committed real-world third-party corpora** live in subdirectories, each with
its own provenance README (source, NIST/author hashes, licence, ground truth):

- [`nemetz/`](nemetz/README.md) — Nemetz SQLite Forensic Corpus v2.0 (141 DBs, CC0)
  — deleted-record ground truth + anti-forensic robustness.
- [`cfreds/`](cfreds/README.md) — NIST CFReDS / CFTT SQLite test sets (SFT-01 encodings,
  SFT-03 deleted/modified records) — U.S. Government public domain, MD5-verified
  against NIST-published hashes.
- [`sharifctf/`](sharifctf/README.md) — a real damaged-header CTF database (robustness).
- [`journal/`](journal/README.md) — real sqlite3-engine rollback-`-journal`
  scenarios (a hot DML journal and a committed-DDL PERSIST journal) for the
  `audit_journal` anomaly arms — minted with the public-domain SQLite engine.
- [`paper_fp/`](paper_fp/README.md) — REAL-engine REPLICATIONS of the three
  false-positive scenarios from the 2025 SQLite-recovery survey (Lee, Park, Lee &
  Choi, FSI:DI 55, art. 302031): B-tree rebalancing, overwritten same-schema
  table, and WAL+`secure_delete`. CC0; consumed by
  `forensic/tests/paper_fp_scenarios.rs` and `docs/competitive-landscape.md`.

#### places.db

- **Source:** SYNTHETIC — system `sqlite3` CLI.
- **Identity:** single-table `moz_places` DB exercising every SQLite storage class
  (Integer/Real/Text/Blob/Null) and the `INTEGER PRIMARY KEY` rowid-alias rule.
  Pre-existing core-reader spike fixture.
- **Generator:**

  ```sh
  sqlite3 tests/data/places.db <<'SQL'
  CREATE TABLE moz_places (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL,
    title TEXT,
    visit_count INTEGER DEFAULT 0,
    last_visit_date INTEGER,
    frecency REAL
  );
  INSERT INTO moz_places (url, title, visit_count, last_visit_date, frecency) VALUES
   ('https://www.rust-lang.org/', 'Rust Programming Language', 5, 1700000000000000, 2000.5),
   ('https://github.com/', 'GitHub', 12, 1700000100000000, 5500.0),
   ('https://news.ycombinator.com/', 'Hacker News', 3, NULL, -1.0),
   ('https://en.wikipedia.org/wiki/SQLite', 'SQLite - Wikipedia', 1, 1700000200000000, NULL),
   ('https://example.com/', NULL, 0, 1700000300000000, 0.0);
  SQL
  ```

- **md5:** `f07a69d05358f227e2120080370bbb6b` — 8192 bytes (2 pages, 4096-byte page).
- **Notable contents:** `frecency` as Real; one NULL `title`, one NULL
  `last_visit_date`, one negative `frecency`. `moz_places` root page = 2. Used by
  `core/tests/real_db.rs`, `core/tests/freelist.rs`, `forensic/tests/audit_realdb.rs`.

#### overflow.db

- **Source:** SYNTHETIC — Python `sqlite3` module.
- **Identity:** one `notes` row whose ~12 KB TEXT body spills onto an
  overflow-page chain, plus one small inline row. Used by `core/tests/overflow.rs`.
- **Generator:**

  ```sh
  python3 - <<'PY'
  import sqlite3
  con = sqlite3.connect('overflow.db')
  con.executescript("PRAGMA page_size=4096; PRAGMA auto_vacuum=NONE;")
  con.execute("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT)")
  big = "OVERFLOW_PAYLOAD_" + ("ABCDEFGHIJ" * 1200)  # 12017 chars
  con.execute("INSERT INTO notes VALUES (1, ?)", (big,))
  con.execute("INSERT INTO notes VALUES (2, 'small row')")
  con.commit(); con.close()
  PY
  ```

- **md5:** `1c17320320a173fb5968c598f9df7373` — 16384 bytes (4 pages).
- **Notable contents:** `notes` root page = 2; row id=1 body length = 12017
  (overflow chain); row id=2 inline.

#### deleted_places.db

- **Source:** SYNTHETIC — Python `sqlite3` module.
- **Identity:** `moz_places` with 400 rows inserted, ids 201..=400 `DELETE`d
  **without VACUUM** under `secure_delete=OFF`, freeing whole leaf pages onto the
  freelist whose old cell content survives. The deleted-record carving fixture
  (`forensic/tests/carve.rs`, `forensic/tests/oracle_differential.rs`,
  `forensic/tests/audit_realdb.rs`) and the freelist fixture
  (`core/tests/freelist.rs`).
- **Generator:**

  ```sh
  python3 - <<'PY'
  import sqlite3
  con = sqlite3.connect('deleted_places.db')
  con.executescript("""
  PRAGMA page_size=4096; PRAGMA auto_vacuum=NONE; PRAGMA secure_delete=OFF;
  CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT,
    visit_count INTEGER, last_visit_date INTEGER, frecency REAL);
  WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 400)
  INSERT INTO moz_places SELECT n,
    'https://site-'||n||'.example.com/path/page',
    'Title for record number '||n||' SECRETMARKER',
    n%100, 1700000000000000+n, n*1.5 FROM seq;
  DELETE FROM moz_places WHERE id > 200;
  """)
  con.commit(); con.close()
  PY
  ```

- **md5:** `16682d7df99b1e8a89287a508d95eb47` — 53248 bytes.
- **Notable contents:** live rows = 200 (ids 1..=200), deleted ids 201..=400;
  `PRAGMA freelist_count` = 5, `PRAGMA page_count` = 13; `moz_places` root = 2.
  Freelist = trunk page 9 + leaf pages 10–13. Note: `secure_delete` defaults to
  **ON** on this build — without the explicit `PRAGMA secure_delete=OFF` the
  deleted content is wiped and nothing is carvable. (Many real browser DBs run
  with `secure_delete` off, so this is realistic.)

#### wal_places.db + wal_places.db-wal

- **Source:** SYNTHETIC — Python `sqlite3` module, snapshotted mid-transaction.
- **Identity:** a main DB + persistent `-wal` sidecar captured
  **mid-transaction** — a held reader connection blocks the checkpoint so the WAL
  survives on disk with one committed COMMIT frame (page 2) the main file does not
  yet reflect. Used by `core/tests/wal.rs` and the WAL branch of
  `forensic/tests/audit_realdb.rs`.
- **Generator:**

  ```sh
  python3 - <<'PY'
  import sqlite3, shutil
  con = sqlite3.connect('wal.db')
  con.executescript("""
  PRAGMA page_size=4096; PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
  CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT,
    visit_count INTEGER, last_visit_date INTEGER, frecency REAL);
  INSERT INTO moz_places VALUES (1,'https://www.rust-lang.org/','Rust',5,1700000000000000,2000.5);
  INSERT INTO moz_places VALUES (2,'https://github.com/','GitHub',12,1700000100000000,5500.0);
  """)
  con.commit()
  con.execute("PRAGMA wal_checkpoint(TRUNCATE)"); con.commit()  # baseline -> main file
  reader = sqlite3.connect('wal.db')                            # hold a read txn
  reader.execute("BEGIN"); reader.execute("SELECT count(*) FROM moz_places").fetchone()
  con.execute("PRAGMA wal_autocheckpoint=0")
  con.execute("UPDATE moz_places SET title='Rust (EDITED IN WAL)', visit_count=777 WHERE id=1")
  con.execute("INSERT INTO moz_places VALUES (3,'https://wal-only-row.example/','WAL-ONLY ROW',1,1700000200000000,100.0)")
  con.commit()
  shutil.copy('wal.db','wal_places.db')          # snapshot while WAL is live
  shutil.copy('wal.db-wal','wal_places.db-wal')
  PY
  ```

- **md5:** `wal_places.db` = `bad96eb068359bcb142533696b6515fc` (8192 bytes);
  `wal_places.db-wal` = `84b08a77d90914c917d92e60a6c8eeab` (4152 bytes).
- **Notable contents:** main-only view = id=1 title `Rust`, visit_count 5, 2 rows;
  WAL-applied view = id=1 title `Rust (EDITED IN WAL)`, visit_count 777, plus id=3
  `WAL-ONLY ROW`, 3 rows. WAL = 1 COMMIT frame for page 2.

#### wal_carve.db + wal_carve.db-wal

- **Source:** SYNTHETIC — Python `sqlite3` module, snapshotted mid-transaction.
- **Identity:** a main DB + persistent `-wal` sidecar where the genuinely-different
  deleted rows live **only in the uncheckpointed WAL frames**, never on the main
  file's pages. A `wal_checkpoint(TRUNCATE)` flushes a clean baseline (rows 1..=50)
  to the main file and empties the WAL; then, with a held reader blocking any
  further checkpoint, rows 101..=150 are inserted (COMMIT) and 121..=140 deleted
  (COMMIT) with **no checkpoint**. Used by the WAL-frame carving tests in
  `core/tests/wal.rs` and `forensic/tests/carve_all.rs` (#60). Cross-referenced in
  `docs/corpus-catalog.md` §J.
- **Generator:**

  ```sh
  python3 - <<'PY'
  import sqlite3, shutil, os
  for f in ('walcarve.db','walcarve.db-wal','walcarve.db-shm'):
      if os.path.exists(f): os.remove(f)
  con = sqlite3.connect('walcarve.db')
  con.execute("PRAGMA page_size=4096")
  con.execute("PRAGMA journal_mode=WAL")
  con.execute("PRAGMA wal_autocheckpoint=0")
  con.execute("CREATE TABLE msg(id INTEGER PRIMARY KEY, sender TEXT, body TEXT)")
  for i in range(1, 51):
      con.execute("INSERT INTO msg VALUES (?,?,?)", (i, f"alice{i}", f"baseline message {i}"))
  con.commit()
  con.execute("PRAGMA wal_checkpoint(TRUNCATE)"); con.commit()   # baseline -> main file, WAL emptied
  reader = sqlite3.connect('walcarve.db')                        # hold a read txn (blocks checkpoint)
  reader.execute("BEGIN"); reader.execute("SELECT count(*) FROM msg").fetchone()
  con.execute("PRAGMA wal_autocheckpoint=0")
  for i in range(101, 151):
      con.execute("INSERT INTO msg VALUES (?,?,?)", (i, f"bob{i}", f"secret WAL body {i}"))
  con.commit()                                                   # INSERT commit -> WAL frame 0
  con.execute("DELETE FROM msg WHERE id BETWEEN 121 AND 140"); con.commit()  # DELETE commit -> WAL frame 1
  shutil.copy('walcarve.db','wal_carve.db')          # snapshot while WAL is live
  shutil.copy('walcarve.db-wal','wal_carve.db-wal')
  reader.close(); con.close()
  PY
  ```

- **md5:** `wal_carve.db` = `6747389de0fefcc4c23543353a31325a` (8192 bytes);
  `wal_carve.db-wal` = `598e80ad38536f4b7a6cb51ddaedc767` (8272 bytes).
- **Notable contents:** on-disk-only carve recovers 0 of the WAL-resident deleted
  rows; WAL-frame carve recovers 20/20 rows 121..=140 (tagged
  `RecoverySource::WalFrame` with `(salt1, salt2, frame_index)` provenance) and
  re-surfaces 0 surviving (live) rows. WAL = 2 COMMIT frames for page 2.

#### updated_messages.db

- **Source:** SYNTHETIC — Python `sqlite3` module.
- **Identity:** a `messages` table where row 7's `body` is `UPDATE`d twice (grow
  then shrink) under `secure_delete=OFF`, so the pre-edit version survives in freed
  slack with the **same rowid as the live row but different values** — the
  version-aware (prior-version) carving fixture. Drives
  `forensic/tests/prior_version.rs` and the prior-version leg of
  `forensic/tests/oracle_differential.rs`.
- **Generator:**

  ```sh
  python3 - <<'PY'
  import sqlite3
  con = sqlite3.connect('updated_messages.db')
  con.executescript("""
  PRAGMA page_size=4096; PRAGMA auto_vacuum=NONE; PRAGMA secure_delete=OFF;
  CREATE TABLE messages(id INTEGER PRIMARY KEY, sender TEXT, body TEXT, amount INTEGER);
  """)
  con.executemany("INSERT INTO messages VALUES(?,?,?,?)",
                  [(n, f"user{n}", f"ORIGINAL message body number {n} ZZZ", 707) for n in range(1, 51)])
  con.commit()
  con.execute("UPDATE messages SET body=? WHERE id=7",
              ("PRIORVERSION secret message body that was later edited " + ("Q" * 120),))
  con.execute("UPDATE messages SET body='EDITED final body' WHERE id=7")
  con.commit(); con.close()
  PY
  ```

- **md5:** `e1edbb56bf37efa6a7c1e738040f1360` (8192 bytes).
- **Notable contents:** 50 live rows (ids 1..=50); live row 7 body =
  `EDITED final body`. The recoverable prior version is rowid 7, body
  `PRIORVERSION secret …`, amount 707 — genuine deleted content whose rowid is still
  live with different values. The full original body survives nowhere (overwritten);
  only the intermediate `PRIORVERSION` version is cleanly carvable.

#### freeblock_2byte_rowid.db

- **Source:** SYNTHETIC — `sqlite3` CLI (Tier-2; ground truth derivable from the
  construction).
- **Identity:** a 4-column table whose rows all have **2-byte rowid varints**
  (ids 1000..=1259), with three well-separated rows deleted so each freed cell is
  its own non-coalesced freeblock. Pins the freeblock-clobbered **2-byte-rowid**
  recovery path (`forensic/tests/freeblock_highrowid.rs`): the 4-byte freeblock
  clobber destroys no serial type, so the whole serial array survives and the
  exact-tile reconstruction recovers the row. The independent sqlite-unhide 09.db
  (env-gated) is the real-corpus twin.
- **Generator:**

  ```sh
  sqlite3 freeblock_2byte_rowid.db <<'SQL'
  PRAGMA page_size=4096; PRAGMA secure_delete=0;
  CREATE TABLE m(id INTEGER PRIMARY KEY, sid TEXT, line TEXT, n INT);
  WITH RECURSIVE c(i) AS (SELECT 1000 UNION ALL SELECT i+1 FROM c WHERE i<1260)
  INSERT INTO m SELECT i, 's'||i, 'line text for record '||i, i FROM c;
  DELETE FROM m WHERE id IN (1050, 1130, 1210);
  SQL
  ```

- **md5:** `e32a55e60a40e3072917d4d5cd3494f5` — 20480 bytes.
- **Notable contents:** 258 live rows; deleted ids 1050/1130/1210, each
  recovered as `line text for record <id>` with no misaligned phantom;
  `PRAGMA freelist_count` = 0 (in-page freeblocks only).

#### freeblock_coalesced.db

- **Source:** SYNTHETIC — `sqlite3` CLI (Tier-2; ground truth derivable from the
  construction). Same shape as `freeblock_2byte_rowid.db`.
- **Identity:** **adjacent** high-rowid rows (`DELETE id BETWEEN 1100 AND 1104`)
  whose freed cells SQLite coalesces into one multi-cell freeblock. Pins the
  **span-level exact-tiling** recovery (`forensic/tests/freeblock_highrowid.rs`):
  the coalesced cells are recovered only when they tile the freeblock exactly, so
  a misaligned run yields nothing rather than a phantom.
- **md5:** `e064fd01f8040c49dc5cf8913532b36f` — 20480 bytes.
- **Notable contents:** 256 live rows; deleted ids 1100..=1104 recovered (≥4 of
  5), no misaligned phantom; `PRAGMA freelist_count` = 0.

#### exclusion_invariant/cross_table_rowid.db

- **Source:** SYNTHETIC — Python `sqlite3` module (Tier-2; ground truth derivable
  from the construction). Generator + provenance: `exclusion_invariant/gen.py`.
- **Identity:** two tables that SHARE rowids, to exercise the exclusion-invariant
  hole where live-row identity is keyed by a global rowid with no table dimension
  (`docs/improvement-roadmap.md` §1.1). Table `t` (created first) is the paper 0F
  B-tree-rebalancing scenario — 80 rows of ~420-byte id-tagged TEXT, `DELETE`
  ids 1..50 — which frees pages still holding live rows 51..80. Table `z` (created
  second, so it wins `Database::live_rows()`'s rowid-keyed collapse) holds the same
  rowids 51..80 with different values. Pins
  `forensic/tests/exclusion_invariant_cross_table.rs`: with `z` hijacking the
  collapse, the carver re-surfaces exactly the 13 live `t` rows (ids 51..63) that a
  rebalance shuffled onto freed pages — the breach the test forbids.
- **md5:** `f8966f346bf9c6c95aa7791b86e1b52b` — 49152 bytes.
- **Notable contents:** `t` live ids 51..80 (`v` = `ROW-<id>-XXXX…`), `t` deleted
  ids 1..50; `z` live ids 51..80 (`v` = `ZZZ-<id>-QQQ…`); `secure_delete=OFF`,
  `auto_vacuum=NONE`. Ground truth: NO carved record's values may equal any
  currently-live row of either table.

#### dedup_cross_table/cross_table_identity.db

- **Source:** SYNTHETIC — Python `sqlite3` module (Tier-2; ground truth derivable
  from the construction). Generator: `dedup_cross_table/gen.py`.
- **Identity:** two live tables `a` and `b` each holding `(1,'DUP-1')` and
  `(2,'DUP-2')` (identical rowid+values across tables) plus a distinct keeper row,
  then both `DELETE` ids 1 and 2. Pins
  `forensic/tests/dedup_cross_table.rs` (roadmap §1.2): the carver recovers each
  deleted identity from BOTH tables' in-page residue, so a dedup keyed only on
  `(rowid, values)` collapses two genuinely-distinct deleted records into one. The
  table-aware identity keeps all four.
- **md5:** `3b78c970e13aa5793146ade0aed84cdb` — 12288 bytes.
- **Notable contents:** `a`/`b` each 1 live row (`KEEP-a`/`KEEP-b`), deleted
  `(1,'DUP-1')` and `(2,'DUP-2')` in both; `secure_delete=OFF`, `auto_vacuum=NONE`.

#### dropped_table_schema.db

- **Source:** SYNTHETIC — `sqlite3` CLI (Tier-2; ground truth derivable from the
  construction).
- **Identity:** a `secrets` table is created, populated, then `DROP`ped (under
  `secure_delete=OFF`), leaving only `keep` live. Its `sqlite_master` row survives
  in page-1 free space. Pins dropped-table **schema recovery**
  (`forensic/tests/dropped_schema.rs`): `recover_dropped_schemas` surfaces the
  dropped object's name + `CREATE` statement, and `audit` reports it as
  `SQLITE-DROPPED-SCHEMA-RECOVERED`.
- **Generator:**

  ```sh
  sqlite3 dropped_table_schema.db <<'SQL'
  PRAGMA page_size=4096; PRAGMA secure_delete=0;
  CREATE TABLE keep(id INTEGER PRIMARY KEY, x TEXT);
  INSERT INTO keep VALUES (1,'a'),(2,'b');
  CREATE TABLE secrets(id INTEGER PRIMARY KEY, account TEXT, password TEXT);
  INSERT INTO secrets VALUES (1,'admin','hunter2'),(2,'root','toor');
  DROP TABLE secrets;
  SQL
  ```

- **md5:** `69087f66e1fc37a47ebf1803d951301d` — 12288 bytes.
- **Notable contents:** live table `keep`; dropped table `secrets` recovered with
  its `CREATE TABLE secrets(...)` statement from page-1 residue.

#### nist_dlc_snapshot.db  (REAL-ext, NIST public domain, **Tier-1**)

- **Source:** NIST CFReDS **Data Leakage Case** (2015) — the Google Drive client
  `snapshot.db`, recovered from a **Volume Shadow Copy** of the case's PC disk
  image. Authored by NIST (not us, not our oracles); the published answer key is
  the independent ground truth. U.S.-Government public domain.
- **Case + answer key:**
  <https://cfreds-archive.nist.gov/data_leakage_case/data-leakage-case.html> ·
  answer PDF <https://cfreds-archive.nist.gov/data_leakage_case/leakage-answers.pdf>
  (question 49: "What files were deleted from Google Drive?").
- **Extraction recipe** (the PC image is ~20 GB, gitignored/not committed — only
  the 20 KB `snapshot.db` is committed):

  ```sh
  # download cfreds_2015_data_leakage_pc.7z.001..003, then:
  7z x cfreds_2015_data_leakage_pc.7z.001          # -> cfreds_2015_data_leakage_pc.dd
  mmls cfreds_2015_data_leakage_pc.dd              # C: partition starts at sector 206848
  # open the partition's Volume Shadow Copy (libvshadow) and read the NTFS within
  # it (pytsk3): /Users/informant/AppData/Local/Google/Drive/user_default/snapshot.db
  ```

- **md5:** `a37a765981eea87d2c2cd5f7be0c6c0a` — 20480 bytes, `page_size` 1024.
- **Notable contents / ground truth:** live `cloud_entry` holds only the Drive
  `root`; the two **deleted** file entries —
  `do_u_wanna_build_a_snow_man.mp3` (a clean freed cell, RowID 3) and
  `happy_holiday.jpg` (a **freeblock-clobbered** cell, first 4 bytes overwritten)
  — are both recovered by `forensic/tests/nist_dlc_snapshot.rs`, the latter via
  freeblock reconstruction (surviving tail `…holiday.jpg`).

#### fuzz_render_alloc_bomb.db  (FUZZ, regression)

- **Source:** FUZZ — a `render`-campaign crash artifact minimized by libFuzzer
  (Tier-2; the contract is "does not abort", not a recall answer key). Committed
  verbatim as `crash-ebdd2ca632cdce51e172def5dd1414e4da57383b`.
- **Identity:** a 536-byte malformed SQLite file whose table-leaf cell declares a
  ~5.9-exabyte spilled `payload_len` varint. Before the fix, the spill branch of
  `decode_leaf_cell` (`core/src/lib.rs`) reached `Vec::with_capacity(total)` with
  that untrusted length and aborted the process with
  `memory allocation of 5955248086788122959 bytes failed`. Pins the allocation-cap
  guard (`forensic/tests/fuzz_render_alloc_bomb.rs`): a payload larger than the
  file can physically supply (`local + per_page * page_bound`) is rejected with
  `MalformedOverflow` before the allocation, so `open → carve` returns cleanly.
- **Generator:** libFuzzer `render` target over arbitrary bytes; no CLI recipe —
  the crash bytes are the fixture. Corpus seed also under
  `fuzz/corpus/render/` (gitignored).
- **md5:** `6fe4622248008bf248eb367f37477c2c` — 536 bytes.
- **Notable contents:** oversized spilled-payload cell; carving degrades to an
  empty/partial result rather than aborting.

#### sqlcipher/  (REAL-engine SQLCipher ciphertext, Tier-2 decryption oracle)

- **Source:** SYNTHETIC — minted by the **SQLCipher 4.17.0 CLI**
  (`/opt/homebrew/bin/sqlcipher`, an independent OpenSSL-backed implementation —
  the decryption oracle). Ground truth is derivable from the construction below.
  The 16-byte per-file salt is random, so a re-mint yields different bytes; the
  committed files are the pinned artifacts these md5s refer to.
- **Consumed by:** `core/tests/sqlcipher_oracle.rs` — our RustCrypto decryptor
  (`sqlite_core::sqlcipher`) must reproduce the plaintext the engine produced, then
  the native reader reads back the known rows.
- **Common schema** (all three): `t(id INTEGER PRIMARY KEY, name TEXT, val INTEGER)`
  with rows `(1,'alpha',100) (2,'bravo',200) (3,'unicode-snow',300)`;
  passphrase fixtures share the passphrase `correct horse battery staple`.
- **Generators:**

  ```sh
  # enc_v4.db — SQLCipher 4 defaults (PBKDF2/HMAC-SHA512, 256000 iter, page 4096, reserve 80)
  sqlcipher enc_v4.db <<'SQL'
  PRAGMA key = 'correct horse battery staple';
  CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, val INTEGER);
  INSERT INTO t VALUES (1,'alpha',100),(2,'bravo',200),(3,'unicode-snow',300);
  CREATE TABLE notes(body TEXT);
  INSERT INTO notes VALUES ('the quick brown fox');
  SQL

  # enc_v3.db — SQLCipher 3 compatibility (PBKDF2/HMAC-SHA1, 64000 iter, page 1024, reserve 48)
  sqlcipher enc_v3.db <<'SQL'
  PRAGMA key = 'correct horse battery staple';
  PRAGMA cipher_compatibility = 3;
  CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, val INTEGER);
  INSERT INTO t VALUES (1,'alpha',100),(2,'bravo',200),(3,'unicode-snow',300);
  SQL

  # enc_rawkey.db — raw 32-byte key (no passphrase KDF for the encryption key)
  sqlcipher enc_rawkey.db <<'SQL'
  PRAGMA key = "x'2b7e151628aed2a6abf7158809cf4f3c762e7160f38b4da56a784d9045190cfe'";
  CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, val INTEGER);
  INSERT INTO t VALUES (1,'alpha',100),(2,'bravo',200),(3,'unicode-snow',300);
  SQL
  ```

- **md5:**
  - `enc_v4.db`     `dca83d44f81d66154b0417b1ef6a295d` — 12288 bytes (3 pages).
  - `enc_v3.db`     `e6a1cc04a264f67ce9169de82b749cc5` — 2048 bytes (2 pages).
  - `enc_rawkey.db` `18ea1699a334c0667edad678bba08131` — 8192 bytes (2 pages).
- **Notable contents:** first 16 bytes are the random salt (NOT the `SQLite
  format 3\0` magic); `enc_v4.db` additionally holds table `notes` (root page 3)
  with one row `the quick brown fox`. Version is auto-detected by page-1 HMAC
  verification; a wrong key matches no profile and fails loud.
