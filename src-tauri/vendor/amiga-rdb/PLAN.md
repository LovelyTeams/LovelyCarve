# Implementation plan

The goal is a *complete* RDB implementation, not a convenient subset:
everything the format can express, readable and writable, so that no
consumer ever has to reach around this crate to amitools or hand-rolled
block pokes. Staged by risk (read → create → mutate), per m68k-machine's
ADR 0004. Boxes get ticked as they land; anything discovered missing
gets *added here first* so the plan stays the map.

## Already landed (0.1)

- [x] `BlockSource` trait, 512-byte blocks, `block_count` optional
- [x] Checksum verification (hostile `SummedLongs` fails, never panics)
- [x] RDSK scan over blocks 0..16, bad-checksum RDSK skipped (stale-copy case)
- [x] PART chain walk: cycle detection, off-disk detection, per-block ID+checksum
- [x] `DosEnvec` through `de_DosType`, extent computation, flags (bootable/noautomount)
- [x] `PartitionSource` — the offsetting adapter filesystem crates mount
- [x] `SeekBlockSource` (`std` only), `examples/rdbinfo.rs`
- [x] Validated against real images: AmigaOS 3.2.2 (DOS\3), AROS (DOS\7),
      two-partition soak layout; agrees with m68k-machine's 68k boot-ROM
      mounter field-for-field

## Milestone 1 — read side complete

Everything the format can say, surfaced. Nothing here writes a byte.

- [x] **Full `DosEnvec`**: `de_TableSize` 17–20 fields (`de_Baud`,
      `de_Control`, `de_BootBlocks`); expose the raw longwords too, so a
      consumer can round-trip an envec this crate doesn't fully model.
- [x] **Remaining RDSK fields**: vendor/product/revision (space-padded,
      not BCPL), `rdb_HostID`, `rdb_DriveInit`, controller fields,
      `rdb_HighRDSKBlock`, park/interleave/precomp geometry.
- [x] **`rdb_Flags` semantics** as named constants (`LAST`, `LASTLUN`,
      `LASTTID`, `NORESELECT`, `DISKID`, `CTRLRID`, `SYNCH`), not a bare u32.
- [x] **FSHD chain**: `FileSysHeaderBlock` — `fhb_DosType`,
      `fhb_Version`, `fhb_PatchFlags` and the patched fields it gates
      (`Type`, `Task`, `Lock`, `Handler`, `StackSize`, `Priority`,
      `Startup`, `GlobalVec`), `fhb_SegListBlocks` chain head. Same
      cycle/bounds/checksum discipline as PART.
- [x] **LSEG chain**: reassemble `lsb_LoadData` runs into the driver's
      hunk-format binary. This crate reassembles bytes; it does not
      implement hunk relocation (that is the loader's job, wherever the
      driver ends up running). **This is the AROS DOS\7 critical path** —
      reading proves the layout understanding that writing will need.
- [x] **BADB chain**: bad-block lists. Nearly extinct in practice, in
      the format forever; read them so a repartitioner can preserve them.
