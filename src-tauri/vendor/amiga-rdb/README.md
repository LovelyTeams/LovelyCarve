# amiga-rdb

Amiga Rigid Disk Block (RDB) partition tables as a pure-Rust,
permissively-licensed library: `RDSK`, `PART`, `FSHD`, `LSEG` and `BADB`
blocks, their checksums, and the `DosEnvec` geometry that tells you where
each partition lives and how to mount it.

In: anything that can read fixed-size blocks (the `BlockSource` trait)
and, for writing, anything that can accept them (`BlockSink`). The
block size is the device's, reported at runtime — 512 is the classic
value, but the format's 32-bit block fields cap such a disk at 2 TB,
and 4 KB-sector disks are in live use. Out, reading: partitions as
extents plus metadata, loadable filesystem drivers reassembled from
their `LSEG` chains, bad-block lists, and `Rdb::validate()` — which
reports blocks with two owners, the layout that parses fine and
destroys itself on the first write. Out, writing: `RdbBuilder` creates
a fresh RDB — the whole layout computed and validated before the first
block lands, filesystem drivers shipped inside the image so a 3.1-era
ROM can mount a dostype it never heard of — and `RdbEditor` mutates an
existing one in place: add/delete/resize partitions, swap drivers,
grow the reserved area, while preserving every byte it does not model
and writing only inside the RDB area. Writes are ordered so that an
interrupted commit leaves a table that parses: the `RDSK` lands last,
and while the reserved area has *any* spare block, every structure
that has to move goes to one the old table does not use — so an
interruption leaves the old table or the new one, whole, never a
splice of the two. In a **completely full** area there is nowhere
spare, and a moved structure lands on a block the old table is
vacating; from that write on, the old table can no longer be walked
past it. The `RDSK`-last order still bounds what that costs — the
published table is never a half-written one — but the honest promise
is old-or-new with headroom, and best-effort without it. `expand_rdb_area`
is the headroom.

What's *inside* a partition is deliberately out of scope — one
filesystem family per crate; a partition composes with a filesystem
crate through a small adapter that offsets LBAs into the parent
device.

`no_std` + `alloc` at the core; the `std` feature (default) adds only
conveniences. No dependencies. MSRV 1.63, tested in CI on that exact
toolchain; raising it is a semver-visible change, not an accident.

## Status

Feature-complete against the plan: read, create and in-place editing
all landed, each validated against independent implementations —
amitools' `rdbtool` runs in CI reading this crate's images and vice
versa, and AmiPart's host CLI agrees field-for-field on the same
edits. The parser and editor are fuzzed, with a smoke run of the
`parse` target in CI on every push. The API is not stable yet:
pre-1.0, breaking changes arrive with a version bump and a changelog
line, never silently.

## Why this exists

There was no permissively-licensed RDB implementation available as a
library: amitools is GPL-2, emulators' implementations are GPL, and the
original is in ROM. This crate is MIT OR Apache-2.0 so that emulators,
image-building tools and hobby OS projects can all use it, whatever
their own licence.

Written against the layouts documented in the AmigaOS NDK, with
[AmiPart](https://github.com/ChuckyGang/AmiPart) (MIT) and NetBSD's
Amiga disk support as permissive C references, and tested
differentially against independent implementations.

## License

Dual-licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
