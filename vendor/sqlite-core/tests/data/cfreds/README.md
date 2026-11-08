# NIST CFReDS SQLite corpus (`tests/data/cfreds/`)

Independent, authoritative **deleted/modified-record ground truth** authored by
the U.S. National Institute of Standards and Technology (NIST) for its Computer
Forensics Tool Testing (CFTT) program. Unlike our own synthetic fixtures (where
we authored both the deleter and the carver — Doer-Checker-weak), every database
here was created on a real device by a third party (NIST) that *also* published
the exact answer key (which rows were deleted/modified, the page size, the
encoding), so a recall/precision number computed against it is real ground truth.

This is the co-located human-facing detail; the single machine index is
[`../../../docs/corpus-catalog.md`](../../../docs/corpus-catalog.md) §K — cross-reference, do not duplicate.

## Source

- **Program:** NIST Computer Forensics Tool Testing (CFTT) — SQLite tool-testing
  reference sets, distributed via the Computer Forensic Reference Data Sets
  (CFReDS) portal and the CFTT *Federated Testing* environment.
- **Specification:** *CFTT — SQLite Data Recovery: Specification, Test Assertions
  and Test Cases* and the *SQLite Recovery Readme*.
  - SQLite tool-testing landing: <https://www.nist.gov/itl/ssd/software-quality-group/computer-forensics-tool-testing-program-cftt/cftt-technical/sqlite>
  - SQLite Recovery Readme (PDF): <https://www.nist.gov/system/files/documents/2022/09/02/SQLiteRecoveryReadme.pdf>
- **Datasets (CFReDS portal):**
  - SFT-01 — "SQLite Databases (Header and Table info)":
    <https://cfreds.nist.gov/all/NIST/SQLiteDatabasesHeaderandTableinfo>
  - SFT-03 — "SQLite Data Recovery (deleted and modified records)":
    <https://cfreds.nist.gov/all/NIST/SQLiteDataRecoverydeletedandmodifiedrecords>
  - SFT-05 — "SQLite Database containing BLOB data":
    <https://cfreds.nist.gov/all/NIST/SQLiteDatabasecontainingBLOBdata>
- **Original download:** each CFReDS dataset links a Google Drive folder; fetched
  with `gdown --folder`. Companion `-wal`/`-shm`/`-journal` sidecars and the
  `SQLiteDocumentation.rtf` ground-truth document are part of the dataset folders.
- **Devices:** databases were created with `sqlite 3.19.0` (Android) and
  `sqlite 3.32.3` (iOS).
- **Licence:** works of the U.S. Government are **public domain** in the United
  States (17 U.S.C. § 105). Redistribution of this vendored subset is unrestricted.
  Attribution to NIST is retained out of courtesy and provenance hygiene.

## Integrity — verified against NIST-published hashes

Every `.sqlite` file's MD5 was checked against the hash NIST publishes in
`SQLiteDocumentation.rtf` (committed here as `CFReDS-SQLite-ground-truth.rtf`):
**10/10 match.** This authenticates the artifacts as the genuine NIST corpus, not
a fake-200 / re-encoded download.

> **Handling note (forensic hygiene):** open these files **read-only and
> immutable** only — `sqlite3 "file:PATH?mode=ro&immutable=1"`. Opening a
> WAL-mode db in default (read-write) mode checkpoints the `-wal` into the main
> file, mutating the artifact and destroying the WAL ground truth.

## What is vendored (and what is not)

| set | files | NIST ground truth | exercises |
|---|---|---|---|
| **SFT-01** | `sft-01_*` (6 `.sqlite` + 2 `-journal`) | page size, journal mode, **encoding**, tables, columns, 100 rows | `core/tests/cfreds_encoding.rs` |
| **SFT-03** | `SFT-03_PERSIST_*`, `sft-03-WAL_*` (4 `.sqlite` + `-journal`/`-wal`/`-shm`) | 100 deleted + 100 modified `invoice_items` rows (~2000 total) | `forensic/tests/cfreds_recovery.rs` |
| **SFT-05** | *not committed* | BLOB/type reporting; 206 MB per file | documented-only (env-gated) |

SFT-05 (BLOB) is **not committed**: each database is ~206 MB, far over the
small-fixture threshold — it is the gitignored/env-gated class per the fleet
test-data provenance standard. Re-download from the SFT-05 dataset link above
when BLOB validation is needed.

### SFT-01 — encodings and page sizes (NIST ground truth)

| variation | encoding | page size | journal mode | rows (Albums, Weekly_Ratings) |
|---|---|---|---|---|
| `sft-01_utf8_wal` | UTF-8 | 4096 | WAL | 100, 100 |
| `sft-01_utf16be_persist` | UTF-16BE | 1024 | PERSIST | 100, 100 |
| `sft-01_utf16le_off` | UTF-16LE | 8192 | OFF | 100, 100 |