- [x] **Non-512 `rdb_BlockBytes` is required, not optional.** The
      survey answered itself with arithmetic: every RDB block count and
      cylinder field is 32-bit, so 512-byte device blocks cap the
      addressable disk at 2 TB. Larger `BlockBytes` is how the format
      reaches modern media at all (4 KB → 16 TB, and matches flash's
      native block size; 32 KB → 256 TB), and real systems run this
      way today — 8 TB drives carved into 2 TB partitions are in live
      use. Power-of-two sizes 512..=32 KB. **API consequence, decide
      early:** block size becomes a runtime property of the source
      rather than the `BLOCK_SIZE` const baked into `BlockSource`'s
      buffer type — this reshapes the trait, `PartitionSource`, and
      every LBA in the public API (all of which must be documented as
      *device* blocks of the source's size), so it lands before the
      API attracts consumers, not after.
- [x] **Partition `SizeBlock` ≠ 128**: filesystem blocks larger than
      the device block are not just legal but in live use — FFS with
      32 KB blocks on 2 TB partitions is a working real-world setup.
      `Partition` already records it; extent math and `PartitionSource`
      stay in *device* blocks, documented loudly, since a
      device-block/filesystem-block confusion is exactly where a
      corruption goes silent. (Two independent knobs: `rdb_BlockBytes`
      is the device's block size, `de_SizeBlock` the filesystem's *per
      partition* — and per partition means exactly that: one 512-byte
      RDB disk can carry a 32 KB-block partition and a 4 KB-block
      partition side by side. A synthetic mixed-`SizeBlock` fixture
      goes in the test suite so nothing ever assumes one value per
      disk.)
- [x] **Overlap validation** (`Rdb::validate()`): reports (a)
      any chained block — PART, FSHD, LSEG, BADB — lying outside
      `rdb_RDBBlocksLo..=Hi`, and (b) any partition extent overlapping
      the RDB area. Landed with (c) partitions overlapping *each other*
      as well — beyond the original item, but the same failure family,
      the same consequence, and one extra pass over the partition pairs.
      Shape: `Rdb::validate() -> Vec<ValidationIssue>` for the three
      eagerly parsed chains and the extents;
      `Rdb::validate_seg_lists(&mut S)` for LSEG, which is lazy by
      design and so needs the disk back. An inverted `Lo > Hi` area is
      itself an issue, reported once, and suppresses the two checks that
      would otherwise compare against a meaningless range.
      Both are real: partitioning tools have been known
      to write RDB structures past the reserved area into the first
      partition when the area was too small — after which each side
      trashes the other, both believing they own the same blocks — so
      damaged-by-construction images exist in the wild. The
      parser still *reads* them (the data is there and a recovery tool
      needs it); validation is how a consumer learns the layout is
      mutually destructive before either side scribbles on the other.

## Milestone 2 — create from scratch

Write-path code with nothing to corrupt: build a fresh RDB on an empty
target, read it straight back, compare. The API external consumers
(Copperline's dynamic drive creation, amibake's image builds) actually
want.

- [x] **`BlockSink`**: the write seam, mirroring `BlockSource` —
      `block_size()`, `write_block(&mut self, lba, &[u8])`, optional
      `block_count()`.

      **Decided: two traits, not one.** Read-only sources are the
      common case and nearly all of this crate's work — a file opened
      for reading, a mapped image, a `&[u8]`, an emulator's read-only
      medium — and folding `write_block` into `BlockSource` would force
      every one of them to supply a method that can only fail at
      runtime, throwing away a compile-time truth. Split, "this code
      writes" is visible in the bound: `S: BlockSource + BlockSink`
      where both are needed, `S: BlockSource` where they are not, and a
      read-only source cannot be handed to a writer by accident. The
      cost is `block_size`/`block_count` on both traits — accepted;
      a type implementing both has them agree trivially, and making
      `BlockSink: BlockSource` instead would rule out a write-only
      target (a fresh image streamed out) for no gain.

      `SeekBlockSource` keeps its name and gains
      `impl BlockSink for SeekBlockSource<T> where T: Read + Write +
      Seek` — one type, both directions, so a read view and a write
      view of one file cannot disagree about the block size. A
      read-only `T` simply does not get the impl, which is the split
      earning its keep. The test `MemDisk` grew the matching impl and
      refuses a write past its end, so "the layout runs off the disk"
      surfaces instead of the sink silently growing.

      **`PartitionSink` deliberately not built** — see milestone 3.
- [x] **Checksum sealing**: `seal_checksum(block: &mut [u8],
      summed_longs: u32) -> Result<(), SealError>`, the exact inverse of
      `checksum_ok` and the shared seal for every block writer. Stores
      `SummedLongs` at byte 4, zeroes `ChkSum` at 8, sums that many
      big-endian longwords with wrapping arithmetic and stores the
      negation at 8.

      **Signature notes.** `summed_longs` is the caller's rather than
      inferred, because the count is part of what the *structure* says
      about itself and differs per block type (64 for RDSK/PART/FSHD
      whatever the device block size, `block_size / 4` for LSEG,
      header-plus-entries for BADB) — there is nothing in a half-built
      block to derive it from. It **errors rather than clamps** on a
      count no block could satisfy: clamping would seal a different
      number of longwords than asked for, leaving the block's own
      header disagreeing with the layout the caller was writing —
      better to fail where the arithmetic was done than to write a
      self-consistent lie. Two variants, `SummedLongsTooShort` (below
      `MIN_SUMMED_LONGS` = 3, so the sum would not cover `ChkSum`
      itself and *no* stored value could zero it — its own test proves
      that) and `SummedLongsTooLong`, which also catches a block too
      small to hold the header, so no length panics. Comes with
      `put_be32`, the public inverse of `be32`.

      The test-local `seal()` helper is now a three-line address-
      arithmetic wrapper over the public function, so every fixture in
      the suite — several hundred blocks — exercises the production
      sealer, and the round-trip property is proved incidentally
      everywhere on top of the deliberate sweep
      (`seal_then_checksum_ok_round_trips`: all seven block sizes ×
      six content patterns × six longword counts).
- [x] **Geometry synthesis**: `synthesize_geometry(total_bytes: u64,
      block_size: usize) -> Result<Geometry, GeometryError>`.

      **Convention: amitools' `rdbtool`, pinned against 0.8.1.** Tools
      differ and there is no right answer — geometry is invented on any
      modern medium — so interoperability decides it: amitools' images
      are this crate's fixtures and `xdftool` is the differential
      oracle the round-trip item below diffs against, so a *different*
      geometry for the same size would make every such comparison a
      false positive.

      **What it does**, established empirically by creating images at a
      spread of sizes and reading back what `rdbtool` chose. Two
      candidates, the one wasting fewer bytes wins, first wins an exact
      tie:

      1. *PC-ish*: 63 sectors; heads from the classic BIOS breakpoints
         applied to the **requested byte size** (≤ 504 MiB → 16, then
         32, 64, 128 at each doubling, 256 above 4032 MiB).
      2. *Amiga-ish*: 32 sectors, 1 head, then while cylinders > 65535,
         halve cylinders and double heads.

      Both compute `cylinders = (bytes / block_size) / (heads *
      sectors)`, **rounding down** — a geometry describes at most the
      disk asked for and the trailing partial cylinder is
      unaddressable, which is the only safe direction (rounding up puts
      a partition's last cylinder past the end of the medium).
      Candidate 2 wins almost everywhere, its cylinder being 16 KiB
      against candidate 1's ~504 KiB; the exceptions are sizes that are
      an exact multiple of candidate 1's cylinder, where both waste
      nothing and the tie hands it over — which is why `rdbtool` emits
      63 sectors for exactly 51 609 600 bytes and 32 sectors for
      10 MiB. Head and sector choices do **not** depend on block size
      (candidate 1's table is byte-based, candidate 2's start values
      fixed); only the cylinder count scales, and with it where the
      halving bites.

      **One deliberate deviation**: a candidate whose fields would not
      fit `rdb_Cylinders`/`rdb_Heads`/`rdb_CylBlocks` is discarded, and
      `GeometryError::TooLarge` returned if that leaves none.
      `rdbtool` is Python, whose integers do not wrap, so it has no
      answer here at all; a wrapped geometry describing a disk that is
      not there — with every partition placed against it pointing
      somewhere real and wrong — is the one outcome this crate will not
      produce. The threshold is petabytes past any medium. Below one
      32-block cylinder is `TooSmall` (16 383 bytes at 512 fails,
      16 384 succeeds, matching `rdbtool` exactly).

      The observed triples are unit tests
      (`geometry_matches_rdbtool_0_8_1`, 35 cases). Beyond those, the
      implementation was cross-checked against the live amitools
      `DiskGeometry` on 430 randomized sizes across all seven supported
      block sizes: zero mismatches. Run as an oracle, never copied.
- [x] **RDSK + PART writing**: `RdbBuilder`, two entry points —
      `new(Geometry)` for a caller cloning a disk it already measured,
      `for_size(total_bytes, block_size)` for one that has a size and no
      opinion (it goes through `synthesize_geometry`). Partitions are
      `PartitionSpec::by_size(bytes)` or `by_cylinders(low, high)`, with
      auto or explicit `DriveName`, bootable + `de_BootPri`, dostype, and
      every envec field overridable. `build(&mut sink) -> Result<RdbLayout,
      BuildError<S::Error>>` returns the LBAs and extents it used; the
      tests re-parse the image anyway, on the principle that the disk is
      the only authority on what is on the disk.

      **Rounding direction: down, and this contradicts the assumption the
      item was written under.** `rdbtool` 0.8.1 does *not* round up: its
      `add` is `cyls = num_bytes // rdisk.get_cylinder_bytes()`, plain
      floor division, verified against images it wrote — 10 MiB on a
      129 024-byte cylinder becomes cylinders 1..=81 (10 450 944 bytes),
      not 82, and a size below one cylinder is refused outright
      ("invalid partition range given!"). Matched exactly, including the
      refusal (`BuildError::PartitionTooSmall`), for the interoperability
      reason that pinned the geometry convention. It is also the same
      direction `synthesize_geometry` rounds, so the crate has one rule
      rather than two: a size is a ceiling, and the tool that hands out
      more than it was asked for is the one that walks off the end of
      something. `by_cylinders` exists for the caller who needs the other
      answer exactly.

      **Every default verified against `rdbtool` 0.8.1** by creating
      images and parsing the raw blocks, not from the NDK's suggestions.
      Envec: `de_TableSize` **16** (so `de_Baud`/`de_Control`/
      `de_BootBlocks` are *absent*, not zero — rdbtool only reaches 19
      when one of them is non-zero), `de_SecOrg` 0, `de_SectorPerBlock`
      1, `de_Reserved` **2**, `de_PreAlloc` 0, `de_Interleave` 0,
      `de_NumBuffers` **30**, `de_BufMemType` 0, `de_MaxTransfer`
      **0x00FFFFFF**, `de_Mask` **0x7FFFFFFE**, `de_DosType`
      **0x444F5303** (`DOS\3`). `de_SizeBlock` is the one default that is
      *not* a constant: rdbtool writes `rdb_BlockBytes / 4` (128 at 512,
      **1024 at 4096**), so `PartitionSpec::size_block_longs` is an
      `Option` and `envec_defaults::size_block_longs(block_size)` is the
      rule. RDSK: `rdb_Flags` **0x7** (LAST|LASTLUN|LASTTID), `HostID`
      **7**, `Interleave` 1, `ParkingZone`/`WritePreComp`/`ReducedWrite`
      all == `rdb_Cylinders`, `StepRate` 3, `AutoParkSeconds` 0,
      `DriveInit`/`BadBlockList`/`FileSysHeaderList` all `CHAIN_END`,
      `SummedLongs` **64 whatever the block size**. RDSK at **block 0**
      and PART blocks consecutively from block 1, both observed.

      **One deliberate deviation**: rdbtool writes
      `"RDBTOOL"`/`"IMAGE"`/`"2012"` into the disk identification strings
      while leaving `rdb_Flags` at 0x7, i.e. *without* `DISK_ID`. By the
      format's own rule those bytes then mean nothing; this crate leaves
      them zero rather than plant an unflagged invention a careless
      reader might print.
- [x] **RDB-area sizing done right** — "does not fit" is a hard error
      from `build()` **before any block is written**, and the structure
      guarantees it rather than the discipline doing so: a private
      `layout()` with no sink in scope computes the complete block
      budget, and every check lives there, so there is no path that can
      write block N+1 after discovering block N was the last one. Proved
      by test: each refusal builds into a `MemDisk` of zeros and asserts
      it is still all zeros. Refused cases: partition past the disk's
      last cylinder (sized or explicit), partition reaching into the RDB
      area, partitions overlapping each other, inverted cylinder range,
      a size below one cylinder, more PART blocks than the area holds,
      a sink smaller than the layout, a geometry describing no blocks, a
      duplicate or unstorable `DriveName`, and a sink whose block size is
      not the geometry's. Zero partitions is *not* an error — rdbtool's
      `create` + `init` produces exactly that, `rdb_PartitionList` is
      `CHAIN_END`, and it is what a caller partitioning in a later step
      wants.

      **Reserved-area policy, observed versus exposed.** rdbtool's own
      default *is* a fixed constant, which is the tension this item
      predicted: it reserves the disk's whole **first cylinder**
      (`rdb_RDBBlocksLo` 0, `rdb_RDBBlocksHi = cyl_blocks - 1`,
      `rdb_LoCylinder` 1) regardless of partition count — 0, 1, 2, 5 and
      10 partitions all give `RDBBlocksHi = 31` on a 32-block cylinder,
      with only `rdb_HighRDSKBlock` moving. Resolved in favour of
      interop for the common case and correctness for the uncommon one:
      the default is rdbtool's first cylinder **or what the layout needs
      plus `RDB_HEADROOM_BLOCKS` (16) of slack, whichever is larger**, so
      images match rdbtool's block-for-block until the point where
      matching it would mean overflowing into the first partition — at
      which point the area grows and `rdb_LoCylinder` moves up with it.
      `RdbBuilder::reserved_blocks(n)` overrides both, for a caller
      reproducing an existing image's area or one that knows what it is
      about to store. Note the ceiling this implies and rdbtool shares:
      an area of *n* blocks holds *n − 1* RDB structures, so the FSHD +
      LSEG payloads of the next item will need the override or the
      growth path, not the first cylinder.
- [x] **FSHD + LSEG writing**: `RdbBuilder::filesystem(FileSystemSpec)`,
      matching the `PartitionSpec` idiom — `FileSystemSpec::new(dostype,
      binary)` plus `version(major, minor)` and setters for the
      patch-flag-gated fields. **This is the other half of the AROS
      DOS\7 fix** — ship a long-name filesystem inside the image so a
      3.1-era ROM can mount DOS\7.

      **The binary is not parsed**, per the hunk non-goal: any bytes are
      accepted, split into `block_size - 20`-byte payloads, chained.
      `rdbtool` accepts arbitrary bytes too, which is how the defaults
      below were observed without needing a real handler to hand.

      **Every default verified against `rdbtool` 0.8.1** the same way
      the envec's were — `create + init + fsadd <file>`, then the raw
      FSHD read back. `fhb_PatchFlags` **0x180 and only 0x180**:
      `SegList` (bit 7) and `GlobalVec` (bit 8), with `fhb_GlobalVec`
      **-1** and `Type`/`Task`/`Lock`/`Handler`/`StackSize`/`Priority`/
      `Startup` left zero *and unpatched* — absent, not zero, which is
      exactly the distinction the read side's `Option`s preserve, so
      the spec's defaults are `None` and setting one turns its bit on.
      `fhb_HostID` **0** — matched even though `rdbtool` writes 7 into
      the RDSK and every PART on the same disk, because the field is as
      meaningless on an image either way and byte-identity with the
      oracle is worth more than tidiness. `fhb_Flags` 0, `fhb_Version`
      packed major<<16|minor. Layout: each FSHD immediately followed by
      its own LSEG run, filesystems after the PART blocks, chained in
      the order added — `rdbtool`'s allocation exactly (FSHD 1, LSEG
      2..7, FSHD 8, LSEG 9..14 for two 2560-byte drivers).

      **LSEG `SummedLongs` is the count actually summed**, floored: five
      header longwords plus `payload_len / 4`. Observed either side of
      every boundary — 493 bytes at 512-byte blocks gives `[128, 5]`,
      496 gives `[128, 6]`, 9000 bytes at 4 KB blocks gives
      `[1024, 1024, 217]` — and it is load-bearing rather than cosmetic:
      `rdbtool fsget` recovers a driver's *byte length* from these
      counts, so the differential extracts our image's driver
      byte-for-byte.

      **One deliberate deviation, and it is a bug on the other side.**
      `rdbtool` 0.8.1 writes that reduced count but sums the *whole
      block* for `ChkSum`. The two agree only while the bytes past the
      declared count are zero — true for a driver whose length is a
      multiple of four, false for any other, whose trailing 1..=3 bytes
      then sit outside the sum it actually took. Such a block fails
      `checksum_ok`, and would fail in a 68k ROM just the same. This
      crate writes the same count with the *correct* sum; the deviation
      is pinned by `rdbtool_writes_an_lseg_that_fails_its_own_checksum`
      rather than worked around, because a differential that tolerated
      the oracle being wrong would be testing nothing.

      **Area accounting.** `layout()`'s `needed` is now `1 + partitions
      + Σ(1 + ceil(len / (block_size - 20)))` — the FSHD payload size is
      known up front, as this plan predicted, so the whole budget is
      still computed before the sink is touched and the growth path and
      the `RdbAreaTooSmall`/`SinkTooSmall` refusals extended with no new
      error variants. This is the first thing that makes the growth path
      matter: a 50 KB driver is 103 blocks against a partition's one, so
      the default area moves well past `rdbtool`'s first cylinder and
      `rdb_LoCylinder` with it. (`rdbtool` refuses this case outright —
      "no space in RDB left" — having fixed the area at creation time.)
      Write order tightened to match: each LSEG chain, then its FSHD,
      then the PART blocks, then the RDSK — every block written only
      after everything it points at.
- [x] **Round-trip property**: every create test parses its own output
      and asserts equality; the amitools oracle (GPL — run, never copy)
      diffed against ours in CI.

      **Round-trip.** Each per-feature test already parses its own image
      and asserts `validate()` is silent; the broad one added on top is
      `rebuilding_from_the_parsed_values_reproduces_the_image` — build,
      parse, rebuild through `RdbBuilder::new(geometry)` +
      `by_cylinders` + explicit `reserved_blocks` + the envec off
      `envec_raw` + the FSHD re-added from the parsed header and
      `load_filesystem`'s bytes, and assert the two images are
      **byte-identical**. It doubles as a coverage assertion about the
      *read* surface: anything the builder writes that the parser does
      not expose would fail it.

      *Byte-identity holds for a driver that is a whole number of LSEG
      payloads, and that is the honest limit.* LSEG records no byte
      count, so `load_filesystem` returns the binary padded to a block;
      feeding that back reproduces the same blocks, but a driver that
      did not fill its last block comes back padded and the rebuilt
      final LSEG sums the whole block where the original summed only as
      far as the driver reached. The difference is two longwords in one
      block — a property of the format, not a defect in the rebuild.

      **Differential.** Four env-gated tests
      (`AMIGA_RDB_DIFFERENTIAL=1`, since amitools is not a build
      dependency): the existing `rdbtool <img> list` extent check, plus
      `rdbtool_reads_a_filesystem_this_crate_built` (`info` for the FSHD
      fields, then `fsget` extracting the driver byte-for-byte — which
      proves the chain, the split, the block order and the SummedLongs
      rule at once), `this_crate_reads_a_filesystem_rdbtool_built` (the
      reverse: rdbtool creates and `fsadd`s, we parse and assert the
      0x180/-1/host-0 defaults, `validate()` and `validate_seg_lists()`
      clean), and the checksum-bug pin above.

      **CI**: a `differential` job that `pip install --user
      amitools==0.8.1` — pinned, because every default here was observed
      against that version and a differential against a moving oracle
      tests nothing — and runs `AMIGA_RDB_DIFFERENTIAL=1 cargo test
      rdbtool`, the filter catching the four gated tests plus the three
      that pin rdbtool-observed constants. The workflow's `TODO` now
      names what is left: the boot-level AROS-fixture differential,
      which needs an image a real ROM can be pointed at.

## Milestone 3 — mutate in place

The risky stage, gated on the differential suite existing first.

**Complete**, bar `PartitionSink` — the one item below, now being
built. Everything else here has landed: the whole `RdbEditor` operation
set, the four properties the AmiPart survey turned up (preserve
unmodelled fields, zero what we stop using, free-block management,
never-touch), refuse-over-overlap in both halves, both area levers plus
`set_geometry_cylinders`, the crash-shape ordering with its exhaustive
truncation tests, and AmiPart running as a second differential oracle.

- [x] **`PartitionSink`** — the writing counterpart to
      `PartitionSource`, deferred on purpose rather than built alongside
      `BlockSink` in milestone 2, on the reasoning that a filesystem
      crate's actual needs (bounds check on every write? a flush? a
      grow-into-free-space story?) were unknown until one existed, and
      guessing would repeat the mistake `rdb_BlockBytes` taught us to
      avoid in the other direction.

      **The consumer arrived.** `amiga-ffs-rs`'s `PLAN.md` (2026-09-09/10)
      confirmed against this crate's published 0.4.0 source — not
      assumed — that `PartitionSource` was read-only, and named the
      shape its `convert` command needs: a partition-scoped write
      window for formatting a fresh volume into an existing partition,
      bounds-checked the same way `PartitionSource::read_block` is
      (a checked-add against the window, a second bound against the
      parent's own block count so a hostile `PART` block cannot forward
      a write past where it claims to end).

      `PartitionSink` mirrors `PartitionSource` exactly: same
      constructor shape (`new(parent, partition)`), `write_block` with
      the identical two-tier bound, `block_size`/`block_count` passed
      through unchanged (never `de_SizeBlock` — the same device-block/
      filesystem-block separation `PartitionSource` already documents).
      No flush method: this crate does not buffer, so there is nothing
      to flush — a caller wanting durability flushes the underlying
      sink itself, the same contract `BlockSink::write_block` already
      documents. **Grow-into-free-space is out of scope**, decided
      explicitly rather than by omission: `PartitionSink` has no view of
      sibling partitions or the RDB's free space (it only ever sees one
      `Partition`'s extent), so "grow the destination" is a question for
      `RdbEditor::resize_partition` before a `PartitionSink` is ever
      constructed, not something the sink could answer with the
      information it has. Both directions can coexist on one partition
      exactly as `BlockSource`/`BlockSink` do: `S: BlockSource +
      BlockSink` composes with `PartitionSource`/`PartitionSink` the
      same way.
- [x] **`PartitionSource` gains `BlockSink` when the parent can take
      it** — the second half of the same request, confirmed the same
      way `PartitionSink` itself was: `amiga-ffs-rs` tried to build
      `convert` for real against what exists today, not guessed at it.
      `format()` takes `S: BlockSink` alone and `PartitionSink` already
      satisfies it, but `Populator::new`/`Mutator::open` — filling a
      formatted volume, which `convert` needs right after `format()` —
      take `S: BlockMedium` (`amiga-ffs-rs`'s own marker trait, one
      type satisfying both `BlockSource` and `BlockSink` with matching
      `Error`). `PartitionSource` and `PartitionSink` are two structs,
      each holding its own exclusive borrow of the parent; neither
      implements the other's trait, and Rust's aliasing rules mean you
      cannot hold both on one partition at once to glue them together
      at the call site. There is no way to construct a single object
      windowed onto one partition that satisfies `BlockMedium` today.

      **Shape: this crate's own precedent, not a new one.**
      `SeekBlockSource<T: Read + Seek>` solves the identical problem
      for a plain file — one struct, a second `impl` gated on the
      inner type's extra capability (`impl<T: Read + Write + Seek>
      BlockSink for SeekBlockSource<T>`), so the block size can never
      differ between a read view and a write view of the same file.
      `PartitionSource` gets the same treatment rather than
      `PartitionSink` gaining `BlockSource` or a third
      `PartitionMedium` type existing beside them: it is the "primary"
      of the pair (built first, holds the composition-seam doc
      comment), and it means a caller constructs exactly one
      `PartitionSource` and gets a type usable everywhere a
      `BlockSource`, a `BlockSink`, *or* (via `amiga-ffs-rs`'s blanket
      impl) a `BlockMedium` is asked for — not two objects to juggle.
      `PartitionSink` itself is untouched.

      The new impl is `impl<'a, S> BlockSink for PartitionSource<'a, S>
      where S: BlockSource + BlockSink<Error = <S as
      BlockSource>::Error>` — the `Error =` constraint matters: without
      it, a parent whose read and write errors are different types
      would make `PartitionSource`'s `BlockSource::Error` and
      `BlockSink::Error` disagree, and `amiga-ffs-rs`'s `BlockMedium`
      blanket impl (which requires exactly matching `Error`s) would
      never fire. The write-side bound-check logic is not new logic —
      it is `PartitionSink::write_block`'s existing two-tier check
      (checked-add against the window, then against the parent's own
      `block_count`), and `PartitionSourceError` already covers both
      directions' failure cases, so no new error variant is needed.

      Same exclusions as `PartitionSink`'s, same reasoning: no flush
      (this crate doesn't buffer), no grow-into-free-space (one
      `Partition`'s extent only, no view of siblings or free space —
      `RdbEditor::resize_partition`'s question, decided first),
      `block_size`/`block_count` passed through unchanged.

      **Surveyed against `rdbtool`'s own command set** (2026-09-09),
      the same discipline as the AmiPart survey below, checking a real
      tool's actual command reference rather than guessing at parity.
      Most of it is already covered — `create`/`open`/`init` by
      `RdbBuilder`/`Rdb::parse`, `add`/`change`/`delete` by
      `add_partition`/the `set_*` edits/`remove_partition`,
      `fsadd`/`fsget`/`fsdelete` by
      `add_filesystem`/`load_filesystem`/`remove_filesystem`, `adjust
      lo=/hi=` by `set_lo_cylinder`/`expand_rdb_area`, and
      `export`/`import` by `PartitionSource`/`PartitionSink` directly
      (bounds-checked better than `rdbtool`'s own). `resize` (the whole
      image file), `map`/`json`/`show`/`list` (presentation — a CLI's
      job, not this library's; `rdbinfo` is explicitly an example) and
      `addimg` (deriving a partition's size and dostype by sniffing a
      filesystem image's own header — the "filesystem contents"
      non-goal by name) are out of scope by design already, not gaps.

      **Two real gaps, filed rather than guessed at, on
      `PartitionSink`'s own precedent**: no `fsflags` equivalent
      ([#4](https://github.com/sidick/amiga-rdb-rs/issues/4) — patching
      an existing filesystem's device-node fields needs
      `replace_filesystem` today, binary and all) and no `free`
      equivalent ([#5](https://github.com/sidick/amiga-rdb-rs/issues/5)
      — no way to ask which cylinder ranges are unclaimed). `remap` and
      `adjust auto` are niche enough to defer past even those
      ([#6](https://github.com/sidick/amiga-rdb-rs/issues/6)). Pending
      work now lives in GitHub issues, not as open items in this
      file — PLAN.md stays the record of what landed and why.
- [x] **AmiPart survey first**: before designing the edit API, read
      AmiPart (MIT, so readable closely — unlike xdftool, which stays a
      run-only GPL oracle) to enumerate the operation set and its edge
      cases: what happens to `rdb_HighRDSKBlock` on delete, how resize
      rounds to cylinder boundaries, what it does with holes in the RDB
      area. The API is shaped by real usage before the first line lands.

      → **`docs/amipart-survey.md`** — operation inventory, resize/move
      and delete edge cases, free-block management (there is none: every
      write regenerates the whole area contiguously), crash shape (the
      RDSK is written *first*), the invariants it keeps and drops, and a
      recommended operation set. The four items it turned up are below.
- [x] **Edit existing structures**: add/delete a partition, change
      flags/bootpri/name, grow/shrink where cylinder math allows.
      AmiPart (MIT) is the readable reference for the operations users
      actually perform, resize edge cases included.

      **Half landed: the metadata edits.** `RdbEditor` — `open`, the
      setters, `commit` — covers everything AmiPart's partition and
      advanced dialogs offer: `set_name`, `set_dos_type`,
      `set_boot_priority`, `set_bootable`, `set_automount`,
      `set_flags_raw` (the escape hatch, so we never do what AmiPart's
      checkbox rebuild does to a flag bit we have not named), the typed
      envec setters (`set_reserved`, `set_pre_alloc`, `set_interleave`,
      `set_num_buffers`, `set_buf_mem_type`, `set_max_transfer`,
      `set_mask`, `set_baud`, `set_control`, `set_boot_blocks`) and the
      disk-level `set_disk_identity` / `set_controller_identity` /
      `set_rdb_flags`.

      **Second half landed: the structural edits.** `add_partition`
      (returning the new index), `remove_partition`, `set_extent` /
      `resize_partition`, `add_filesystem` / `remove_filesystem` /
      `replace_filesystem` / `partitions_using_filesystem`, and
      `set_bad_blocks` / `remove_bad_blocks` — the whole of survey
      §7.2's operation set bar the disk-level
      `set_geometry_cylinders` and the area levers, which are their own
      items below.

      **`PartitionSpec`/`FileSystemSpec` are reused verbatim**, so
      creating a partition and adding one are described the same way and
      *written by the same code*: `fill_part_fields`, `fill_fshd_fields`
      and `fill_lseg_fields` are now free functions that the builder
      seals immediately and the editor leaves to `prepare`'s reseal. A
      created `DH0` and an added one are the same bytes because there is
      one function, not because two were kept in step. Name assignment
      follows the builder's rule — the first `DH`*n* no existing
      partition carries, explicit names never renamed, collisions
      refused.

      **Gap placement policy: first fit, lowest free cylinder upward.**
      `Placement::Size` scans the gaps between the existing extents
      (starting at `rdb_LoCylinder`, or the cylinder after
      `rdb_RDBBlocksHi` when the `RDSK` understates its own area) and
      takes the first run long enough; the cylinder count is floored, as
      on create. Best fit was considered and rejected: it keeps large
      runs intact, but it makes where a partition lands depend on
      partitions the caller was not thinking about, and on a disk with
      single-digit partitions there is nothing to optimise. First fit is
      deterministic, explains itself to a user ("it went in the first
      hole big enough"), reproduces `rdbtool`'s and AmiPart's
      pack-after-the-last behaviour on the usual hole-free layout, and
      fills a hole a delete left rather than growing the used tail —
      which is the point of having a policy at all.
      `EditError::NoRoomForPartition` names the largest gap there was;
      a caller who wants a different answer says `Placement::Cylinders`,
      which is why the pair exists.

      **Delete does not erase the partition.** Removing the table entry
      unchains one `PART` block and nothing else — every byte between
      `de_LowCyl` and `de_HighCyl` stays where it was, and re-adding the
      same extent gets the filesystem back. Documented loudly and tested
      by stamping the extent and comparing after. The *block* is zeroed;
      the contents are not ours to touch.

      **Resize is a table-entry edit, and says so.** `set_extent` and
      `resize_partition` refuse an inverted range, one past the disk's
      last cylinder (the lower of `rdb_HiCylinder` and `rdb_Cylinders -
      1`, which real images disagree about), one reaching into the RDB
      area, and one overlapping another partition — so no edit can
      produce a layout `validate()` would complain about. The docs state
      three destructive facts rather than implying them: shrinking
      destroys the filesystem inside, growing does not grow it, and
      moving `de_LowCyl` relocates every filesystem block relative to
      the partition start.

      **Filesystems: no silent dedupe, and no silent dostype rewrite.**
      `add_filesystem` appends whatever it is given — two `FSHD`s for one
      dostype is a layout the format permits and a caller may want — and
      `replace_filesystem` is the explicit operation AmiPart's `ADDFS`
      documents and does not perform. `remove_filesystem` **returns the
      indices of the partitions whose `de_DosType` matched the removed
      driver** (and `partitions_using_filesystem` asks the same question
      without removing anything); their dostype is left exactly as it
      was. AmiPart rewrites them to `DOS\0` (§1a), which silently
      changes which handler mounts a partition that may be perfectly
      happy with a ROM filesystem of the same dostype. **Reporting
      shape: indices into `partitions()`**, not names — indices are what
      every other editor method takes, so the answer feeds straight back
      into `set_dos_type` or `remove_partition`.

      **`set_bad_blocks` landed rather than being deferred** — it fell
      out of the raw-block model in a dozen lines. Entries are repacked
      into as many `BADB` blocks as they need (`(block_size - 24) / 8`
      each) with each block's `SummedLongs` the header plus *its own*
      entries, which is what makes the count readable back; which entry
      sat in which block carries no information and is not preserved,
      exactly as the read side already says. `remove_bad_blocks` empties
      the chain and zeroes what it used. Being the tool that can rewrite
      a list rather than drop it is the differentiator §7.2 named:
      AmiPart zeroes `rdb_BadBlockList` on every write, orphaning the
      blocks its own bad-block dialog appended (§4).

      **A structure added by the editor has no LBA until the commit**,
      and reads the new public `UNPLACED_BLOCK` (`u64::MAX`, which
      truncates to `CHAIN_END` in the u32 fields — the format's own "no
      block") until then. `CommitReport` is the answer to where it went.

      **`de_TableSize` grows, never shrinks.** The last three envec
      setters extend it when the envec stops short of their field, and
      the longwords that become readable on the way are *zeroed* rather
      than exposing whatever slack the block held: they were absent, and
      absent is not "whatever bytes happened to be there". `envec_raw`
      is re-read from the patched block, so the model still says exactly
      what the block says.

      **Scope, from the survey**: a resize is a *table-entry* edit and
      nothing more. Moving partition data and resizing filesystems is
      what AmiPart's `PART_Move`/`GROW`/`SHRINK` do by reaching into
      FFS/SFS/PFS internals, and it is out of scope here by the crate's
      founding non-goal — so shrinking is destructive to whatever
      filesystem is in the partition and must be documented as such
      rather than quietly offered.
- [x] **Preserve unmodelled fields**: an edit rewrites every field it
      parsed, including the ones it does not interpret —
      `rdb_DriveInit`, `rdb_BadBlockList`, the controller identity
      strings, unknown `rdb_Flags` and `pb_Flags` bits, the raw envec
      longwords above what we model, `de_TableSize` as it was found. A
      *tested* property, not a documented intention: this is AmiPart's
      most damaging behaviour (it drops all of the above on every write,
      `docs/amipart-survey.md` §6) and the one most easily inherited by
      building the write path out of the fields the model happens to
      name.

      **Decided: keep the blocks, not the fields.** `RdbEditor::open`
      reads the whole area — `RDSK`, every `PART`, every `FSHD`, every
      `LSEG` of every driver (which the parser leaves lazy, so the
      editor walks those chains itself), every `BADB` — and holds each
      block's *bytes*. A setter patches the field it is about and
      nothing else. This makes the property structural rather than
      diligent: a longword we have never heard of survives because
      nothing ever took it apart. Exactly five kinds of byte can change
      on a commit — the fields an edit set, the chain pointers
      (`pb_Next`, `fhb_Next`, `lsb_Next`, `bbb_Next`,
      `fhb_SegListBlocks`), the `RDSK`'s three chain heads,
      `rdb_HighRDSKBlock`, and each block's `ChkSum`. Each block's own
      `SummedLongs` is *preserved* rather than replaced with ours, so a
      foreign `PART` summing 128 longwords still sums 128 and an `LSEG`
      still declares its driver's byte length.

      **Tested three ways**: `no_op_commit_is_byte_identical` (open a
      foreign image, commit, assert the disk is the same disk),
      `editing_one_field_changes_only_that_field` (change one
      `de_BootPri` and assert *every* differing byte on the whole image
      lies in that longword or the checksum over it, then compare the
      re-parsed model against the old one field for field), and
      `the_foreign_fixture_carries_what_a_rewrite_would_drop`, which
      proves the fixture is worth testing against in the first place:
      `rdb_DriveInit` set, a `BADB` chain, controller strings, an
      unknown `rdb_Flags` bit, `rdb_Reserved1` filled, unknown
      `pb_Flags` bits, `de_TableSize` 21 with values in every tail
      longword and two past anything we model, non-zero
      `de_SecOrg`/`de_PreAlloc`/`de_Interleave`, slack after the envec,
      an `FSHD` patch mask with a bit above `GlobalVec`, an `LSEG` whose
      `SummedLongs` stops short of its own payload, and bytes outside
      the `RDSK`'s checksum entirely.
- [x] **Zero what we stop using**, inside the RDB area only: a `PART`
      block orphaned by a delete, the tail of the area a shorter layout
      no longer covers, and a stale `RDSK` left behind when the block it
      was found at is not `rdb_RDBBlocksLo`. AmiPart leaves all three on
      disk, checksum-valid and unreferenced, where the next tool's RDSK
      scan can find them. Pairs with never shrinking `rdb_RDBBlocksHi`:
      the area keeps its declared size, and what is inside it is exactly
      what the chains say.

      **Reachable now, and tested end to end.** `commit` computes the
      blocks the old layout used and the new one does not, and zeroes
      them after the `RDSK` has landed, inside the area only (a vacated
      block *outside* it belongs to whoever owns that space now; the
      never-touch guarantee outranks tidiness, and
      `commit_relocates_a_chain_block_from_outside_the_area` asserts
      those blocks are left exactly as they were).

      **The set is computed from the layout `open` read, not from the
      structures that survive the edits** — which is what makes a
      *delete* reach it at all: the block a removed `PART` sat on is
      gone from every list in the editor, and that record is the only
      thing that remembers it was ever ours. `remove_partition`,
      `remove_filesystem` (`FSHD` *and* every block of its `LSEG` chain)
      and `remove_bad_blocks` all land here, each with a test asserting
      every vacated block reads back as 512 zero bytes and that the
      zeroing write comes *after* the `RDSK` in `blocks_written`.

      **Zeroing semantics, stated once:** a vacated block inside the
      area is overwritten with zeros; a vacated block outside it is not
      touched at all; a block that is vacated and *reallocated* in the
      same commit is written with its new contents and not zeroed; and
      nothing outside `rdb_RDBBlocksLo..=Hi` is ever zeroed no matter
      what the old layout used it for. `rdb_RDBBlocksHi` is still never
      shrunk, so the area keeps its declared size and what is inside it
      is exactly what the chains say.
      The stale-`RDSK` case does not arise from us at all: the editor
      never moves the `RDSK` (see the crash-shape item), so it cannot
      leave a second one behind.
- [x] **Free-block management** inside the RDB area: reuse holes left
      by deleted PART/FSHD/LSEG blocks before extending toward
      `rdb_HighRDSKBlock`.

      **Satisfied by design: minimal motion over a contiguous-repack-free
      model.** There is no allocator state, no bitmap and no compaction
      pass, because there is nothing to compact — a structure already
      inside the area keeps its block, and one that has nowhere to go
      takes a block the layout does not occupy. Holes *are* the free
      list: the layout is what the chains say, and every block of the
      area they do not name is available. This is the exact opposite of
      AmiPart, which needs no free-block management because every write
      regenerates the whole area contiguously (§4) — and which therefore
      cannot leave a hole, cannot keep a chain order, and rewrites every
      block on every edit.

      **Two tiers, and the order is the crash shape.** First choice is
      the lowest block *neither* the old nor the new layout uses — a hole
      an earlier edit left, or headroom below `rdb_RDBBlocksHi` — so the
      old chains stay walkable right up to the `RDSK` flip and the swap
      is genuinely atomic (§7.4's option (b), and another reason never to
      shrink the ceiling). Only when the area has no such block left does
      it fall back to a block the old layout is *vacating* in this same
      commit: still correct, since the new `RDSK` publishes the new
      chains and the zeroing pass runs after it, but from the moment that
      block is overwritten the old table can no longer be walked past it.
      That is the honest cost of a full area; the alternative would be
      refusing an edit the format allows.

      **Proved by `a_block_freed_by_a_delete_is_reused_by_a_later_add`**:
      delete, commit (the block is zeroed), add, commit — and the new
      `PART` lands on exactly the block the delete freed, with no growth
      and no repack.
- [x] **Never-touch guarantee**: mutation writes only inside
      `rdb_RDBBlocksLo..=Hi`, asserted in code, not just documented —
      partition contents are provably out of reach. This is precisely
      the invariant whose absence causes the overlap bug above; here it
      is structural.

      **Structural via `LeasedSink`**, a private type that owns the sink
      for the whole of a commit and refuses any LBA above
      `rdb_RDBBlocksHi`. Nothing else in the commit path has the sink in
      scope, so there is no code around the check — and
      `CommitError::OutsideRdbArea` is the variant that would fire if
      the placement arithmetic above it were ever wrong, instead of a
      block landing in a filesystem. The window is `0..=Hi` rather than
      `Lo..=Hi` because the `RDSK` may legally sit *below* the area it
      declares (`rdb_RDBBlocksLo` 1 on a disk carrying a foreign boot
      sector), and rewriting it where it was found is not a violation.
      `commit_writes_only_inside_the_rdb_area` tracks every LBA a commit
      asks for and compares both partitions' extents byte for byte
      afterwards.
- [x] **Refuse-over-overlap**: an edit whose blocks don't fit the
      existing RDB area fails with an error naming the shortfall — the
      overlap outcome does not exist as a behaviour — and unlike a
      tool with history, there is no legacy write-anyway path to keep.

      **Two halves, both landed.** The *blocks* half is
      `CommitError::RdbAreaTooSmall`, which names `needed`, `available`
      and the area, is raised by `plan()` (no sink in scope, so a refused
      commit provably writes nothing), and says in as many words that
      growing `rdb_RDBBlocksHi` is not implemented yet — the next item.
      The *extents* half is new with the structural edits:
      `add_partition`, `set_extent` and `resize_partition` refuse
      `EditError::CylindersInverted`, `PastEndOfDisk`, `OverlapsRdbArea`
      and `PartitionsOverlap` before touching the editor's own blocks,
      using the same block arithmetic `validate()` reports with — so an
      edit cannot produce a layout the parser would then complain about,
      and there is no `ENFORCESIZE`-style opt-out or silent clamp
      (AmiPart clamps a too-large `HIGH` by default, §2). Every refusal
      has a test asserting the sink, or the editor, is untouched.
- [x] **`set_geometry_cylinders(n)`** — AmiPart's `INIT NEWGEO`, the
      disk-got-bigger case: keep `rdb_Heads`/`rdb_Sectors` and
      `rdb_LoCylinder`, raise `rdb_Cylinders`/`rdb_HiCylinder`. Named in
      survey §7.2 and *not* in the structural-edit chunk that landed —
      recorded here so the plan stays the map. It pairs with the area
      items below (both are disk-level rewrites of the `RDSK` alone) and
      needs the same predicate in reverse: shrinking the geometry under
      an existing partition is the refusal case.

      **Landed as specified**, with `rdb_HiCylinder = cylinders - 1`.
      Three refusals: `EditError::CylindersBelowPartition` names the
      partition whose `de_HighCyl` the new count does not reach;
      `EditError::ClaimsBlocksPastEndOfDisk` covers both a geometry
      shrunk so far that the RDB area no longer fits inside the disk it
      describes (which also disposes of `cylinders == 0`, since an area
      always contains at least one block) and a geometry claiming more
      blocks than the source reported at `open`;
      `EditError::UnusableGeometry` for a zero-block cylinder.

      **`rdb_Park`, `rdb_WritePreComp` and `rdb_ReducedWrite` are left
      alone**, though creation tools commonly set all three to the
      cylinder count and AmiPart rewrites them here. That is the
      preserve-unmodelled-fields rule applied to fields we *do* model:
      an edit changes what it was asked to change, and all three are
      public on `Rdb` for a caller who wants them to track.

      **The disk-size check is asymmetric with `expand_rdb_area`, on
      purpose.** Growing the geometry is checked against
      `BlockSource::block_count` when the source gave one and taken at
      its word when it did not — a claim about the medium is exactly
      what the call *is*, and the realistic flow (clone onto a bigger
      disk, then `NEWGEO`) has the bigger disk in hand. Growing the
      *area* is checked against the lower of `block_count` and the
      geometry's own total, because that one decides where this crate
      will write.
- [x] **Expand the RDB area**: grow `rdb_RDBBlocksHi` (and
      `rdb_HighRDSKBlock`) when — and only when — validation proves the
      blocks being claimed are not inside any partition's extent. The
      realistic enabler for editing old images created with the
      historically tiny default area; pairs with the user resizing or
      moving the first partition to free the space.

      **Both levers, per survey §7.3.** `expand_rdb_area(new_hi)` raises
      `rdb_RDBBlocksHi`; `set_lo_cylinder(n)` raises `rdb_LoCylinder`.
      They are genuinely different operations — the first can often be
      done with no partition change at all, when the first partition
      starts above the area, which is the common case — and AmiPart only
      has the second because for it the reserved area simply *is*
      everything below `lo_cyl`.

      **The predicate is AmiPart's, stated in blocks.** An expansion is
      permitted iff the newly claimed blocks `old_hi + 1 ..= new_hi`
      intersect no partition's `start_lba..start_lba + block_len` — the
      same arithmetic `ValidationIssue::PartitionOverlapsRdbArea`
      performs, so an expansion is refused exactly when a parse of the
      result would have complained, and no edit can produce a layout
      `validate()` would then flag. Only the *newly claimed* blocks are
      checked: a partition already overlapping the old area is damage
      this crate did not cause and will not deepen, and requiring it to
      be repaired first would block the expansion that is very often how
      it gets repaired. `EditError::RdbAreaBlocked` names the blocking
      partition **and the cylinder it would have to move to** (in that
      partition's own `de_Surfaces`/`de_BlocksPerTrack` cylinders, which
      need not be the drive's), modelled on
      `MSG_PV_OVERFLOW_BLOCKED`: "there is no room" alone is a dead end
      for whoever has to act on it. `set_lo_cylinder` refuses on the same
      shape, `EditError::LoCylinderBlocked` naming the partition that
      starts below the new boundary; *lowering* it passes the predicate
      trivially and is allowed, since where partitions may actually start
      is floored by the cylinder after `rdb_RDBBlocksHi` anyway.

      **Never the other way**: a `new_hi` below the current one is
      `EditError::RdbAreaWouldShrink`, not a shrink — the declared area
      is a lease. Equal is a no-op and succeeds.
      `rdb_HighRDSKBlock` is *not* touched here: it is the high-water
      mark of what the layout occupies and the commit recomputes it.
      Expanding makes room; it does not itself use any.

      **`CommitError::RdbAreaTooSmall` now names the remedy** —
      "grow it with expand_rdb_area" — which is the whole point of the
      item: the same `add_filesystem` that was refused succeeds after an
      expansion, and its blocks land in the space the expansion claimed.

      **The lease follows the NEW `Hi` on the commit that grows it, and
      that is the subtle part.** `LeasedSink` takes its ceiling from the
      editor's `rdb_RDBBlocksHi`, so an expanding commit may write above
      the *old* one — before the `RDSK` flip publishes the larger area,
      because `RDSK`-last is the crash-shape rule. Leasing the old
      ceiling instead would refuse every write the expansion exists to
      enable, so the inversion is not a compromise but the operation
      itself. It is safe because the expansion is validated *first*: the
      claimed blocks were proved to belong to no partition and to lie
      inside the disk (`EditError::ClaimsBlocksPastEndOfDisk` against the
      source at edit time, `CommitError::RdbAreaPastEndOfDisk` against
      the sink at commit time — the sink being the only authority present
      when the writing happens). So the blocks written above the old
      ceiling are owned by nobody, and the never-touch guarantee still
      holds in the sense that matters: no partition's contents are
      reachable.

      **The commit-time disk check runs only when this editor moved the
      ceiling.** An editor that did not is not re-litigated — it writes
      nowhere it was not already entitled to write, and refusing an image
      whose stored `rdb_RDBBlocksHi` was always past the end of its own
      medium would break the no-op commit's byte-identity on exactly the
      damaged images this crate exists to repair.
- [x] **Crash-shape discipline**: order writes so an interrupted edit
      leaves the *old* chain intact (write new blocks first, flip the
      chain pointer last). The format has no journal; ordering is all
      there is.

      **Order**: every chained block first — `LSEG` before the `FSHD`
      that heads it, each chain written from its **tail forward**, so no
      block is ever written before the block it points at — then the
      `RDSK` alone as the single pointer flip, then the zeroing pass.
      The exact reverse of AmiPart's ascending order, which puts the
      `RDSK` first and so publishes a table before the blocks it points
      at exist (survey §5); there is no compatibility reason to
      reproduce that.

      **Placement is minimal motion, which is §7.4's option (b) taken
      one step further.** A structure already inside the area keeps the
      block it was found on; only a structure with nowhere to go — one
      *outside* the area today, a new one once adding lands — is
      allocated a block the current layout does not occupy. So the
      option-(b) property (new blocks land where the live chains are
      not) holds, and on top of it an edit that changes no structure
      count moves nothing at all: the only blocks overwritten in place
      are the ones being rewritten *as themselves*, every one sealed
      before it is written, on a chain whose shape did not change. There
      is no intermediate state in which a chain leads into garbage, and
      the in-place repack of option (a) is never needed — a layout that
      does not fit is refused (`CommitError::RdbAreaTooSmall`, which
      names the shortfall and points at `expand_rdb_area`) rather than
      packed destructively.

      **`commit_truncated_at_every_write_leaves_a_readable_rdb`** is the
      test the survey demands: a sink that fails after *n* writes, for
      every *n*, re-parsing after each. The RDB always parses, always
      validates clean (`validate` and `validate_seg_lists` both), always
      carries both partitions, its `BADB` entries and its `rdb_DriveInit`,
      the driver always reassembles, and the edited partition is either
      its old self or its new one — never a mixture.

      **Extended to an expanding commit**
      (`an_expanding_commit_truncated_at_every_write_leaves_a_readable_rdb`):
      expand the area, add a driver that only fits because of it, and cut
      the commit off after every write. The old-or-new property holds
      throughout, and the test pins the one subtlety an expansion brings.
      *A truncated expanding commit can leave blocks written above the
      **old** published `rdb_RDBBlocksHi`, which the `RDSK` still on disk
      does not claim.* That is harmless and it is the price of the
      operation: those blocks are referenced by nothing (the old chains
      live entirely below the old ceiling), owned by nothing (the
      expansion proved no partition holds them), and are either
      overwritten by the next successful commit or left as unreferenced
      bytes in reserved space. The test asserts that exactly rather than
      loosening the property — every issue `validate()` and
      `validate_seg_lists()` report across the interrupted states is a
      `BlockOutsideRdbArea` for a block in the claimed-but-unpublished
      region, and once the `RDSK` has landed there are none at all.

      **One deliberate difference from the survey's sketch: the `RDSK`
      is never moved.** AmiPart normalises it to `rdb_RDBBlocksLo`; we
      rewrite it where the parse found it. Moving it means a window in
      which two checksum-valid `RDSK` blocks describe two layouts and
      the format's scan takes the *lower* — the failure that the
      RDSK-last ordering exists to prevent, reintroduced by the
      relocation. An `RDSK` above `rdb_RDBBlocksHi` is
      `CommitError::RdskOutsideRdbArea` rather than a relocation.

      **`commit` takes `S: BlockSink` alone**, not `BlockSource +
      BlockSink`: the editor already holds every block it needs, so a
      read bound would be a bound nothing uses — and by the same
      argument that made `BlockSink` a separate trait in milestone 2, it
      would rule out a write-only target for no gain. AmiPart's
      read-back verification pass (survey §5) is the thing that *would*
      want the source back, and it is a separate decision from this
      one.
- [x] **AmiPart as second differential oracle**: once mutation lands,
      apply the same edit in both tools and compare the results — a
      second independent implementation alongside the xdftool round-trip
      diff from milestone 2, and one whose source can legally be
      consulted when the comparison disagrees.

      **Re-scoped by the survey**: this cannot be a block-by-block diff,
      and not because either tool is wrong. AmiPart regenerates the RDB
      area from its own model on every write, so its output differs from
      ours by construction — chain order (it sorts partitions by
      `de_LowCyl`), `rdb_RDBBlocksHi` (shrunk to `rdb_HighRDSKBlock`),
      `rdb_BadBlockList`/`rdb_DriveInit`/controller strings (zeroed),
      `de_TableSize` (forced to 19), the dead geometry fields, and the
      `RDSK` location. The comparison is therefore **semantic**: parse
      both images with this crate and compare the models on the fields
      both tools claim to own. `docs/amipart-survey.md` §1c and §6 list
      the divergences to expect.

      **It builds on macOS, with three small local patches.** AmiPart
      ships a native host CLI (`host/`, plain `gcc`, no dependencies,
      `make -C host`) that shares `src/rdb.c` and the whole engine with
      the m68k binary and operates on `.hdf` images — exactly what the
      survey said it would. Three things stopped it, none of them deep:
      `host/amiga_shim.c` includes `<linux/fs.h>` for `BLKGETSIZE64`/
      `BLKSSZGET` (raw-device path only; guarded behind `__linux__`, and
      the existing `lseek(SEEK_END)` fallback covers the rest);
      eleven shim entry points — `PutStr`, `Output`, `Input`, `Flush`,
      `FGetC`, `PrintFault`, `ExamineFH`, `Delay`, `SetSignal`,
      `Inhibit`, `ColdReboot` — are defined in `amiga_shim.c` but
      declared nowhere, which is a warning under gcc and a hard error
      under clang 16+ (declarations added to `host/amiga_compat.h`,
      copied from the definitions — `Output`/`Input` return `BPTR`, so
      implicit `int` would have been wrong as well as noisy); and
      `src/partclone.c` passes a `MoveProgressFn` where an
      `FFS_ProgressFn` is wanted, again a gcc warning and a clang error
      (`-Wno-incompatible-function-pointer-types`, since it is in the
      partition-move path this crate has no business in).

      Two further host-build facts worth recording, both found by
      running it: `SetFileSize` is a stub returning 0, so AmiPart's own
      `CREATE SIZE=` cannot make the image (the tests write the zeros
      themselves and use `INIT NEW`); and `IMAGE=` is capped at **58
      characters** (`src/cli.c:resolve_target`), an AmigaDOS-sized limit
      that a macOS `$TMPDIR` path blows through on its own, so the
      helper runs `amipart` with its working directory set to the
      image's and hands it a bare file name.

      **Three env-gated tests** (`AMIGA_RDB_AMIPART=1`, binary from
      `AMIGA_RDB_AMIPART_BIN` or `amipart` on `PATH`), all comparing
      *semantically*: parse both images with this crate and compare
      partitions (name, `de_LowCyl`/`de_HighCyl`, `start_lba`,
      `block_len`, dostype, the bootable/automount flag bits, `de_BootPri`
      — sorted by `de_LowCyl`, which normalises away the chain-order
      divergence) and filesystems (dostype, version, and the driver bytes
      an `LSEG` walk gives back).

      - `amipart_and_this_crate_agree_on_an_added_partition` — AmiPart
        `INIT NEW` + `ADDPART DH0`, copied twice, then `ADDPART WORK`
        via its CLI on one copy and `add_partition` via `RdbEditor` on
        the other.
      - `amipart_and_this_crate_agree_on_an_added_filesystem` — the same
        shape with `ADDFS` against `add_filesystem`, over a 2564-byte
        driver so the partial final `LSEG` block is part of what is
        compared. The driver bytes are the sharp end: they prove the
        chain, the payload split and the block order agree with a second
        implementation in a way no field assertion could.
      - `this_crate_preserves_an_image_amipart_wrote` — open an
        AmiPart-written image with two partitions and a driver, commit
        with no edits, assert the disk is **byte-identical**. The
        preserve-unmodelled-fields property against a foreign *writer*
        rather than a fixture, `de_TableSize` 19 and all.

      **What the differential found: agreement on every compared field,
      and one structural consequence worth having.** AmiPart shrinks
      `rdb_RDBBlocksHi` to `rdb_HighRDSKBlock` on every write, so the
      area it leaves behind is exactly full — an `add_partition` on an
      AmiPart-written image is `CommitError::RdbAreaTooSmall` until
      `expand_rdb_area` is called. The first test asserts
      `rdb_blocks_hi == high_rdsk_block` on the image it was handed and
      then expands, which is the area lever exercised against a real
      foreign writer rather than a fixture, and is precisely the
      "historically tiny area" case the item above was written for.

      **Deliberately not in CI.** AmiPart is a macOS-local oracle: its
      host build is an unpackaged `gcc` target, and putting it on the
      runner would mean pinning a third-party git revision plus the
      patch set above. `rdbtool` stays the CI differential. The recipe,
      for the record:

      ```
      git clone https://github.com/ChuckyGang/AmiPart
      # apply the three patches described above, then
      make -C AmiPart/host
      AMIGA_RDB_AMIPART=1 AMIGA_RDB_AMIPART_BIN=$PWD/AmiPart/host/amipart \
          cargo test amipart
      ```

## Cross-cutting

- [x] **Errors**: `Display` for every error type; `std::error::Error`
      under the `std` feature. `Display` is `core::fmt`, so it holds in
      `no_std` too; the `Io`/`Parent` variants require `E: Display` on
      the impl only, so a source whose error is `()` still parses. Under
      `std`, `source()` returns the wrapped error, so `?` into
      `Box<dyn Error>` works and the chain survives.
- [x] **Fuzzing**: `cargo-fuzz` target for `Rdb::parse` over arbitrary
      images — the parser already refuses cycles/bounds/checksums, and
      the fuzzer's job is to prove there is no panic path left.
      `fuzz/fuzz_targets/parse.rs` parses the input as a disk image and,
      when the parse *succeeds*, keeps going into everything else
      attacker data reaches: `validate()`, `load_filesystem` for each
      FSHD, `validate_seg_lists`, and a `PartitionSource` read at both
      ends of every extent. Every `Result` is discarded on purpose — an
      `Err` is the specified behaviour, so only a panic or a sanitizer
      report counts.

      **The editor is in the target too**, added with milestone 3's last
      chunk. After a successful parse it runs `RdbEditor::open` on the
      same image, applies a handful of edits steered by the selector
      byte — `set_boot_priority`, `set_name`, `set_rdb_flags`,
      `set_lo_cylinder`, `set_geometry_cylinders`, `expand_rdb_area`,
      `remove_partition`, `add_filesystem` — and commits to a `VecSink`
      holding a copy of the image. That is where the crate's arithmetic
      is densest (block allocation over an attacker-chosen
      `rdb_RDBBlocksLo`/`Hi`, cylinder extents, area expansion), so it is
      where an overflow or an index panic would live. Every edit's
      `Result` is discarded, as on the read side. **Two things are
      asserted rather than discarded**, because they are the write path's
      contract: the sink panics on any write above `rdb_RDBBlocksHi`,
      which is the never-touch guarantee checked from *outside* the
      crate rather than by `LeasedSink` checking itself; and an image a
      commit reported success on must parse back. 3.1 million runs on the
      committed corpus, clean.

      **Input mapping.** The image sits at offset 0 and the *last* byte
      is a block-size selector, `512 << (sel % 7)`, covering all seven
      supported device block sizes. Image-first keeps libFuzzer's
      mutations aligned to block boundaries (a leading selector would
      slide the whole image sideways on every insert) and makes a crash
      artifact a real disk image once the last byte is chopped off.
      The selector matters because block size is a runtime property of
      the source here and `rdb_BlockBytes` must agree with it, so
      without steering it the 4 K/32 K paths would never be entered.
      The in-memory source reports `block_count` and fails reads past
      the end, so the off-disk refusals are exercised rather than
      papered over with zero fill.

      **Seeds are committed, the corpus is not.** Reaching a valid RDSK
      by mutation means guessing a checksum, which coverage feedback
      cannot steer; `fuzz/seeds/` holds two tiny (4 K and 32 K) images —
      RDSK + PART + FSHD + LSEG + BADB, one 512-byte-block, one
      4 K-block — so all four chains are live from the first run.
      `fuzz/corpus`, `fuzz/artifacts` and `fuzz/target` are gitignored.

      **How to run** (needs nightly and `cargo install cargo-fuzz`):

      ```
      cargo +nightly fuzz run parse fuzz/corpus/parse fuzz/seeds \
          -- -max_total_time=60 -max_len=65536
      ```

      `-max_len` has to clear the 32 KB seed or libFuzzer truncates it
      into something that no longer parses. `fuzz/` is its own workspace
      root, so it never joins the parent build graph: `cargo test` and
      `cargo clippy --all-targets` at the repo root neither see nor need
      it. **CI runs exactly this command**, on nightly, as a normal
      blocking job: 60 seconds from the committed seeds is a smoke run,
      not a campaign — enough to catch a panic a change just introduced
      on a shape the seeds already reach, cheap enough per push, and a
      panic is a bug whoever found it. A longer scheduled campaign is
      still the place for depth, when there is a reason.

      **What it found, first minute:** `parse_part` computed the extent
      as an unchecked `high_cyl - low_cyl + 1`, so an inverted cylinder
      range panicked in a debug build and — the worse half — *wrapped*
      in a release one, conjuring a partition claiming most of the
      address space out of two plausible u32s. Fixed: an inverted range
      is an empty extent (`block_len == 0`, which the overlap checks
      already skip), reported by the new
      `ValidationIssue::PartitionCylindersInverted` rather than refused,
      since a recovery tool needs to see the damage. The two extent
      multiplications saturate now as well — all three factors are
      attacker-controlled u32s and their product need not fit a `u64`.
      That saturation then exposed a second panic one layer out, which
      is why the target reaches into `PartitionSource` at all:
      `read_block` added `start_lba + lba` unchecked, so a saturated
      extent made an *in-range* `lba` overflow the parent LBA. It
      returns `OutOfRange` now — out of range is out of range whichever
      end it falls off. All three cases have regression tests; several
      million runs after the fixes, clean.
- [x] **CI** (GitHub Actions): test on stable, `--no-default-features`
      build, clippy `-D warnings`, rustfmt, docs build (`RUSTDOCFLAGS=-D
      warnings`), MSRV. One workflow, `.github/workflows/ci.yml`, jobs
      parallel. The **differential** job landed with milestone 2's
      round-trip item: `amitools==0.8.1` pinned via `pip install
      --user`, `AMIGA_RDB_DIFFERENTIAL=1 cargo test rdbtool`. The
      boot-level differential — a real 3.1-era ROM mounting a
      redistributable AROS `DOS\7` fixture, the one question the
      block-level oracle cannot answer — is filed as
      [#7](https://github.com/sidick/amiga-rdb-rs/issues/7) rather than
      a bare `TODO` in the yaml.
- [x] **MSRV**: 1.63, in `rust-version` and tested by its own CI job
      (test + build only — clippy/rustfmt run on stable, where their
      opinions are current). Verified by actually running the suite on
      the 1.63.0 toolchain; nothing in the crate reaches past it, and
      the floor is deliberately conservative because retro tooling is
      packaged by distros that move slowly. *(README mention still
      pending.)*
- [x] **crates.io**: published at the end of milestone 1 as 0.2.0,
      read-complete being a coherent first release; semver honestly
      since — 0.3.0 for milestone 2+3, 0.4.0 for the review-fix API
      breaks, all pre-1.0 and none silent.
- [x] **Docs**: every public item documented (`#![deny(missing_docs)]`
      once the surface settles); one worked example in the crate docs
      showing disk → partitions → `PartitionSource` → filesystem crate.
      Denied now that milestone 1 has settled the read surface; the
      paired fields that shared one doc comment (`low_cyl`/`high_cyl`,
      the `rdb_blocks_*` and identification strings, every struct-variant
      error field) now each carry their own, which is the point — the
      *inclusive* half of a range is exactly what a reader gets wrong.
      The crate example builds its own one-partition image in hidden
      doctest lines rather than exposing a `#[doc(hidden)]` fixture
      helper, so `cargo test` runs the whole path for real and the public
      API stays what it says it is.

