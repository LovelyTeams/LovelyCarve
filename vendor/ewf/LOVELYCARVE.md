# Vendored ewf reader

Source: https://github.com/SecurityRonin/ewf-forensic/tree/c456f993f68385f3438b3ff16ba5ab6fd0850fb4/core
Version: ewf 0.4.10. License: Apache-2.0 (see LICENSE).

Only the core reader and its tests are vendored; the forensic integrity analyser,
upstream CLI, workflows and development scripts are not runtime dependencies.
The standalone manifest expands the upstream workspace dependencies.

Local changes:
- Read EWF2 case_data `at` (acquiry_date) and `tt` (system_date), matching
  libewf_case_data.c; keep the upstream synthetic `ad` / `sd` aliases.
- Export segment discovery and explicit-path lazy opening for source snapshots.
- Escape parent paths, support uppercase Ex01 extensions and signature-only names.
- Preserve sector geometry, media type, compression, model/serial, and UTF-16 header2.
- Decode EWF2 bzip2 and pattern-fill chunks; retain its per-chunk checksum flags
  without increasing the 16-byte chunk-table entry size.
- Bound decompression, verify uncompressed Adler-32 and compressed stream trailers,
  and reject short/oversized decoded chunks instead of returning fabricated zeroes.
- Bound EWF2 geometry and reject offset overflow rather than shifting chunk indices.
- Fix the upstream no-default-features VerifyResult impl gate.
- Gate the truncated-image verify test with `verify`, so the host's
  no-default-features configuration can run the library regression suite.
- Update the synthetic raw-chunk test to include the format's Adler-32 trailer.
- Correct the table-header CRC regression expectation and build valid EnCase
  sectors/table fixtures with independent checksums and exact stored chunk sizes.
- Decode reads of at least 1 MiB in up to four coarse Rayon jobs directly into
  disjoint output spans. Bulk reads bypass the decoded-chunk LRU; small random
  reads retain it. Both paths use the same bounded, checksum-validating decoder,
  including unaligned ranges and final partial chunks. Worker errors propagate
  to the host's existing tolerant-read/fault-audit layer.

Run `just test-ewf` from the repository root. Adapter tests are part of `just test`.
Large external corpus tests remain opt-in via EWF_TEST_CORPUS; they are not claimed
as local validation when those files are absent. Small committed fixtures retain
upstream provenance in tests/data/README.md and the upstream test descriptions.

## Bulk-read verification and benchmark

`tests/bulk_reads.rs` compares bulk and serial bytes on committed EWF1/EWF2,
raw/compressed and multi-segment fixtures, checks acquisition MD5 when present,
and covers unaligned ranges, EOF, hot-cache preservation and injected worker I/O
failure. The compatibility suite covers checksum and decoded-length rejection.

Set `EWF_BENCH_IMAGE=<image path>` and run
`just test-ewf --release --features flate2/zlib-rs --test bulk_throughput -- --ignored --nocapture`
for an opt-in full logical-stream benchmark. `EWF_BENCH_MODE` selects `md5`
(default), `sha1` or `read`; `EWF_BENCH_PASSES` controls repetitions, and
`EWF_BENCH_READERS` selects `serial,bulk` (default) or the reverse order. The
serial control uses cached per-chunk reads below the bulk threshold; both paths
use the same open reader and decoder. The benchmark models the host's 16 MiB
reader/hasher pipeline with a bounded channel, but uses this standalone crate's
hash dependencies and release profile; it is not a desktop end-to-end timing.
The explicit `flate2/zlib-rs` feature matches the host's unified backend. Report
read-only decode throughput separately from end-to-end hashing, and distinguish
warm-cache runs from cold reads; the benchmark does not flush the OS cache.

## EWF1 regression fixtures

The EWF1 table header checksum covers bytes `0..20` (including the second
reserved word) and is stored at `20..24`, as documented in libewf's
[`ewf_table.h`](https://github.com/libyal/libewf/blob/33e8dccf18aa0dbb26cee908a3ad122011abbdaf/libewf/ewf_table.h).
The reader already used this range; the old test and layout comments incorrectly
expected `0..16`. The test now uses a fixed checksum and checks corruption of the
reserved word too.

The shared synthetic EnCase fixture now writes `volume`, `sectors`, `table`,
`table2`, `done`, with independently calculated descriptor, volume, table-header
and entry checksums. This matches the sectors-before-table layout in the
[EWF specification](https://github.com/libyal/libewf/blob/33e8dccf18aa0dbb26cee908a3ad122011abbdaf/documentation/Expert%20Witness%20Compression%20Format%20(EWF).asciidoc).
Previously these fixtures put chunks in a separate sectors section after the
table, rather than before it or embedded in an early-format table. The two
failing final-chunk length tests now check exact stored sizes and complete bytes
in eager and lazy mode; a new case covers compressed data larger than its
uncompressed chunk. No production chunk-boundary logic changed for these fixes.

## Header date encodings

The only date fields exposed by `EwfMetadata` are `acquiry_date` and
`system_date`; both retain the stored text. The application adapter adds
`acquiry_date_parsed` / `system_date_parsed` (`display`, `time_basis`) without
changing those raw fields, for both IPC and CLI JSON.

| Section | Acquisition / system keys | Encoding |
| --- | --- | --- |
| EWF1 `header` (zlib, 8-bit text) | `m` / `u` | EnCase/FTK: `YYYY M D h m s`; linen: decimal POSIX seconds |
| EWF1 `header2` (zlib, UTF-16LE) | `m` / `u` | Decimal POSIX seconds |
| EWF2/Ex01 `case_data` (UTF-16LE) | `at` / `tt` | Decimal POSIX seconds, using libewf's implementation mapping |
| Upstream synthetic Ex01 fixtures | `ad` / `sd` | ISO date-only text; aliases retained for compatibility |

Reference: libyal/libewf commit
[`33e8dccf18aa0dbb26cee908a3ad122011abbdaf`](https://github.com/libyal/libewf/tree/33e8dccf18aa0dbb26cee908a3ad122011abbdaf),
`libewf_header_values.c` (`libewf_convert_date_header_value`,
`libewf_convert_date_header2_value`) and `libewf_case_data.c` (the `at` / `tt`
branches of `libewf_case_data_parse_utf8_string_value`). The EWF2 document's
target/actual-time descriptions differ from the implementation; the mapping
above follows the implementation.

Six-part and offset-free ISO dates are displayed as acquisition-machine local
wall time with unknown timezone, without `mktime`, DST normalization or timezone
conversion. Numeric seconds are displayed on the UTC epoch basis with an
explicit Unix-time label; this does not recover the acquisition machine's
offset. Unlike libewf's host-local rendering of numeric seconds, this display is
independent of the examining machine's timezone. ISO date-only values retain
day precision. Invalid dates, overflows, unsupported offsets and unknown formats
fall back to their unchanged raw text. Zero seconds is displayed as the epoch;
it is not inferred to mean a missing acquisition.

The vendor does not expose XML `xheader` dates (including ctime-like strings),
nested source `aq` timestamps or file-entry dates through `EwfMetadata`; this
change does not add those readers. The UI retains both raw fields under the
acquisition disclosure. Adapter parser/model tests cover local dates, epoch
seconds, precision, invalid input and old serialized payloads. Set
`EWF_DATE_IMAGE` to the reported competition E01 when running `just test` to
also check its raw `2026 9 21 9 13 5` and formatted `2026-09-21 09:13:05` values;
the external test returns without reading evidence when the variable is unset.