The same logical `Albums` table stored under three encodings must decode to
byte-identical Unicode (first row `(1, "WALK LIKE AN EGYPTIAN", "Bangles",
"Columbia")`) — a UTF-16 db mis-read as UTF-8 yields NUL-interleaved mojibake.

### SFT-03 — deleted & modified records (NIST ground truth)

`invoice_items` (a Chinook-derived table, `InvoiceLineId INTEGER PRIMARY KEY`,
~2240 rows). NIST committed **100 deletes** and **100 modifications**
(`UPDATE … SET Quantity = 200`) per variation:

- **PERSIST** (rollback-journal mode): the 100 deletes leave the live table at
  2140 rows (PKs `1..2240` minus 100), and the 100 modifications set `Quantity=200`.
  Both survive in the `-journal` page images (header zeroed post-commit, bodies
  intact) and are **recovered 100/100** (deletes + modified prior values) by
  `carve_rollback_journal` — validated by `forensic/tests/cfreds_journal_recovery.rs`.
- **WAL** (uncheckpointed): the main db retains all 2240 rows (pre-delete); the
  100 deletes live in the `-wal`. Our WAL handling surfaces both the 2240
  (main-only) and 2140 (WAL-applied) states. Validated by
  `forensic/tests/cfreds_recovery.rs`.

### Rollback-journal recovery (validated)

Rollback-journal (`-journal`) carving — the DELETE/PERSIST analog of WAL-frame
recovery — is implemented (`carve_rollback_journal`): it diffs the journal's
pre-transaction snapshot against the live db and recovers **100/100 deletes +
100/100 modified prior values** from SFT-03 PERSIST (ios + android). It auto-folds
into the combined workbook (deletes red, modifications blue) and JSONL, source
`rollback-journal`. Design + format details:
[`../../../docs/design/journal-recovery.md`](../../../docs/design/journal-recovery.md).
Remaining: the §6 anomaly observations (hot journal, checksum mismatch, …) into
the `audit` output.

## MD5 manifest

| file | md5 | bytes | NIST-published |
|---|---|---|---|
| `sft-01_utf8_wal_ios.sqlite` | `e3a55b86c26ffa14ad80b03c0f19fc5f` | 24576 | ✓ |
| `sft-01_utf8_wal_android.sqlite` | `f2af4866f3b413331e2271c0ff7bd27e` | 24576 | ✓ |
| `sft-01_utf16be_persist_ios.sqlite` | `050ca34817a01990bc01a795c989d805` | 15360 | ✓ |
| `sft-01_utf16be_persist_android.sqlite` | `2cecc0c57a08b9cc7dcbf79fc709c925` | 15360 | ✓ |
| `sft-01_utf16be_persist_ios.sqlite-journal` | `a49f3056a1ec698960fd560a54b60ece` | 3608 | companion |
| `sft-01_utf16be_persist_android.sqlite-journal` | `ca73132379f043755a1f4c726e085ee3` | 3608 | companion |
| `sft-01_utf16le_off_ios.sqlite` | `012ec1241e8ae60295845c1eb56fc96a` | 49152 | ✓ |
| `sft-01_utf16le_off_android.sqlite` | `3b61eb03fac965e4e1cd675cbaa28bf3` | 49152 | ✓ |
| `SFT-03_PERSIST_ios.sqlite` | `8233765792b1717a6041137dfc35bf8e` | 163840 | ✓ |
| `SFT-03_PERSIST_android.sqlite` | `ff8571e426b93c658e16c66bdc276291` | 163840 | ✓ |
| `SFT-03_PERSIST_ios.sqlite-journal` | `c254dd2a52e64ab1b9ee905caaefee0a` | 57968 | companion |
| `SFT-03_PERSIST_android.sqlite-journal` | `ffd6bd7ac00a11a0832e8fc0fb375618` | 57968 | companion |
| `sft-03-WAL_ios.sqlite` | `3b09e123eafe41afab769f34a296453c` | 176128 | ✓ |
| `sft-03-WAL_android.sqlite` | `424fa1917b366122c2dd8ae9b5419ee2` | 176128 | ✓ |
| `sft-03-WAL_ios.sqlite-wal` | `5c06b76ce5628e601443472a3dd94129` | 65952 | companion |
| `sft-03-WAL_android.sqlite-wal` | `cd464a1dde5e1f3b4f089a26c623ba16` | 65952 | companion |
| `sft-03-WAL_ios.sqlite-shm` | `a4bf442e42045c41dc342dd9506f940d` | 32768 | companion |
| `sft-03-WAL_android.sqlite-shm` | `1491f478e2b33694400cb232f4e74633` | 32768 | companion |
| `CFReDS-SQLite-ground-truth.rtf` | `18b406cf17e4e6019bedd159b4ebb4a0` | 33871 | NIST doc |