## Review findings (2026-09)

An independent review of the whole crate after 0.3.0, confirmed
finding by finding. Each item below is fixed with a regression test
that fails before the fix and passes after. Several break the public
API, which pre-1.0 is allowed but not silent: the removals below
(`RdbError::EnvecTooShort`, `EditError::UnreadableBlock`) and the added
variants (`RdbError::ChainTooLong`, `RdbError::SharedChain`,
`ValidationIssue::SharedLsegChain`, `PartitionSourceError::BeyondParent`
— additive, but these enums are not `non_exhaustive`, so an exhaustive
match stops compiling) are all noted here and in the changelog when 0.4
goes out.

- [x] **An interrupted commit could publish a mixed table.** `plan`
      kept an in-area block in place and `prepare` then rewrote it with
      a changed `pb_Next` — *before* the `RDSK` flip, so the still
      published old `RDSK` walked its old head straight into the new
      structure. Crash there and the disk carried a checksum-valid
      table that was neither the old one nor the new one: a deleted
      partition still at the head with its overlapping replacement
      spliced in behind it, two filesystems each free to destroy the
      other. The rule now is that a block may only be rewritten where
      it lies if its **pointers do not change** — `Next`, and an
      `FSHD`'s seg-list head; anything else relocates to a block the
      old layout does not use, and the rule cascades back along the
      chain to a fixed point. A change that is *not* a pointer (a name,
      a dostype, a boot priority) still rewrites in place, which is the
      old-or-new-per-field transient this crate has always documented
      and is unaffected by chain shape. Knock-ons: relocation spends
      first-tier blocks, but the two-tier allocator already refuses to
      hand out a block the old layout occupies until nothing else is
      left, so no new `RdbAreaTooSmall` arithmetic was needed; a
      structural edit now moves the chain it touches and zeroes what it
      vacated, which two existing tests were rewritten to assert. The
      truncation test over a structural edit now asserts the strict
      property — every prefix parses as *exactly* the old table or
      *exactly* the new one — and a new test does it in the destructive
      shape, where the replacement partition reuses the deleted one's
      cylinders.
