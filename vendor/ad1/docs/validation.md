# Validation

How `ad1-core`/`ad1-forensic` correctness is established, graded by **who
confirms it** (most-trustworthy first).

## Summary

| Aspect | Tier | Status |
|---|---|---|
| zlib chunk decompression + logical addressing | 2 | ✅ recomputed MD5/SHA1 match independent (flate2 + RustCrypto) ground truth |
| Structural offsets (headers, item, metadata, chunk table) | 1 | ✅ confirmed against FTK-written hashes across all 299,729 files of a real AD1 |
| Robustness / panic-freedom on malformed input | 2 | ✅ malformed-input suite + two cargo-fuzz targets (3 real bugs found & fixed) |
| End-to-end vs FTK Imager stored hashes on `userbss.ad1` | 1 | ✅ 299,729/299,729 files — MD5 + SHA1 both match, 0 mismatches |

## Tier 2 — independent oracle on crafted data

The test fixtures are built by an encoder that follows the al3ks1s/AD1-tools
layout, but the file **data** is compressed by [`flate2`](https://crates.io/crates/flate2)
and the **stored hashes** are computed by RustCrypto
[`md-5`](https://crates.io/crates/md-5)/[`sha1`](https://crates.io/crates/sha1) —
libraries independent of `ad1-core`. So when the reader decompresses a file and
its recomputed MD5 equals the stored MD5, the chunk table walk, cross-segment
logical addressing, and zlib inflate are confirmed against ground truth the
reader did not produce (`read_at_whole_file_matches_data_and_stored_hash`,
`reads_file_data_spanning_multiple_segments`).

The **residual tier-3 risk**: the encoder and decoder share the *structural
offsets* (both take them from the same C reference). A systematic offset error
would pass these tests. That is exactly what the tier-1 step below closes.

## Tier 2 — fuzzing

Two `cargo-fuzz` targets (`fuzz_reader`, `fuzz_forensic`) drive arbitrary bytes
through `open → entries → read_at` and `audit`. They found and fixed three real
defects: a 72 GB allocation from an attacker-controlled `segment_count`, a
`count + 1` overflow on a `u64::MAX` chunk count, and a `cur - chunk_base`
underflow on a short non-last chunk. Each has a deterministic regression test.

## Tier 1 — real-world data (confirmed)

The authoritative check: reconcile `ad1-core`'s decompression + recomputed hashes
against **FTK Imager**'s stored per-file hashes in a **real** AD1 — `userbss.ad1`
from the [2025 Magnet Virtual Summit CTF](https://cfreds.nist.gov/all/Hexordia/2025MVSCTF)
(Hexordia / Kevin Pagano). FTK is a fully independent implementation, so
agreement confirms the structural offsets the crafted fixtures could not (both
sides of a fixture share those offsets; FTK does not).

- **Image:** `userbss.ad1`, 51,678,663,221 bytes (48 GiB), single segment,
  MD5 `0b6b53e3475b97ae8b3bd3c1e7cec2d9`.
- **Parsed:** version 4, 64 KiB chunks, **316,682** tree entries.
- **Result (full image):** for all **299,729** files, `ad1-core`'s recomputed MD5
  **and** SHA1 (RustCrypto, over its own zlib decompression + logical addressing)
  match FTK's stored values — **299,729/299,729 MD5, 299,729/299,729 SHA1, 0
  mismatches, 0 short decompressions** (~600k independent comparisons, 10m49s).

Reproduce (the image is gitignored — see `tests/data/README.md`):

```sh
AD1_USERBSS=/path/to/userbss.ad1 \
  cargo test -p ad1-core --test tier1_real --release -- --nocapture
# AD1_USERBSS_LIMIT=N caps the file count for a quick smoke run.
```

The test (`core/tests/tier1_real.rs`) is env-gated: it skips cleanly when
`AD1_USERBSS` is unset, so CI stays green without the 48 GiB artifact.
