# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this crate is

`amiga-rdb` is a pure-Rust, `no_std` + `alloc` library for the Amiga Rigid Disk Block (RDB) partition table format: parsing, creating from scratch, and editing in place — `RDSK`, `PART`, `FSHD`, `LSEG`, `BADB` blocks. Zero dependencies. The `std` feature (default) adds only conveniences (`SeekBlockSource`, `std::error::Error` impls).

**Read PLAN.md before starting any non-trivial work.** It is the actual design document — a staged (read → create → mutate) plan with every decision's reasoning attached, checkboxes ticked as items land, and a rule that governs this whole project: *anything discovered missing gets added to PLAN.md first, so the plan stays the map.* Do not implement something PLAN.md doesn't mention without adding it there. `docs/amipart-survey.md` is a supporting design document (a cited survey of AmiPart's mutation behaviour) that fed the milestone-3 design.

## Commands

```bash
# Full test suite (unit tests + doctest)
cargo test

# no_std + alloc build (the core promise — must always pass)
cargo build --no-default-features
cargo test --no-default-features --all-targets

# Single test
cargo test <test_name>

# Lint / format / docs (all must be clean; CI enforces all of these)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# MSRV check (1.63; install once with `rustup toolchain install 1.63.0 --profile minimal`)
cargo +1.63.0 test

# Differential oracle against amitools' rdbtool (pinned ==0.8.1; pip install --user amitools==0.8.1)
AMIGA_RDB_DIFFERENTIAL=1 cargo test rdbtool

# Differential oracle against AmiPart's host CLI (not on any package registry —
# clone github.com/ChuckyGang/AmiPart and build host/ with gcc; see PLAN.md's
# milestone-3 section for the exact patch set this repo's macOS build needed)
AMIGA_RDB_AMIPART=1 AMIGA_RDB_AMIPART_BIN=/path/to/amipart cargo test amipart

# Fuzzing (needs nightly + cargo-fuzz; corpus is not committed, seeds are)
cargo +nightly fuzz run parse fuzz/corpus/parse fuzz/seeds -- -max_total_time=60 -max_len=65536
```

Neither differential suite runs without its env var set — they're `#[test]`s that return immediately otherwise, so the default `cargo test` needs no external tool. CI runs the `rdbtool` differential (it installs amitools) and a 60-second fuzz smoke on every push; the AmiPart oracle is local-only (no package to install in CI).

Everything lives in one file, `src/lib.rs` (~13k lines, roughly half of it tests) — there is no module split to navigate.

## Architecture

### The one seam: `BlockSource` / `BlockSink`

All I/O goes through two traits: `BlockSource` ("read block N") and `BlockSink` ("write block N"), each reporting its own `block_size()` at *runtime* — not a compile-time const. This is deliberate and load-bearing: the RDB format's 32-bit block fields cap a 512-byte-block disk at 2 TB, so reaching larger media means larger blocks (up to 32 KB), and every LBA in the public API is a *device* block of the source's own size. `rdb_BlockBytes` on disk must agree with the source's `block_size()`, checked at parse time.

Two traits, not one `read`+`write` trait — a read-only source (a file opened read-only, an emulator's ROM-mode medium) never has to implement a `write_block` that can only fail at runtime. Anything that both reads and writes is generic over `S: BlockSource + BlockSink`.

`PartitionSource` is the composition seam with filesystem crates: it offsets a partition's LBAs into the parent device, still speaking the parent's device-block size (never the partition's own `de_SizeBlock`, which is a completely independent, per-partition, filesystem-level knob — one disk can legally carry partitions with different filesystem block sizes).

### Three layers, one per PLAN.md milestone

1. **Read** (`Rdb::parse`) — parses `RDSK`, walks the `PART`/`FSHD`/`BADB` chains eagerly into an owned `Rdb` struct (no source lifetime), and leaves `LSEG` driver payloads lazy behind `Rdb::load_filesystem` (they can be hundreds of KB; most callers don't want them). `Rdb::validate()` / `validate_seg_lists()` report layout damage — blocks with two owners, overlapping partitions — as data, not parse failures: **the parser refuses to invent data but never refuses to read damaged-but-decodable images**, because a recovery tool's only path to the bytes is a successful parse. This is the single most important design invariant in the crate; look for it before "fixing" a validation check into a hard error.

2. **Create** (`RdbBuilder`) — builds a fresh RDB from nothing. The whole block layout is computed and validated in a function with **no sink in scope** (`layout()`), so "does not fit" is structurally impossible to discover after a write has already started — there is no code path that writes block N+1 after finding out block N was the last one that fit. Every default (envec fields, RDSK fields, geometry-to-CHS conversion) was reverse-engineered by observation against `rdbtool` 0.8.1's actual output, not the NDK's suggested values — see `envec_defaults`/`rdsk_defaults`/`fshd_defaults` and `synthesize_geometry`'s doc comment for the provenance and the exact two-candidate-geometry algorithm.

3. **Edit** (`RdbEditor`) — opens an existing RDB, keeps every block's *raw bytes* (not just the fields this crate models), lets you mutate specific fields/add/remove partitions and filesystems, then `commit()`s. Two properties matter more than anything else here:
   - **Byte preservation**: fields this crate doesn't interpret (unknown flag bits, `rdb_DriveInit`, controller identity strings, etc.) must survive an edit unchanged — verified by AmiPart-survey-informed tests that this crate does the opposite of AmiPart's known field-dropping bugs.
   - **Crash-shape discipline**: `commit()` writes new/relocated blocks first and the `RDSK` block *last*, so an interrupted write leaves either the old table or the new one intact, never a splice of both. This is proven by truncation tests that assert the *exact* set of readable partitions at every possible write-count cutoff, not just "still parses". A block already inside the reserved area is rewritten in place only if its chain pointers don't change; if a pointer would change, the block is relocated instead — rewriting a still-referenced block's pointer before the flip is exactly the bug class a 2026 review found and fixed (see PLAN.md's "Review findings" section for the full incident writeup). When the reserved area is completely full, `commit` degrades to a documented best-effort mode rather than pretending the same guarantee holds — `expand_rdb_area` is the way to get headroom back.

### Two independent oracles, not just unit tests

Every non-obvious default or algorithm in this crate should be checked against a real tool's *observed* behavior, not derived from the NDK docs alone — both are GPL, so run them as oracles, never copy their code:
- **amitools' `rdbtool`** (pinned `==0.8.1`) — the CI-integrated oracle for milestones 2 (create) and reading.
- **AmiPart** (MIT, so its source can be read directly, not just run) — the milestone-3 oracle for editing behavior; `docs/amipart-survey.md` catalogs its actual mutation semantics (including bugs this crate deliberately does *not* reproduce).

The crate is also fuzzed (`fuzz/fuzz_targets/parse.rs`) with a 60-second smoke test wired into CI; two real hostile-input crashes have already been found and fixed this way (see PLAN.md's review-findings section) — treat a new fuzz finding as a real bug, not a fuzzer quirk.

### Error handling conventions

Every fallible operation returns a typed error implementing `Display` (works in `no_std`) plus `std::error::Error` under the `std` feature. Parse errors (`RdbError`), validation issues (`ValidationIssue`), edit errors (`EditError`), commit errors (`CommitError`) are separate enums at each layer — don't collapse them. `checksum_ok`/`seal_checksum` are the one pair of functions every block writer and every block verifier goes through; don't hand-roll checksum math elsewhere.