- [x] **End-of-disk checks mixed drive and partition cylinders.**
      `de_Surfaces * de_BlocksPerTrack` is per partition and may differ
      from `rdb_Heads * rdb_Sectors`, so comparing a partition's
      `de_HighCyl` against the drive's last cylinder compares two
      different units: a divergent-geometry partition could be extended
      past the medium, or truncated by a `set_geometry_cylinders`
      shrink that read as passing. Both checks are now in device
      blocks — the unit `validate()` already thinks in — as is the
      `rdb_LoCylinder` floor at the other end, which had the same
      disease. The errors still name cylinders, but computed from
      blocks and in the extent's *own* cylinder, so the number means
      something for a divergent geometry and is unchanged for the usual
      one. `add_partition`, `set_extent` and `resize_partition` all go
      through the one fixed predicate.
- [x] **`place_by_size` panicked on a hostile geometry.** A parsed
      `rdb_Heads * rdb_Sectors` is bounded only by `u32::MAX` squared,
      so a cylinder times the block size overflowed `u64` — a debug
      panic, and in release a wrap to zero and then a division by it.
      Saturating arithmetic, which lands an absurd cylinder in
      `PartitionTooSmall` (no size a caller can express reaches one),
      plus the same treatment for the RDB-area cylinder rounding.
- [x] **One short `DosEnvec` aborted the whole parse.** A `PART` block
      whose `de_TableSize` does not reach `de_DosType` failed
      `Rdb::parse` outright, so one damaged entry cost a recovery tool
      every other partition on the chain — the opposite of this crate's
      read-everything rule. Such a partition is now parsed with what
      the envec declares, absent numeric fields reading as zero (a
      partition with no `de_HighCyl` gets a zero-length extent, the
      inverted case's answer), `envec_raw` preserving exactly the
      longwords the block carried so an edit round-trips it byte for
      byte, and the new `ValidationIssue::EnvecTooShort` reporting it
      where the rest of the layout damage is reported. **API break**:
      `RdbError::EnvecTooShort` is removed — `parse_part` is now
      infallible — and with it `EditError::UnreadableBlock`, whose only
      reason to exist was that call's `Result`.
- [x] **`checksum_ok` broke its own contract twice.** It indexed
      `SummedLongs` at byte 4 without checking the slice was that long,
      so a fragment panicked the host; and it accepted `SummedLongs` of
      1 or 2 though `MIN_SUMMED_LONGS` documents them as unsatisfiable
      — a block whose first longwords happened to sum to zero passed a
      check that had not covered `ChkSum` at all. Both are `false` now.
      Nothing real is excluded: `RDSK`/`PART`/`FSHD` say 64, the
      shortest `BADB` says 6, `LSEG` the whole block.
- [x] **The `RDSK` probe turned end-of-medium into the wrong error.** A
      source that declines to report a block count can only signal its
      end by failing a read, so a four-block image with no RDB on it
      answered `Io` where `NoRdsk` is documented. A failed read now
      ends the *scan*; a genuinely failing device gets `NoRdsk` from
      the probe too, which is the right trade — the probe is a search,
      not a health check — and every read after an `RDSK` is found
      still reports its error faithfully.
- [x] **`SeekBlockSource` read and wrote the wrong block, silently.**
      The byte offset was an unchecked `lba * block_size`: a debug
      panic, and in release a *wrap* — 2⁵⁵ × 512 is exactly zero, so a
      read of a block the disk does not have returned `Ok` with block 0
      in the buffer and a write of one would have landed on the `RDSK`.
      The LBA is attacker-supplied by a short path: a `PART` block with
      a `de_Surfaces`/`de_BlocksPerTrack`/`de_LowCyl` triple of
      2¹⁵/2¹⁵/2²⁵ — three ordinary-looking longwords — gives a
      partition whose block 0 is the disk's block 2⁵⁵, and
      `PartitionSource` forwards it. Both directions now use a checked
      multiply and answer `io::ErrorKind::InvalidInput`. Belt and
      braces at the other end too: `PartitionSource` bounds the parent
      LBA it computes against the parent's own `block_count` when there
      is one (`PartitionSourceError::BeyondParent`, an added variant),
      so it never forwards an LBA it already knows is nonsense to a
      source that may or may not check.
- [x] **`walk_chain` was quadratic, and unbounded without a block
      count.** The visited set was a `Vec` scanned per hop, so the walk
      cost grew with the square of a length the *image* chooses — and a
      chain of tens of thousands of `LSEG` blocks is legitimate, so the
      length cannot simply be refused. It is a `BTreeSet` now (`alloc`,
      no dependency). Separately, a source that reports no
      `block_count` cannot have its chain pointers range-checked, so
      nothing bounded the walk at all: the new `MAX_CHAIN_BLOCKS`
      (2²⁰ blocks — 512 MB of `LSEG` payload at 512-byte blocks, three
      orders of magnitude past any real driver and past most disks of
      the era) ends it with `RdbError::ChainTooLong`. With a block
      count the visited set still ends the walk first, so the constant
      is unreachable on any source that says how big it is.
- [x] **`RdbEditor::open` amplified a shared `LSEG` chain.** *k*
      `FSHD`s pointing at one *L*-block chain made the editor retain
      *k × L* copies of it, both factors chosen by the image. Sharing
      is *damage*, not a layout to support — each `FSHD` owns its
      chain, and an edit to one would rewrite the other's driver from a
      different buffer — so `open` refuses it with the new
      `RdbError::SharedChain`, naming the block that gave it away.
      With that plus the per-chain visited set, every block the editor
      retains is a distinct block of the disk, so a source that reports
      a block count bounds the editor's memory by its own size. The
      read path is unchanged: `parse` and `load_filesystem` walk one
      chain at a time and read each correctly, and `validate_seg_lists`
      — which reports rather than refuses — gained
      `ValidationIssue::SharedLsegChain`, one issue per colliding
      filesystem rather than one per shared block, since the image
      chooses how many of those there are.
- [x] **Two documented promises were wider than the code.** The README
      said an interruption leaves the old table intact, full stop; the
      commit's own docs have always said that the *completely full*
      area falls back to blocks the old table is vacating, past which
      the old table can no longer be walked. The README now promises
      what holds: old-or-new while the area has any headroom,
      RDSK-last damage-bounding without it, and `expand_rdb_area` as
      the remedy. And `RdbBuilder::build` carried a comment claiming "a
      block is written only after everything it points at", which is
      false — it writes each chain head first. The comment is
      corrected rather than the order (the builder's target is empty by
      contract, so nothing reaches those chains until the `RDSK`
      lands; `RdbEditor::commit`, whose target is *not* empty, really
      does write tail-first, and says why).
- [x] **Below-cap, confirmed and fixed in the same pass.**
      `validate()`'s partition-overlap check was an allocating
      *O(P²)* over a partition count the `PART` chain chooses; it is a
      sort plus a sweep now, so the quadratic term is paid only for
      pairs that genuinely overlap — which are pairs the caller asked
      to hear about. `plan()`'s `taken`/`was_original`/`original_links`
      linear scans, run once per structure per fixed-point pass over an
      area whose size the image chooses, are a `BTreeSet` and a
      `BTreeMap` keyed by LBA. `examples/rdbinfo.rs` computed a
      partition's size in MB with an unchecked multiply — a debug panic
      on a hostile `de_HighCyl` — now saturating. The editor's exposed
      model kept a stale `rdb_FileSysHeaderList` after a filesystem was
      added or removed, so a caller reading `rdb()` between the edit and
      the commit saw a chain head with nothing behind it; the head is
      recomputed from the list it heads, and `rdb()` now documents the
      rule it follows (every field is the edited value, every block LBA
      is provisional until `commit`). `Cargo.toml`'s description still
      said "and eventually writing"; it creates and edits. And the
      `rdbinfo` example lacked `required-features = ["std"]`, which is
      why `cargo test --no-default-features --all-targets` failed —
      declared now, and that command is a CI step rather than a thing
      someone might run.

## Non-goals, so they don't creep in

- **Hunk relocation/loading** — reassembling LSEG payload bytes is in
  scope; executing or relocating them is the consumer's loader's job.
- **Filesystem contents** — one filesystem family per crate; this crate
  stops at the partition boundary, always.
- **MBR/GPT coexistence** — hybrid PC/Amiga layouts (as on shared CF
  cards) are a consumer-level composition: they can probe MBR
  themselves and hand this crate an offset `BlockSource`. Revisit only
  if a real consumer hits it.
- **Device I/O** — opening `/dev/...`, image file formats with headers
  (ADF is headerless and fine; HDF likewise; anything wrapped —
  vhd/qcow — is the consumer's problem behind `BlockSource`).
