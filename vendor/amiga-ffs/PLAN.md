# Implementation plan

A complete FFS/OFS implementation — read, create, mutate — staged by
risk. Boxes get ticked as they land; anything discovered missing gets
added here first so the plan stays the map. Companion to amiga-rdb's
plan, which owns everything outside the partition.

## Already landed (0.1)

- [x] `BlockSource`: runtime block size, `u64` LBAs, typed errors,
      `&mut self` — each choice deliberate, each confirmed by watching
      another implementation choose differently and pay for it
- [x] `Variant`: the eight `DOS\0`–`DOS\7` types with their axes
      (FFS/OFS, intl, dircache, long names); unknown dostypes refused.
      **2026-09-09, as-built — a maintainability pass, found by
      independent review.** `Volume::max_name_len`, `Mutator::max_name_len`
      and `Populator::max_name_len` had byte-identical bodies (`if
      variant.has_long_names() { MAX_NAME_LONG } else {
      MAX_NAME_CLASSIC }`), each just reaching its own `Variant` a
      different way — the one capability question `Variant` did not
      already answer itself, next to `is_ffs`/`is_intl`/`has_dircache`/
      `has_long_names`/`fold`. Added `Variant::max_name_len(self) ->
      usize`; the three public methods (kept, unchanged signatures —
      an internal delegation, not an API change) now call
      `self.variant().max_name_len()`. `MutatorVolume::max_name_len`
      already forwarded to `Volume::max_name_len`, so it picks up the
      same answer with no change of its own.
- [x] Both checksum algorithms (standard zero-sum at any block size;
      boot block's end-around-carry) with round-trip tests
- [x] Both case-folding tables (classic ASCII-only vs intl Latin-1,
      with the `÷`/`ÿ` exceptions) and the name-hash function,
      hash-table sizing by block size
- [x] BCPL strings, hostile length bytes clamped

## Milestone 1 — read

**Landed** (commits `db33576`, `76989f3`, `a837b68`): the read side is
complete — every variant, every block size, 82 tests green on stable
and MSRV across both feature sets, and both differential legs passing —
xdftool on all eight variants, fstool on the six it can read. One box
below stays open for the parts of the differential suite that need more
than `cargo test` can reach.

- [x] **Root block**: locate (from geometry / reserved blocks), parse
      name, dates, hash table, bitmap pointers. Detect the variant from
      the *partition's* dostype but verify against what the volume
      actually contains — mismatches exist and must be diagnosable.
- [x] **Directory traversal**: hash-chain walk for lookup by name
      (both fold tables), full enumeration for listing. Classic
      entries first.
- [x] **Long-filename entries** (`DOS\6`/`DOS\7`): the different
      header-block name layout. **Regression to assert from day one:
      every name on a long-name volume non-empty** — the exact failure
      now observed in *two* independent readers against the same
      fixture: affs-read, and AROS's own `afs.handler`, which masks the
      dostype low byte away, mounts a `DOS\7` volume as classic FFS,
      reads the volume name (classic offset in both layouts), and
      resolves no directory entry. Accepting a variant's dostype
      without implementing its layout is this format's signature trap;
      this crate refuses what it cannot parse instead.
- [x] **File reading**: FFS data-block chains via file-header block
      lists and extension blocks; OFS data blocks with their headers
      (and use those headers to *verify*, since they're there).
- [x] **Metadata**: protection bits — the full 32-bit long, including
      the group/other RWED bits — comments, dates (ticks since
      1978-01-01, the 1900-leap-year rules), and the owner longword
      (UID/GID). Owner and the extended bits are first-class, not
      "where present": muFS is in scope (below), and plain FFS carries
      the same fields zeroed.
- [x] **Hard/soft links**: link chains resolved, loops refused.
- [x] **Dircache blocks** (`DOS\4`/`DOS\5`): read them, but treat the
      hash chains as authoritative — caches go stale, and a reader
      that trusts a stale cache invents a directory that isn't there.
      `read_dircache()` is documented advisory and nothing in the crate
      resolves a name through it; `validate()` compares cache against
      chains and reports the six ways they can disagree. Record layout
      (three *words* of DateStamp, a signed type byte, two counted
      strings, word-aligned) confirmed byte for byte against `DOS\5`
      images built by xdftool. The pointer is longword −2, the field a
      file header uses for its extension chain — no collision, because
      the meaning follows the secondary type.
- [x] **Bitmap**: read and verify (block-in-use vs reachable-from-root)
      — the read-side half of `validate()`, and the foundation the
      allocator will stand on. Four inversion-prone conventions each
      confirmed against xdftool-built images: **1 = free**, LSB-first
      within each big-endian longword, first bit is block `reserved`,
      and the checksum is longword **0** rather than 5. Bitmap extension
      blocks are bare pointer arrays — no type, no own key, no checksum
      — with the last longword chaining onward. `bitmap_flag == 0` is
      surfaced (`Bitmap::valid()`), never silently trusted.
- [x] **`validate()`**: checksums, hash-chain membership matches hash
      of name, bitmap consistency, orphan blocks. Parse damaged volumes
      where possible — recovery needs the read side most of all. Returns
      a `Report` of typed `Finding`s rather than erroring: the walk
      records and continues, so a directory with one corrupt chain still
      yields the other seventy-one. Orphans (leaked) and
      reachable-but-free (double-allocation risk) are reported as
      *different* findings, because the mutation ordering in milestone 3
      exists precisely to fail in the first direction and not the second.
- [ ] **Differential suite**: same tree read through this crate,
      xdftool (GPL oracle — run, never copy), fstool (MIT —
      readable when outputs disagree), and AROS's `afs.handler`
      (readable for understanding, licence-incompatible for copying —
      but it runs *inside a guest* against the same images, which
      makes it the one oracle that is also a real consumer). Its
      `getHashKey` already confirms our hash structurally: seed with
      length, `*13 + fold(c) & 0x7FF`, modulo table size, flags
      selecting the fold table. Fixtures from amibake images
      (redistributable AROS DOS\7 included) plus synthetic minimal
      volumes per variant, mixed block sizes 512..=32K.

      **Partly landed**, so the box stays open. `tests/differential.rs`
      does the xdftool leg: it generates all eight `DOS\0`–`DOS\7`
      variants at test time (nothing checked in — a committed ADF is a
      blob nobody can review) and reads each back through this crate —
      whole tree, every byte of every file including one that crosses an
      extension block, dircache against the chains, bitmap against
      xdftool's own `info` accounting, and `validate()` clean on all
      eight. It skips with a printed reason when xdftool is absent, so
      it is never a build dependency.

      **The `fstool` leg has since landed too**, in the same file and the
      same shape (`AMIGA_FFS_FSTOOL`, then `PATH`, skip with a printed
      reason). Six tests, and CI `cargo install fstool --locked`s it and
      asserts presence rather than skipping:

      - **fstool reads what we write**, `DOS\0`–`DOS\5`: the variant
        string from `info`, the whole tree entry-for-entry through `ls`,
        and every byte of every file through `cat` — the 40 000-byte
        extension-block crosser, the empty file, and a Latin-1 `Café`
        whose `é` is one byte on the volume and two in fstool's output.
        `DOS\4`/`DOS\5` read correctly through it even though it knows
        nothing about dircache blocks, because its reader walks all 72
        buckets rather than hashing a name to find one.
      - **We read what it writes**, `DOS\0`–`DOS\3`: tree, bytes,
        `validate()` clean with zero findings, at the ADF geometry and at
        its own default 1 MiB (geometry is a documented don't-care; the
        root-block formula agrees at both sizes, and its boot block leaves
        the root pointer zero, which is fine because `canonical_root_lba`
        derives it).
      - **Mutation, both directions.** `fstool add`/`rm` and the shell's
        `mkdir`/`put` mutate a volume this crate populated — allocating
        from our bitmap, splicing into our hash chains, freeing an
        extension chain we wrote — and we read it back clean, no orphans,
        no reachable-but-free. And the reverse: `Mutator` creates,
        renames across directories (Latin-1 on both ends), deletes and
        sets metadata on a volume `fstool create` produced, fstool reads
        every byte back, then writes into it again. Two independent
        writers taking turns on one image, three turns deep.

      What it cannot do, established by reading its source rather than
      guessing, and asserted so the limits stay facts:

      - **No `DOS\6`/`DOS\7`.** Confirmed, and worse than a refusal:
        `Affs::open` accepts any boot flag 0..=7 and decodes bit 2 as
        "dircache", so a long-name volume opens, is mislabelled `+DC`,
        and `read_name` reads the BCPL name at the classic offset `0x1b0`
        clamped to 30 bytes — which is not where the name is. Long names
        stay this crate's own, and the long-name regression stays the
        thing only we assert.
      - **No `DOS\4`/`DOS\5` writing** — and it does not refuse, which is
        the finding this leg paid for. Two real bugs in fstool 0.4.26,
        both cited in the test that asserts them:
        `Variant::from_flag` sets `intl = flag & 2 != 0`, but
        directory-cache mode *implies* international folding, so
        `hash_name` puts an accented name in the classic-fold slot —
        enumeration finds the file, `Lock()` never will; and `AffsEditor`
        has no notion of a dircache, so the cache AmigaDOS actually
        serves `List` from is left stale. This crate's validator names
        both. Nothing else about the resulting volume is wrong, which is
        what makes them two specific bugs rather than a broken writer.
      - **No comments, no dircache, no block size but 512** (`BSIZE` is a
        compile-time constant), and no protection/owner/date knobs on
        `create`. `-O` takes exactly `fstype`, `intl` and `volume_label`.
      Tracked as GitHub issue #2 — AROS `afs.handler` as a guest
      oracle, and the amibake `DOS\7` fixture; neither is blocked on
      anything here. Two of the four oracles this box originally
      named are landed and wired into CI, which is the substance of
      it; the box stays unticked for the two `cargo test` cannot
      reach.
- [x] **`guard_chain`'s visited set stopped being a `Vec`** (2026-09-09,
      found by independent review). `guard_chain` — every hash-chain,
      extension-chain, link-chain and dircache-chain walk in the crate
      routes through it — tracked "already seen this block" with
      `Vec<u64>::contains`, an O(so-far) scan on every step, so a chain
      of `n` distinct, valid, correctly-checksummed blocks (never
      tripping `ChainCycle`, so the length bound was the only thing
      stopping it) walked in O(n^2). `tests/hostile.rs` already had two
      `#[ignore]`d timing tests pinning this from an earlier review pass
      (26ms at n=2000 vs 242ms at n=8000, a 9.2x ratio for a 4x input);
      they're un-ignored now, with their assertions inverted to check
      growth *stays* close to linear instead of demonstrating that it
      wasn't. `guard_chain` now tracks `visited` in a `BTreeSet<u64>`
      (`alloc::collections`, no new dependency — this crate already pulls
      `BTreeMap` from the same module in `compact.rs`), turning each
      check O(log n) and the walk O(n log n); post-fix the same two
      chain lengths ratio 4.2x in a debug build (0.47ms/1.51ms, 3.2x, in
      release). Three other chain walks had quietly reimplemented the
      same `Vec`-and-`.contains()` shape inline instead of calling
      `guard_chain` and got the same fix: `compact::survey`'s
      directory-visited set, `mutate::refuse_own_subtree`'s
      parent-chain walk, and `repair::sever`'s two visited sets (the
      cross-directory one and each hash slot's own per-chain one).
      Landed alongside the milestone-3 `Mutator::volume()` fix above,
      from the same review pass.
- [x] **`validate()`'s bitmap phase stopped aborting on one bad page**
      (2026-09-09, found by independent review). `Volume::read_bitmap`
      read every bitmap page in one loop and returned a single `Result`
      for the whole bitmap; one page failing its checksum made the whole
      call `Err`, and `validate_bitmap` responded to that `Err` by
      recording one `Finding::Checksum` and returning — skipping
      coverage marking, `BitmapIncomplete`, and the entire
      orphan/reachable-but-free comparison for *every other page too*,
      intact or not, on the whole volume. Confirmed with a failing test
      (`validate_bitmap_survives_one_bad_page`): a two-bitmap-page volume
      with page 1's checksum flipped and a genuine `ReachableButFree`
      block sitting on the still-intact page 0 — before the fix,
      `validate()` reported only the `Checksum` finding on page 1 and
      said nothing about the dangerous double-allocation risk on page 0,
      exactly the failure mode this module's own doc comment says the
      rest of the validator never has ("a directory whose third hash
      chain is corrupt still yields the other seventy-one" — the bitmap
      phase alone did not live up to it). Fixed in `Bitmap` rather than
      `validate()`, judged the smaller and more honest fix: `read_bitmap`
      now treats a page's checksum failure as a fact about *that page
      only* — it records the page's LBA in the new `Bitmap::bad_pages`,
      leaves a same-length placeholder of zero words so every later
      page's block-to-word indexing stays correct, and keeps reading.
      `Bitmap::covers` (and everything built on it — `is_free`,
      `covered`, `covered_count`, `allocated_count`, `free_count`)
      excludes a bad page's block range, answering "cannot assess"
      rather than a wrong yes or no for those blocks specifically, and
      `validate_bitmap` now pushes one `Finding::Checksum` per bad page
      and runs the normal comparison over everything else regardless. An
      `Io` error reading a page (a transport failure, not a checksum
      mismatch) is unchanged and still aborts the whole read — narrower
      in scope than the per-page checksum case, and not part of what
      this review item asked for. Two existing tests asserted the old
      all-or-nothing contract directly
      (`a_bitmap_page_with_a_bad_checksum_is_refused`,
      `repair_rewrites_a_bitmap_page_whose_checksum_is_gone`) and were
      updated to the new one rather than left pinning the bug.

## Milestone 2 — create

**Landed.** Write code with nothing to corrupt: format a fresh volume,
populate from a host tree, read it straight back. The API Copperline
(dynamic OFS/FFS drives from directories) and amibake (dir→hdf) actually
want.

Wave 1 was the seam, `format()` and CI; wave 2 `Populator`, the
round-trip property tests and the fuzzers; the guest-mount proof closed
it under a real Kickstart 3.1.

- [x] **`BlockSink`** mirroring `BlockSource` — the shape amiga-rdb has
      already settled on, adopted verbatim: a *second* trait, not a
      bound on `BlockSource` and not `BlockSink: BlockSource`, because
      read-only sources are the common case and one trait would force
      every one of them to supply a `write_block` that can only fail at
      runtime. `format()` takes `BlockSink` alone (it writes and never
      reads); anything needing both says `S: BlockSource + BlockSink`,
      which is what the test images are.
- [x] **Format**: boot block, root block, bitmap covering the volume, for
      every variant and block size — `format()` in `src/format.rs`, with
      the three placement decisions each settled against an
      xdftool-formatted image and each stated in the module docs. Bitmap
      pages go straight after the root, extension blocks between the two;
      `DOS\4`/`DOS\5` roots carry an empty dircache block from birth (the
      oracle's do, so a null pointer there would be wrong), placed after
      the bitmap rather than at the oracle's own allocator-artefact
      position. The boot block's checksum is left **zero** by default —
      that is what makes it non-bootable, which is what `Format` produces
      and `Install` fixes; one that checksums over 1012 zero bytes is
      strictly worse, because the ROM would accept it and jump in.
      `FormatOptions::boot_checksum` opts in for a caller writing its own
      boot code. Every variant × {512, 1024, 4096, 32768} opens, validates
      with zero findings, and has a bitmap marking exactly the blocks the
      layout allocated; xdftool mounts, lists, writes to and reads back a
      `DOS\1` and a `DOS\3` ADF this crate formatted, and the two
      implementations' fresh images agree longword for longword bar the
      dates and the root's longword −4.
      **2026-09-09, as-built — a separate independent-review pass, this
      one over `format()`.** Only `reserved >= block_count` was refused; `reserved ==
      0` formatted without complaint. The boot-area write is `for lba in
      0..reserved`, so `reserved == 0` writes *nothing* there — the
      dostype never reaches block 0, meaning `Volume::open` cannot
      identify the volume this call just claimed to have formatted — and
      block 0 is simultaneously covered by the bitmap as an ordinary
      free block, so a later allocation can hand it out and a caller
      writes over whatever it expected to find there. `reserved == 1` on
      a 512-byte-block volume has the same problem in miniature: the
      boot area is [`BOOT_AREA_LEN`] (1024 bytes, two 512-byte sectors)
      regardless of the filesystem's own block size, and one block is
      only half of it. Fixed with a new refusal,
      `FormatError::ReservedTooSmall { reserved, min_reserved }`, checked
      before the existing `BadReserved` (which only ever catches
      `reserved` too *large*): `min_reserved` is
      `div_ceil(BOOT_AREA_LEN, block_size)`, the same arithmetic
      `format()`'s own boot-area write already depends on, so the check
      and the write it protects cannot drift apart. Tested
      (`format_refuses_a_reserved_too_small_for_the_boot_area`):
      `reserved: 0` and `reserved: 1` both refused at 512 bytes,
      `reserved: 2` (`DEFAULT_RESERVED`) still formats normally.
- [x] **Populate from tree**: `Populator` in `src/populate.rs`, and
      `populate_from_tree()` on top of it under `std`. Files, directories,
      protection/comment/date/owner, both name layouts (with the
      `T_COMMENT` overflow block when a long LNFS name crowds the comment
      out), OFS and FFS data blocks, `T_LIST` extension blocks, and
      `DOS\4`/`DOS\5` dircache maintenance. Name validation is
      `format()`'s own `check_name_bytes`, generalised over the maximum
      rather than reimplemented — 30 bytes classic, 107 LNFS, Latin-1, no
      `:`/`/`, no control codes.

      Three decisions worth stating. **The allocator is a cursor**, not a
      bitmap search: a populator is constructed from the `FormatLayout`
      the format returned, so the used set is *known* rather than
      discovered, allocation walks upward from block `reserved` (using the
      half of the volume that lives *below* the root, which an
      after-the-root cursor would throw away) stepping over the format's
      one contiguous run, and the bitmap is written **once** at
      `finish()`. Nothing here ever decides whether a block on disk is
      free, so nothing here can decide it wrongly — which is precisely the
      discipline M3 has to acquire and this milestone gets to skip.
      **The session is honest about being unfinished**: construction
      clears the root's `bitmap_flag`, so an interrupted or `abandon()`ed
      populate leaves a volume saying "my bitmap is mid-update" rather
      than one claiming blocks are free while files use them; `finish()`
      restores it last, after the pages it describes are down. **Entries
      go in at the head of their hash chain**, which is what the oracle
      does: three names hashing to slot 54 written in order to a `DOS\3`
      ADF by xdftool come back newest-first, and this crate reproduces
      that order exactly. Dircache records are appended in creation order
      into a chain that spills at block boundaries, as xdftool's do, with
      the one difference that the record's secondary-type byte is filled
      in where xdftool leaves it zero — strictly more information, and a
      validator reads a zero there as "not recorded" either way.

      Host metadata maps as documented on `protection_from_metadata`: the
      owner nibble *denies*, so `u+r`/`u+w`/`u+x` **clear** the R/W/E
      bits; the D bit is always clear because POSIX governs deletion by
      the directory's write bit and there is nothing per-file to map; the
      group and other nibbles grant in the normal sense and are mapped
      straight through; archive/script/pure/hidden stay clear because they
      are facts about a backup, a Shell and an Amiga that the host does
      not have; and the owner longword is **not** derived from the host
      uid/gid, because truncating one into a 16-bit muFS UID would invent
      an owner. Names are converted from UTF-8 to Latin-1 where every
      character fits and refused by name where one does not.

      **2026-09-09, as-built — a maintainability pass, found by
      independent review.** `lib.rs`'s "shape of the API" section and
      `build.rs`'s own doc both state that this format's signature
      trap — reading a long-name volume's name at the wrong offset —
      must have exactly one implementation, but `Populator`'s internal
      `entry_name_at` (used only for the duplicate-name check before
      creating an entry) re-derived the same `variant.has_long_names()`
      branch and the same field offsets independently of `read.rs`'s
      `parse_entry`/`split_nac`, which already make that decision for
      the read path. Confirmed the two were computing the same bytes
      before touching anything — they were. Factored into
      `crate::read::entry_name(block, variant) -> &[u8]`, a
      `pub(crate)` free function next to `split_nac` (its natural home:
      `read.rs` already owns the authoritative decision, and the
      function needs a block slice and a `Variant`, not a `Volume`,
      which is exactly what `Populator` has on hand for a block it just
      read into its scratch buffer). `parse_entry` now calls it too, so
      there is one branch instead of two that merely agreed by
      construction. Tested with a name past the classic 30-byte limit
      on an LNFS volume
      (`a_duplicate_long_name_past_30_bytes_is_refused_on_lnfs`):
      duplicate detection under both the raw and the intl-folded name,
      and a name differing only past byte 30 accepted as genuinely
      different — proof the shared path reads the whole LNFS field
      rather than truncating at the classic length the wrong offset
      would.
- [x] **Round-trip property tests**: `tests/volumes.rs` builds a seeded
      pseudo-random tree (a xorshift written out rather than a dependency
      taken) for **every variant × {512, 1024, 4096}** — varying depth,
      name length including past 30 bytes on LNFS and Latin-1 bytes only
      the intl table folds, file sizes from 0 to ~100 KB so a header's
      pointer table is crossed where the arithmetic says it must be,
      comments up to and past what fits beside a name, and random
      protection/owner/date — writes it, reads it back and compares
      *every* field, then validates with zero findings. Names are compared
      byte for byte rather than under the fold table, because case is
      preserved on disk and a writer that upper-cased what it stored would
      still pass every lookup.

      The differential leg is in `tests/differential.rs` and runs the
      other way: this crate populates the fixture tree on all eight
      variants, xdftool lists it, reads every byte of every file back out
      (including the one that crosses an extension block), *writes a new
      file into it* — allocating from the bitmap `finish()` laid down and
      chaining into a hash table this crate filled — and the result still
      validates here, dircache agreement included. Same skip-if-absent
      rule as the rest of that file.
- [x] **Guest-mount proof**: an image created here boots/mounts under a
      real Amiga ROM (the consumer that cannot be argued with).
      Done under Kickstart 3.1 (A1200) booting Workbench 3.1, via a
      deterministic headless Copperline run: `examples/guest-adf.rs`
      builds a `DOS\3` ADF (formatted and populated entirely by this
      crate — a comment, a 40 000-byte extension-block crosser, a
      Latin-1 `Café-Latin1` name), the volume appears on the Workbench
      desktop by name, and an AmigaShell `list df1: all` +
      `type df1:readme` through the ROM's own FFS handler lists every
      entry with sizes, comment and protection intact and prints the
      file's content — the lowercase `readme` resolving against the
      stored `ReadMe` proving the guest's intl fold agrees with ours.
      Mount is *not* boot: the image is deliberately non-bootable
      (`format()` leaves the boot checksum zero, as `Format` does;
      `Install` is a different program), so Workbench boots from df0
      and the proof volume rides df1. Reproduce with
      `cargo run --example guest-adf -- proof.adf` and a scripted
      `copperline --model A1200 <KS3.1 ROM> --insert-disk-after 0 df1
      proof.adf ...` — deterministic, so the same script yields the
      same screenshots.
- [x] **CI**: stable + MSRV 1.63, `--no-default-features`, clippy
      `-D warnings`, rustfmt, docs; differential job where xdftool is
      installable. Pulled into this milestone because the write side is
      where an untested-configuration regression starts corrupting
      images rather than misreading them. `.github/workflows/ci.yml`
      runs exactly the commands this repository is checked with locally
      — nothing in it is a check the working tree does not already pass.
      **It has not had a live run**: the workflow lands with the code and
      the first push is its first execution. Wave 2 adds a nightly
      `cargo fuzz build` job — see the fuzzing box for why it builds and
      does not run.
- [x] **Fuzzing**: `fuzz/`, cargo-fuzz, nightly-only, with its own
      `[workspace]` so `cargo test`, `cargo clippy --all-targets` and the
      MSRV build never see it. Two targets, because the writer gives the
      fuzzer a second question:

      - `fuzz_read` takes arbitrary bytes as a volume, opens it, walks
        every directory, resolves every link, streams every file, reads
        every comment, dircache, soft link and bitmap, and validates —
        and must never panic, hang or OOM. Files are read with
        `read_file_with` and never `read_file`: the convenient form
        allocates `byte_size` bytes and `byte_size` comes off the disk,
        which is a documented footgun rather than a bug, and a fuzzer
        that tripped it would be reporting the documentation. The walk
        carries its own visited set, because `read_dir` refuses a cycle
        *within* one directory and a cycle across two is only visible to
        the walker. The image is capped at 256 KB: the bugs live in block
        contents, not in block counts.
      - `fuzz_roundtrip` reads the input as a little tree program and
        asserts the property this milestone is for — **any tree the
        writer accepts reads back identical**. `DuplicateName` and
        `VolumeFull` are skipped because they are the writer working;
        every other error is a failure, since they are all statements
        about a caller's input and this caller's input has been made
        legal by construction.

      Sixty seconds each on the seed corpus: 248 843 runs of `fuzz_read`
      (958 edges covered) and 42 005 of `fuzz_roundtrip` (1268), no
      crashes, no timeouts, no OOMs, nothing in `artifacts/`. Nothing
      needed fixing — which is a statement about the read side's existing
      chain guards and length clamps rather than about the fuzzer, and
      the corpus is kept so the next change to either side starts from
      here rather than from noise. The seed corpus is **generated,
      not committed** — `cargo run --example seed-corpus` writes one
      populated volume per variant per block size — for the same reason
      no ADF is checked into `tests/`: a binary blob in a repository is a
      thing nobody can review, and one that drifts out of step with the
      writer that made it is worse than no seed at all.

      CI **builds** the targets on nightly and does not run them. A CI job
      is the wrong place to find a fuzz bug (a minute of libFuzzer proves
      nothing; an hour on every push is somebody else's electricity) and
      exactly the right place to catch the harnesses rotting, which is
      what happens to code that lives outside the workspace and is
      therefore never compiled by `cargo test`. The same job runs the
      corpus generator, so that stays honest too.

## Milestone 3 — mutate

**Landed.** Wave 1 — the allocator and the validator's repair half, the
two pieces everything else in this milestone stands on; wave 2 — create,
delete, rename and set-metadata in volumes this crate did not write,
through `Mutator`; wave 3 — file write, append and truncate, the ranged
read that shares their chain machinery, and the crash sweep over all of
it. The differential mutation suite closed last, guest leg included: the
ROM's own FFS and this crate's `Mutator` applying the same operations to
copies of the same volume, and agreeing about everything either can
observe.

- [x] **Allocator**: bitmap-based block allocation with the volume's
      own policy quirks documented as discovered. `src/allocator.rs`,
      over `S: BlockMedium` (which moved to the crate root, since the
      allocator and the repairer want the same bound `Populator` defined).

      **The policy is first fit from a hint, forward, wrapping once**, and
      the hintless form rotates a cursor so consecutive calls climb rather
      than rescanning the same full region. That is behaviourally what the
      shipping implementations do. Linux's `affs` (`fs/affs/bitmap.c`, GPL
      — read to describe, never copied) takes a *goal* from the caller,
      defaults it to the volume's first allocatable block, scans forward
      within the goal's bitmap page, moves to the next page when it is
      full, and wraps to page zero before giving up; it additionally
      pre-allocates a run of consecutive free bits within the goal's
      longword, which is a performance trick with no on-disk consequence
      and is deliberately *not* copied here (a run handed out and not used
      would have to be given back or leaked, and one call marking exactly
      one block is what makes the ordering rule below statable). xdftool,
      observed rather than read, scans from the *bottom* every time: three
      files on a `DOS\1` ADF filled 866..902, and after deleting the
      middle one and writing another it filled the freed 873..879 and
      882..883 before touching anything higher — first fit with the hint
      pinned to zero. All of these are correct; the difference is locality,
      nothing on disk records which was used, and what the tests assert is
      only the three things that are not a choice: never hand out an
      allocated block, never hand one out twice, never hand out anything
      outside `reserved..block_count`.

      **The crash ordering is in the types, not only in the docs.**
      `allocate()` returns an `Allocation`, not a block number.
      `Allocation::block()` is the LBA to write the block's *own contents*
      at — always safe, because nothing reaches it — and
      `Allocator::reference()` is the LBA that may go into somebody else's
      pointer, which **refuses** (`AllocError::NotDurable`) until the
      bitmap page carrying the bit has been flushed. Between them sits
      `flush()`, which writes only the dirty pages (one dirty flag per
      page; a flag is cleared only after its write returns, so an
      interrupted flush leaves the rest queued) with the checksum in
      longword 0. Freeing cannot be enforced the same way — this code
      cannot see the caller's metadata write — so `free()` is documented
      as the second half of an unlink and refuses the one error it *can*
      see, a double free. `mark_bitmap_invalid`/`mark_bitmap_valid` are
      the session's bookends, the second stamping LNFS `NumBlocksUsed`
      and the flag last.

      A volume whose `bitmap_flag` is 0 is refused outright
      (`AllocError::BitmapInvalid`): those bits are whatever an
      interrupted update left, and rebuilding them is repair's job, not
      an allocator's guess.

      Tested by fill-and-free to exhaustion (every free block once, then
      all of them back), hint and wrap behaviour, double free and
      not-covered refusals, a 4000-step seeded interleave of
      alloc/free/flush cross-checked against a model `HashSet` *and*
      against the bits actually written, a write-counting sink proving a
      flush writes one block per dirty page and nothing on a second call,
      and a crash sweep that stops the medium after every prefix of a
      session's writes: the damage is always leak-shaped (`OrphanBlock`
      allowed) and `ReachableButFree` never appears.

      **2026-09-09, as-built — a maintainability pass, found by
      independent review, not an integrity bug.** `allocate_exact`'s own
      doc comment said reusing `AllocError::NotCovered` for an
      already-allocated destination "would misreport *why*" — and then
      the code did exactly that, leaving a caller unable to tell "this
      destination is occupied, evacuate it first" apart from "this LBA
      isn't even in the volume." Fixed by adding
      `AllocError::AlreadyAllocated { lba }` and returning it where the
      code used to reuse `NotCovered`, and rewriting the doc comment to
      match what the code now actually does rather than what it used to
      contradict. Checked every call site (`compact.rs`'s
      `DestPick::Exact`, the only one): none pattern-matches on
      `NotCovered` to catch this case specifically, so the fix is the
      variant and the doc, not a behaviour change any existing caller
      depended on. Tested both directions
      (`allocate_exact_on_an_occupied_block_reports_already_allocated_not_not_covered`,
      `allocate_exact_outside_the_bitmaps_coverage_still_reports_not_covered`)
      so the rename is proven to distinguish the two cases rather than
      just relabel every refusal.
- [x] **Create/delete/rename** in existing volumes: hash-chain
      insertion/removal under both fold tables, dircache invalidation
      (`DOS\4`/`5`: update or clear, never leave stale). `Mutator` in
      `src/mutate.rs` — a session holding a `Volume` and an `Allocator`,
      because reading a 2 GB volume's bitmap once per created file would
      dominate everything else, and holding nothing *but* those two,
      because a cached anything is a second copy of the volume to keep in
      step. Every public operation leaves the volume consistent, so there
      is no `finish()` to forget: that is affordable here and not in
      `Populator` precisely because nothing in this module ever lets the
      bitmap say "free" about a block something reaches, so the flag stays
      −1 throughout.

      **Block assembly moved to a shared module** (`src/build.rs`,
      crate-private): the header block's two name layouts, the `T_COMMENT`
      block, the OFS/FFS data block, the dircache block and its records.
      `Populator` was refactored onto it rather than copied from — a
      second implementation of "where does an LNFS comment go" is a second
      chance to put it at the classic offset, which is the mistake this
      crate exists to not make.

      **2026-09-09, as-built — `T_LIST` closed the same gap, found by
      independent review.** `build.rs`'s own module doc states the
      principle — what a header, comment, data or dircache block
      contains must exist once — but the extension block was the one
      block type left out of it, hand-assembled in five places:
      `mutate.rs`'s `create_file` and `edit_file`, `compact.rs`'s
      `defragment_file_avoiding` and `relocate_header_core`, and
      `populate.rs`'s `create_file_with`. All five wrote byte-identical
      blocks before this — confirmed field by field, not assumed — so
      this was a pure consolidation with no discrepancy to reconcile.
      `build_extension_block(buf, lba, header, pointers: &[u32], next)`
      now owns the six fixed longwords, the table and the checksum;
      the four batch writers (which already know a file's whole block
      count) hand it a complete slice, and `Populator::create_file_with`
      — which discovers data blocks one at a time from a streaming
      callback and has to write forward, patching a predecessor's
      pointer once its successor's LBA is known, rather than backwards
      like the other four — now accumulates a `Vec<u32>` of pointers
      per extension block instead of writing bytes directly, and calls
      the shared function once each block is complete, at exactly the
      point it used to call `put_buf_checked`. Write timing, order and
      byte content are unchanged; `put_buf_checked` became dead code
      and was removed. Verified against the full differential suite
      (both the xdftool and fstool legs, 21 tests, all green) as well
      as the whole test suite on both feature sets, not just the unit
      tests touching the five call sites.

      Four decisions worth stating.

      **Deleting the target of a hard link is refused**
      (`MutateError::LinkedTo`), and the reason is that the two shipping
      implementations produce *different volumes*. AmigaOS promotes — its
      own documentation: "if the object a hard link points to is deleted,
      then the first hard link in the chain is altered so that it becomes
      the new file header block. The original file header block is then
      freed" — so the object's header block changes number, invalidating
      every pointer to it including its children's `parent` longwords.
      Linux's `affs` does the opposite and says why: "we can't remove the
      head of the link, as its blocknr is still used as ino, so we remove
      the block of the first link instead" (`affs_remove_link`), so it
      keeps the original block, copies the *link's* name into it,
      re-inserts it into the **link's** directory and frees the link — the
      file survives under another name in another directory. ADFlib
      refuses links outright, amitools has no link code at all, and AROS's
      `afs.handler` defines `BLK_ORIGINAL`/`BLK_LINKCHAIN` and never reads
      them, so its delete leaves the links dangling. Either shipping
      behaviour rewrites several blocks with no ordering that makes an
      interruption harmless. Refusing keeps the choice open and rules out
      the one outcome nobody wants; deleting the links first and then the
      target works today, and is what the refusal points a caller at.
      Deleting a *link* is implemented: it is spliced out of its target's
      −10 chain while still reachable from its directory, then unlinked,
      then freed.

      **Dircaches are regenerated, not patched.** The affected directory's
      whole chain is rebuilt from its hash chains after every create,
      delete, rename and metadata change, reusing blocks where the chain
      is no longer than before, allocating where it grew and freeing where
      it shrank. Surgical editing would be less I/O and one more place to
      leave the cache disagreeing with the chains; regeneration cannot,
      because the chains are the input. The chain is written *backwards*,
      last block first, so a `next` pointer never names a block that is
      not there yet. Every mutation test asserts zero `DircacheStale`
      findings afterwards.

      **The parent's date is stamped — when the caller supplies a clock.**
      The question was settled by reading four implementations, which do
      not agree. amitools' `xdftool` stamps the parent's longword −23 *and*
      the root's −10 on every create and delete (`ADFSDir._create_node`
      and `_delete` both end in `update_dir_mod_time()` +
      `volume.update_disk_time()`). Linux's `affs` stamps the parent
      (`affs_insert_hash`/`affs_remove_hash` both mark the directory inode
      dirty, which `affs_write_inode` writes to `change`/`root_change`,
      −23) but writes the root's −10 only from `affs_commit_super`, on
      sync and unmount. ADFlib stamps the parent in `adfCreateEntry` only
      when the entry becomes a slot's head and never in `adfRemoveEntry`.
      AROS stamps nothing on create or delete and every ancestor up to the
      root on a *write*. This crate follows amitools, the closest thing to
      a real-Amiga reference among them — but it is `no_std` and has no
      clock, so `Mutator::clock` supplies "now" and **until it is set no
      date is written at all**: a wrong date on the disk is worse than an
      old true one.

      **A rename unlinks before it relinks.** The entry is unreachable in
      between — a leak, recoverable — where the other order would put one
      block in two chains with one `hash_chain` longword to serve both.
      The new slot's head is read *after* the unlink, because for a
      same-slot rename (which is what a change of case is) the unlink is
      exactly what changed it. Renaming a directory into its own subtree
      is refused by walking `parent` pointers upward with a cycle guard,
      since the volume may already contain the loop.

      Tested by: creates into an existing tree on every variant ×
      {512, 4096} with a file that crosses an extension block; head/middle
      /tail chain splicing; the refusals (non-empty directory, duplicate,
      linked-to, own subtree, bad names, the root); case-only and Latin-1
      renames under both fold tables; LNFS renames crossing the merged
      field's capacity in both directions, with the comment moving
      inline↔overflow and the block count moving with it; `set_metadata`
      on every variant; dircache agreement after 30 creates, 25 deletes, a
      rename and a metadata change at two block sizes; **a create and its
      delete returning the volume to byte-identical block accounting**; a
      seeded random interleave of all five operations against a
      `BTreeMap` model on all eight variants × {512, 4096}, compared field
      by field and byte by byte after every batch and emptied back down to
      the blocks a fresh format had; a crash sweep (below); and a
      differential leg where xdftool lists a mutated volume, reads every
      byte out of it, writes into the bitmap this crate edited in place —
      and where the same operation sequence run through both
      implementations produces the same tree, the same file bytes and the
      same used-block count.

      **2026-09-11, as-built — a speculative allocation could fail an
      operation that needed no space at all, found by independent
      review.** `refresh_dircache`'s "light-touch directory pull" (the
      `if self.layout_policy` block a few paragraphs above, trying to
      relocate a *kept* dircache block nearer to its directory) called
      `Allocator::allocate_for` behind `?`. That call is purely
      optimistic — a refusal was already handled correctly one line
      later, by declining the candidate and moving on, the same as
      "nothing nearer was found" — but `?` still propagated a *different*
      failure, most plausibly `AllocError::VolumeFull`, straight out of
      the whole operation. Since `refresh_dircache` runs on every
      `unlink`, `delete`, `rename` and `set_metadata` (not only `create_*`,
      which legitimately needs space), the effect was that deleting an
      entry from a directory with a two-page-or-larger dircache on a
      volume with zero free blocks failed with `VolumeFull` — even though
      the delete needed no new space and would have freed some. Fixed by
      matching on the trial allocation's `Result` instead of using `?`:
      any error there is now treated exactly like "no improvement found"
      (`continue` to the next candidate), and only the genuinely-needed
      allocations later in the same function (the `fresh` loop, for a
      dircache that grew) still propagate a real `VolumeFull`. Regression:
      `deleting_from_a_full_volume_does_not_spuriously_fail_with_volumefull`
      (`tests/volumes.rs`) — a `DOS\4` volume with 25 files directly in the
      root (forcing a two-page root dircache), filled solid by creating
      files until the allocator has genuinely nothing left, then a
      `delete` of one of the original 25 confirmed to fail before this fix
      and succeed after it, with the volume validating clean afterwards.
- [x] **File write/append/truncate**: FFS chains and OFS headers both.
      `Mutator::write_file`, `Mutator::append` and `Mutator::truncate` in
      `src/mutate.rs`, over one private `edit_file` — three ways of
      choosing an offset, a slice and a resulting length, so the block
      arithmetic and the OFS bookkeeping exist once.

      **The file header block is the single commit.** Every size change
      ends in exactly one write, carrying `byte_size`, `high_seq`, the
      whole data-pointer table and the extension pointer together; before
      it the file is entirely the old one, after it entirely the new one.
      That is forced rather than chosen: `file_chain` refuses a header
      whose length and extent disagree *in both directions*, so any order
      moving one before the other leaves a file that will not read, and
      `byte_size` lives in the header.

      Two consequences follow, and they are the wave's two real decisions.
      **The extension chain is rebuilt, never patched** — a `T_LIST` block
      is reachable only through the block before it back to the header, so
      editing one in place would commit a new block count from a block
      that is not the header. A size change therefore allocates a fresh
      chain, writes it last-block-first, points the header at it and frees
      the old blocks afterwards; it costs one block per ~35 KB of OFS file
      per size change, which is the price of the commit being one write,
      and is why every entry point takes a whole slice. (The same argument
      that made dircaches regenerate rather than patch, one chain over.)
      **A data block whose *recorded* length changes is replaced, not
      edited** — on OFS only, where the length is a field (longword 3): a
      grow makes the old last block full, a shrink makes some earlier
      block short, and copy-on-writing it keeps the header the only
      commit. FFS records no such length, so its boundary block is written
      in place and the bytes it gains past the old end are unreachable
      until the commit adds them.

      **Writing past the end zero-fills**, and the evidence says that is a
      choice rather than a default. AmigaDOS cannot even reach the state
      by seeking: "You cannot Seek() beyond the end of a file"
      (`dos.library/Seek`), and `ACTION_SEEK` "shall fail with the error
      code `ERROR_SEEK_ERROR`" past EOF, leaving the pointer unaltered —
      confirmed structurally in AROS's `afs.handler`, whose `seek()`
      initialises `ERROR_SEEK_ERROR` and never clears it when the chain
      runs out. The only route is `SetFileSize()`, whose autodoc says "if
      the file is extended, no values should be assumed for the new bytes"
      and whose packet specification says outright that "unlike other
      operating systems, AmigaDOS does not enforce zero-initialization of
      the extended region". Both permitted answers ship: AROS grows via
      `writeData` with a NULL buffer, which on FFS does not write the new
      block at all and leaves whatever the disk held; Linux's `affs`
      zeroes, through `cont_expand_zero` and
      `affs_extent_file_ofs`/`affs_getzeroblk`. This crate zeroes — not
      for tidiness but because the bytes a freshly allocated block holds
      are a *deleted file's*, and handing them back through a new file's
      length is an information leak the specification permits and nobody
      wants. Sparse is not on the table at all: the pointer table is dense
      by construction, data block *n* being the *n*th slot counted through
      the chain with 0 as the terminator, so the format has no hole to
      leave.

      Tested by: the write/append/truncate matrix on OFS, FFS, a dircache
      variant and a long-name variant at 512 and 4096 — appends crossing
      the extension boundary, overwrites spanning
      partial-first/full-middle/partial-last, truncates to 0, to mid-block,
      to an extension boundary and upward, and a write past the end whose
      gap is asserted to be zeroes on a volume pre-filled with `0xA5`;
      every step followed by a byte-identical readback against a `Vec<u8>`
      model and a clean `validate()`; a seeded random interleave of the
      three operations with offsets and lengths drawn *around* the two
      boundaries where the arithmetic differs; the crash sweeps below; and
      a differential leg where xdftool reads back every byte of a file
      this crate appended to, overwrote, truncated and grew, and where the
      same end state reached by a delete-and-rewrite there and an
      append-and-overwrite here yields the same tree, the same bytes and
      the same used-block count.
- [x] **Ranged reads** (`read_range(&chain, offset, buf)`): the FUSE-shaped
      gap in the read surface, and Copperline's too — a trackdisk-level
      consumer never wants whole files. `Volume::read_range` in
      `src/file.rs`, landed with wave 3 because both live in the chain
      machinery.

      It walks **only the blocks the range covers** — the first is
      `offset / payload_size`, arithmetic rather than a walk — and
      verifies the OFS headers of exactly those, since a block's expected
      sequence number falls out of its index and not out of having reached
      it from block 1. The verification is the *same* function the
      streaming read uses, because two copies would be two chances for one
      of them to stop checking the sequence number. Clamping is `read(2)`'s:
      `byte_size` is the authority for where the file stops (the last
      block has capacity past it and returning that would invent data), an
      offset at or past the end returns `Ok(0)` rather than an error, and
      a short count is how a partial fill reports itself. Tested against
      the whole-file read on both filesystems at both block sizes over
      twelve range shapes, with the block *reads* counted to prove the
      range really is a range, and with a corrupted OFS block that a range
      stopping short of it still reads past and a range touching it
      refuses by name.

      `FileChain` was split from the streaming read precisely so it could
      be collected once and reused; a pre-collected chain is also
      immutable data a concurrent consumer can hold outside its `Volume`
      lock, which keeps the *read* critical section at one block read.
      Read is the only path with that property: `lookup` and `readdir`
      walk hash chains, so their sections are chain-length long — fine in
      practice, and fine *because* the adapter's block cache absorbs the
      walk, which is the cache earning its place rather than an
      optimisation. Concurrency itself stays the consumer's: `&mut self`
      was deliberate, a FUSE adapter wraps the volume in a `Mutex` and
      brings the block cache its own medium warrants, since an LRU sized
      for a local image is wrong for a ZuluSCSI card over USB and that is
      exactly why the format crate cannot choose it.

      The mount-time story for such an adapter, recorded here because it
      falls straight out of surfaces the crate already has: `validate()`
      walks the whole volume — seconds on an image file, rather longer
      through a USB-attached card — so it is a mount *option*, defaulted
      by mode: always before exposing writes, opt-in for read-only. A
      damaged volume still mounts read-only with everything reachable
      (the walk records and continues by design, and read-only is when
      someone most wants the driver); refuse-or-repair gates only the
      write side. `bitmap_flag == 0` is the cheap early signal that a full
      walk is warranted before anyone pays for one. And a mount that came
      up read-only *because* validation found something must say so
      distinctly from one the user asked to be read-only: `EROFS` on the
      first write is technically correct and tells them nothing, while a
      mount-time log naming the findings — and the repair option that
      would clear them — turns "the driver refused" into "the driver
      protected the image". Which is the whole point of `Report` being
      typed findings rather than a boolean.

      The deletion surface such an adapter should call is
      `Mutator::unlink`/`Mutator::release` (below, "the create-then-unlink
      split"), not `Mutator::delete`: FUSE's `unlink`/`rmdir` callback maps
      onto `unlink` alone, and the actual block freeing happens later, on
      the `forget`/last-`release` callback for whichever handle(s) were
      still open — the adapter's own open-handle refcount is what decides
      *when* to call `Mutator::release` on the LBA `unlink` returned, this
      crate only guarantees that doing so late is safe and that not doing
      it yet leaks nothing worse than an `OrphanBlock` a later `repair` or
      `release` clears.
- [x] **Crash-shape discipline**: data blocks before metadata, chain
      pointers flipped last, bitmap updated in an order that at worst
      *leaks* blocks (validator-recoverable) rather than double-uses
      them. The format has no journal; ordering is all there is.

      Every write path in the crate is swept: `tests/volumes.rs` stops the
      medium after each
      successive write of an allocator session (wave 1), of a repair
      (wave 1) and of a mutation session that creates a directory, creates
      a file with an overflowing comment, renames and deletes (wave 2), on
      a plain, a dircache and a long-name variant. The damage is always
      leak-shaped: `OrphanBlock` is allowed, `ReachableButFree` never
      appears, every entry still reachable still reads, and — the one
      addition this wave needed — `DircacheStale` is allowed on the two
      variants that have caches and nowhere else, because a cache is
      advisory, a half-written one is what every dircache-unaware tool
      leaves behind, and `validate()` saying so is the mechanism by which
      it gets rebuilt.

      Wave 3 added the sweep the box was held open for — **file write,
      append and truncate** — and it comes out in two pieces, because the
      two halves of the operation have different shapes.

      An **overwrite that changes no length changes no metadata**: the
      block count, `byte_size`, the pointer tables and the bitmap are all
      untouched, so every prefix of it validates with *zero* findings and
      the file is the right length made of some old blocks and some new
      ones. That is asserted directly — each block compared against the
      old bytes *and* the new bytes and required to be one of them. It is
      also the one place this milestone's "old volume, new volume, or old
      volume plus leaks" invariant weakens, and it weakens to exactly
      **old bytes or new bytes, independently per block**. The weakening
      is inherent to overwriting in place rather than a shortcut: the
      alternative is copy-on-writing every touched block, which turns
      every overwrite into a reallocation and makes a file's blocks
      migrate across the volume for a guarantee the format cannot express
      anyway. Nothing structural is at risk in it, and `ReachableButFree`
      remains impossible.

      A **size change** is swept the same way the entry operations are, on
      a plain, a dircache and a long-name variant and on both filesystems:
      every prefix of a session that appends across an extension-block
      boundary, overwrites through the middle, truncates down and grows
      again. `OrphanBlock` is allowed and `DircacheStale` on the two
      variants that have caches; `ReachableButFree` never appears, no
      reachable block is corrupt, and — the assertion this wave is really
      about — *every file still reachable still reads every one of its
      bytes*, which is the header-block-as-single-commit rule saying that
      a file's length and its extent are never caught disagreeing.
- [x] **Differential mutation tests**: same operation sequence applied
      through this crate and through the guest's own filesystem on a
      copy; resulting volumes must agree (allowing documented
      don't-care fields — dates, allocation order).

      **Partly landed** — the oracle leg is done and the guest leg is
      not. `tests/differential.rs` runs the same sequence (makedir, two
      writes, a delete, a write) through xdftool and through `Mutator`,
      and the resulting trees agree entry for entry, byte for byte, and
      on the used-block count; separately, xdftool lists, reads and
      *writes into* a volume this crate mutated. Wave 3 added the same
      exercise for file *contents*: xdftool reads back every byte of a
      file this crate appended to across an extension-block boundary,
      overwrote through the middle, truncated and grew — the one check the
      model tests cannot make, since a writer and a reader that agreed on
      the same wrong chain arithmetic would satisfy both — and the same
      end state reached by a delete-and-rewrite there and an
      append-and-overwrite here produces the same tree, the same bytes and
      the same used-block count. The don't-cares are the
      two predicted ones and no others: allocation order (this crate
      scans forward from a hint, xdftool from the bottom of the bitmap)
      and dates (the caller's to supply here, the wall clock's there).
      **The guest leg is done.** A deterministic Copperline run boots
      Kickstart 3.1 + Workbench 3.1 with a writable copy of a volume
      this crate built in df1 (`write_protected = false` — inserted
      images default protected, and the guest says so in a requester),
      and an AmigaShell script performs makedir, copy, rename, a
      delete that frees an extension chain, and a fresh-file write
      through the ROM's own FFS. Two closures follow: this crate reads
      the guest-mutated image back with `validate()` clean — the ROM's
      allocator, renamer and freer produce a volume we certify — and
      `examples/guest-replay.rs` replays the identical five operations
      through `Mutator` on the pristine copy and compares the two
      volumes logically: tree, kinds, sizes and content hashes agree
      entry for entry, with dates and block placement the two
      documented don't-cares. Not in `cargo test` — it needs a ROM and
      an emulator — but reproducible from the two examples plus the
      scripted invocation, deterministically, which is what Copperline
      is for. The AROS `afs.handler` variant of the same exercise
      remains under the milestone-1 differential box with the amibake
      fixture pipeline.
- [x] **Validator repair**: the write-side half of `validate()`, doing
      what the ROM disk-validator does. `Volume::repair()` in
      `src/repair.rs`, and the first consumer of the allocator.

      The walk is *the same walk*: `validate()`'s tree pass was factored
      out as `walk_reachable()` and both call it, because the bitmap a
      repair writes is **defined** as the set that walk returns, and a
      second implementation of it would be a second answer. The bitmap is
      then rebuilt as the **union** of that walk and the old bits, the
      pages written fresh, the extension blocks and the root's pointers
      rewritten, and `bm_flag` stamped −1 in a write of its own, last —
      with `bm_flag = 0` written *first*, so every intermediate state is
      the honest "my bitmap is mid-update" one. Pages and extension blocks
      whose pointers name nothing usable (zero, out of range, a duplicate,
      or a block the tree itself is using) are replaced with freshly
      allocated ones; a page whose checksum does not balance is rebuilt
      from the walk alone and *said so* (`Action::PageUnreadable`), since
      that is the one place allocation cannot be preserved.

      **Repair only ever adds allocation and only ever removes
      reachability**, and both halves are asserted by the tests. The
      union is why: an orphan stays allocated (`Action::LeakKept`) rather
      than being freed the way the ROM validator frees it, because
      freeing a block the walk did not reach is only safe if the walk was
      complete — and a walk over a damaged volume is exactly where it is
      not, since one unreadable directory block hides its whole subtree
      and every file in it looks like a leak. The same reasoning is why an
      invalid bitmap is still *read*: its bits cannot be believed when
      they say "free", but a bit saying "allocated" is either true or a
      leak, and taking it at its word is conservative either way.
      Severing is opt-in (`RepairOptions { sever }`, default off) and
      narrow: only what the ordinary reader has *proved* it cannot
      follow — a header block that will not parse, a chain that revisits a
      block, a comment pointer whose block will not read as this entry's
      comment — cut by writing 0 into the slot or the previous entry's
      chain longword. The entries behind the cut stay on the disk and stay
      allocated: leaked, not freed. `repair()` returns a report of typed
      `Action`s mirroring the `Finding`s they answer.

      Tested against each damage separately — a cleared bit
      (`ReachableButFree` → allocated, and the volume back to byte-exactly
      what it was), a stale bit (`OrphanBlock` kept, invariant asserted),
      `bm_flag` zeroed, a page whose checksum is gone, a page pointer
      clobbered three ways, and a scribbled header with `sever` on and off
      — plus the whole matrix of eight variants × {512, 1024, 4096}, a
      crash sweep over the repair's own writes, and a differential leg
      where xdftool lists, reads every byte of, and *writes into* a volume
      this crate repaired. A prerequisite for resize, which must rebuild
      the bitmap rather than hand FFS an invalid flag the way AmiPart
      does.

      **2026-09-09, as-built — the same later independent-review pass as
      above, this time in `sever`.** This module's own documentation says
      severing may
      cut only "what the ordinary reader has *proved* it cannot follow"
      — but the code routed every refusal `entry_at`/`comment` could
      return through `.ok()`, so `Error::Io` (a transport-level read
      failure: a flaky sector, a transient bus error) was treated as the
      same proof as `Error::Checksum` or `Error::NotHeader` (the reader
      having actually looked at the block's bytes and found them wrong).
      An `Io` error proves nothing about the block's content — only that
      this one attempt to read it failed — so severing on it cuts a
      chain, or clears a comment pointer, that may be perfectly intact.
      Confirmed with a failing test
      (`sever_does_not_cut_on_a_transient_io_error`, using a new
      `MemDisk::fail_read_once` that fails exactly one read of a chosen
      block and then behaves normally, the read-side counterpart to the
      existing `fail_after` write-crash injector): a genuine three-entry
      hash chain, its middle link's block perfectly good, one glitched
      read of it — before the fix, `repair(&RepairOptions{sever: true,
      ..})` truncated the chain there and leaked the tail behind it, for
      a block with nothing wrong with it. Fixed by distinguishing
      `Error::Io` from every other refusal at both sever sites (the
      chain walk and the comment-pointer check): an `Io` error now cuts
      nothing, pushes a new `Action::Unverified { lba }` instead of
      `ChainTruncated`/`CommentPointerCleared`, and the walk moves on
      leaving the pointer exactly as it was — matching this module's own
      stated rule rather than a superset of it. Every other error
      (`Checksum`, `NotHeader`, `UnknownSecondaryType`, and the existing
      cycle/out-of-range checks) still severs exactly as before.
- [x] **`unlink`/`release`: the create-then-unlink split** (2026-09-08,
      after M3 itself was landed — a FUSE-adapter requirement surfaced
      while designing that consumer, not something the original wave
      anticipated). `Mutator::delete` frees a block the instant its
      directory entry disappears, which is wrong for a FUSE mount: POSIX
      programs unlink a file while still holding it open — a temporary
      file that cleans itself up if the program dies, a rename-over-open
      editor save — and keep reading and writing through the handle they
      already have. The kernel doesn't tell a filesystem "the last handle
      closed" at the moment of `unlink()`; it tells it later, in a
      separate call, however much later that is. A crate whose only
      deletion primitive frees blocks synchronously with the directory
      change gives a FUSE adapter no seam to hold that gap open at.

      `Mutator::unlink(parent, name) -> Result<u64, _>` is
      `delete`'s exact metadata half — hash-chain and link-chain splice,
      dircache regeneration, date stamp, every refusal `delete` already
      had (`DirectoryNotEmpty`, `LinkedTo`, an unreadable file chain) —
      stopping short of clearing a single bitmap bit, and returning the
      header block's LBA as the handle a caller holds until every reader
      is done with it. `Mutator::release(header_lba) -> Result<u64, _>`
      is the other half: free the header, its extension blocks, its data
      blocks and its comment block. `delete` is now defined as `unlink`
      immediately followed by `release` on the LBA it returns — one
      implementation of each half, not three. This was a **factoring, not
      new discipline**: the module's write-order rule 3 ("bitmap last
      when freeing") already put the unlink before the free in `delete`'s
      own body; splitting the function at that exact seam was the whole
      change.

      `release` does not trust that a caller only ever offers it a header
      `unlink` actually produced. Before freeing anything it verifies
      reachability itself — hashes the header's own recorded name and
      walks *that one chain* in the header's own recorded parent, refusing
      with a new typed `MutateError::StillLinked` if the header is still
      named by it. That is the cheap end of "prove it is unreachable"
      (one chain) rather than the thorough end (a whole `validate()` walk)
      — deliberately: it catches exactly the mistake `release`'s contract
      has to guard against (a header offered that was never unlinked),
      for the cost of a lookup rather than a tree walk. `release` also
      refuses the root (`IsRoot`) and anything that does not parse as a
      file or directory header at all — a data block, a `T_LIST`
      extension block, a comment block — via the same typed refusal
      `Volume::entry_at` already gives every other caller. A second
      `release` of an already-released header is not given its own error:
      it surfaces as the allocator's existing `AllocError::DoubleFree`,
      the same shape a retried `delete` already takes — by the time the
      reachability check has passed, "is this block already free" is the
      allocator's question to answer, not a second implementation of it
      in `Mutator`.

      **The state in between `unlink` and `release` is not a transient
      phase this module hides — it is ordinary, persistent on-disk data**,
      exactly as legal as the leak an interrupted `delete` already
      produces, because it *is* that leak, held open on purpose instead of
      by accident. Three things about it worth stating precisely, since a
      FUSE adapter has to reason about all three:

      - It survives a restart. Nothing marks it "in progress" anywhere
        that isn't already on the disk; a `Mutator` opened fresh in a
        later process sees the same allocated-but-unreachable chain and
        can `release` it just the same.
      - `validate()` classifies it as exactly `Finding::OrphanBlock`, one
        per block owned, and nothing else — the identical finding an
        interrupted `delete` produces, for the identical reason (the walk
        from the root does not reach it). Pinned directly:
        `unlink_leaves_an_orphaned_chain_that_still_reads` in
        `tests/volumes.rs` asserts the finding set is *exactly* that set
        and nothing more, that every other file on the volume is
        untouched, and — the actual point — that the unlinked file's
        content still reads correctly by the LBA `unlink` returned,
        through `entry_at`/`read_file`, which is what a still-open FUSE
        handle needs.
      - `repair()` does **not** release it. `repair`'s bitmap rebuild is
        the *union* of the reachability walk and the bitmap's existing
        bits (its own module documentation): an orphan is already marked
        allocated, the walk does not reach it to add anything, and the
        union leaves it exactly as it was. That is the correct
        conservative direction — a walk over a genuinely *damaged* volume
        might miss a subtree that is not actually a leak, so `repair`
        never frees anything the walk failed to reach — but the honest
        consequence is that a crash (or a plain process exit) between
        `unlink` and `release` leaves those blocks leaked until something
        calls `release` on that header LBA specifically. `repair`'s job is
        making the bitmap agree with the volume, not deciding which leaks
        are safe to reclaim, and from inside this crate an
        unlinked-but-still-open handle is indistinguishable from every
        other leak it is conservative about.

      **`delete()` keeps its exact existing semantics** — same refusals,
      same write order, same return value — as the composed pair; every
      pre-existing `delete` test passes unchanged, which is the
      confirmation the refactor is a factoring and not a behaviour change.

      Tested in `tests/volumes.rs`: `unlink_then_release_equals_delete`
      (same final bitmap, byte for byte, as plain `delete`, on every
      variant × {512, 4096}); `unlink_leaves_an_orphaned_chain_that_still_
      reads` (the intermediate-state properties above, pinned directly);
      `release_refuses_a_still_linked_header` and
      `release_refuses_a_block_that_is_not_a_header` (the two typed
      refusals); `releasing_twice_refuses_as_a_double_free` (the chosen,
      documented shape for a double release); crash sweeps over `unlink`
      alone and `release` alone
      (`an_interrupted_unlink_leaks_and_never_double_allocates`,
      `an_interrupted_release_leaks_and_never_double_allocates`), pinning
      the two halves' own write-prefix safety now that they are reachable
      independently rather than only as `delete`'s interior; and a
      property-test extension
      (`a_random_interleave_mixes_unlink_and_deferred_release_with_a_model`)
      that replaces some of the seeded interleave's deletes with an
      `unlink` now and a `release` a batch later, the model tracking both
      "in the tree" and "unlinked, pending release" states, checking the
      pending set's content stays readable and the volume's only findings
      are the expected orphans at every batch boundary, and confirming a
      full drain (release everything pending, delete everything else)
      still lands back on a fresh format's exact block set.

      The FUSE adapter this was built for is still future work (the
      milestone-3 "Ranged reads" entry's mount-time notes are the design
      surface it will extend), but the primitive it needs — hold a leak
      open across an unknown gap, prove it safe to close later — did not
      want to wait for that adapter to exist before landing, since
      `delete`'s only alternative shape was baking synchronous freeing in
      deep enough that adding this later would have meant relitigating
      the write-order rules rather than factoring them.
- [x] **`Mutator::volume()` stopped handing out `&mut Volume<S>`**
      (2026-09-09, found by independent review). `Volume` has inherent
      `resize()` and `repair()`, both `&mut self`, both writing the
      on-disk bitmap directly — and `Mutator::volume()` handed back a
      plain `&mut Volume<S>`, so nothing stopped
      `mutator.volume().resize(n)` or `.repair(&opts)` compiling and
      running mid-session. `Mutator` caches an `Allocator` loaded once at
      `open()`; `resize`/`repair` rewriting the bitmap underneath it has
      no way to tell that cache it is now stale, so the session's next
      allocation would work from the wrong picture of the disk, and its
      own flush (every operation's last step) would write that stale
      picture back over whatever `resize`/`repair` had just written —
      capable of re-marking a still-reachable block free, the one state
      (`Finding::ReachableButFree`) this whole module exists to prevent.
      Every call site in this repo (grepped across `src/`, `tests/`,
      `examples/`) only ever used `.volume()` for reads — `lookup`,
      `read_dir`, `entry_at`, `read_file`, `read_file_with`, `read_range`,
      `file_chain`, `lookup_path`, `validate`, `read_softlink`,
      `resolve_link`, `comment`, `read_bitmap`, `read_dircache`,
      `root_lba`, `root`, `block_size`, `block_count`, `variant`,
      `max_name_len`, and `source_mut` (twice, both only to reach a test
      double's own instrumentation) — never `resize` or `repair`, so the
      fix could narrow the accessor instead of working around the
      problem. `Mutator::volume()` now returns `MutatorVolume<'_, S>`, a
      thin forwarding view exposing exactly that read-only surface and
      nothing else; every existing call site still compiled once
      unqualified `let vol = m.volume();` bindings picked up the `mut`
      an owned view (rather than a borrowed one) needs. Landed alongside
      the milestone-1 `guard_chain` fix below since both came out of the
      same review pass; not a semver break, since 0.3.0 has not shipped.

## Milestone 4 — resize

- [x] **In-place grow/shrink of an existing volume**:
      `Volume::resize(&mut self, new_block_count) -> Result<ResizeReport,
      ResizeError<E>>` and a read-only `Volume::minimum_size(&mut self)`,
      in `src/resize.rs`. The filesystem half only — the RDB's `high_cyl`
      move is amiga-rdb's, composed by the consumer; the module doc states
      the ordering rule for each direction (grow: enlarge the partition
      first, then call `resize`; shrink: call `resize` first, then shrink
      the partition — reasoned from "a volume must never be told it is
      bigger than the medium actually is").

      The algorithm shipped in AmiPart's `ffsresize.c` (John Hertell, MIT
      — ported and cited, not copied) confirmed the core fact this crate's
      own `canonical_root_lba` already encoded: FFS recomputes the root's
      LBA from geometry on every mount, so any size change **moves the
      root** to the new midpoint. Moving it means copying the root's
      content to the new LBA and re-parenting its *direct* children (their
      `parent` longword names the root; deeper entries and hard-link
      `real_entry` pointers are untouched, since no child header moves) —
      plus two more pointers this crate found by reading its own layout
      rather than by reading AmiPart, which does not have them: the root's
      own `DOS\4`/`DOS\5` dircache chain (each block's own `parent` field)
      and the boot block's advisory root pointer.

      Three places this goes further than AmiPart, as planned: **every**
      block size 512..=32768 rather than 512/1024 only; the bitmap is
      rebuilt immediately by reusing `Volume::repair`'s reachability walk
      (`walk_reachable`, shared rather than duplicated) instead of
      stamping `bm_flag = 0` for FFS to fix on the next mount; and
      `DOS\4`/`DOS\5` dircache chains plus the LNFS `NumBlocksUsed`/
      `FileSystemType` fields are kept correct in both directions, where
      AmiPart does not touch them at all.

      **What the plan did not anticipate**, learned by building it: a
      shrink's *bitmap page and extension block* relocation falls out of
      `repair`'s own tolerant reader for free (a pointer past the new end
      simply fails its usability check and gets replaced) — but freeing
      what becomes obsolete does not, because `repair`'s one-direction
      rule (never remove allocation an incomplete walk might have missed)
      would otherwise keep the old root and every excess bitmap page as
      permanent leaks. `resize` patches those bits directly into the
      still-intact old bitmap before handing off to `repair`, which is
      the one piece of bitmap surgery this module does itself rather than
      delegating. And the new root's own target block can — on a volume
      packed solid from `reserved` upward, which is exactly how this
      crate's own allocator fills a volume — land on a block real user
      data occupies; this operation refuses that case
      (`ResizeError::RootTargetOccupied`) rather than relocating arbitrary
      data to make room, which is a real, named limitation:
      `minimum_size()` reports what this engine will actually accept, not
      the theoretical floor a data-relocating resize could reach, and the
      two can differ by roughly 2× on a volume with no free gap. Engineered
      directly (a volume filled from `reserved` upward with one big file,
      grown by the smallest possible step so the new midpoint lands inside
      it) rather than only hit incidentally, once a deterministic
      construction was worked out.

      Ordering: the very first write is a full copy of the (still valid)
      root at the new LBA with `bitmap_flag` forced to 0 — after that one
      write, any mount at the new geometry finds a structurally valid,
      honestly-untrustworthy root, the same state an interrupted ordinary
      mutation leaves. Every later write only improves on that floor, down
      to `bitmap_flag = -1` last, out of `repair`.

      `repair` alone only ever rebuilds the bitmap, and calling it after an
      interrupted `resize` can leave `ParentMismatch`/`DircacheStale`
      findings it has no opinion on. What finishes the job — found while
      writing the shrink crash sweep below, in response to review feedback
      asking whether the gap was inherent — is calling `resize` **again
      with the same target**: every write past the first is idempotent, so
      a retry redoes only what an earlier call left undone rather than
      being a no-op itself, and the "nothing to do" fast path now checks
      `bitmap_flag` as well as the size before deciding there is truly
      nothing to redo. Two things a retry still cannot recover, both
      bounded to leaks rather than anything dangerous and both pinned by
      the shrink crash sweep rather than left as prose: the *very first*
      root position in a sequence of retries, once a later call's notion of
      "the old root" has already moved past it (stays allocated as an
      `OrphanBlock`); and the root's own dircache pointer, if a crash lands
      after the first write (which carries it over unchanged) but before it
      is updated, leaving it unreadable through any bound a retry still has
      a use for — the pointer is cleared rather than the whole retry
      failing, `DircacheStale` reports it, and any later `Mutator`
      operation on the root regenerates it from the hash chains, which are
      never at risk. What was genuinely inherent (turning the documented
      gap into a solved problem, per the review's framing) was solved;
      what remained after that is two narrowly-scoped, safe-direction
      losses of bookkeeping, not of data — and now a test asserts exactly
      that shape rather than a wider "trust me".

      Tested in `tests/volumes.rs`: grow-then-shrink round trip on every
      variant × {512, 1024, 4096} with byte-identical files and clean
      `validate()`; the no-op case; crossing the 25-bitmap-pointer
      boundary into an extension block and back; shrink refusal naming
      the offending block with `minimum_size()` agreeing exactly at the
      boundary; relocating a multi-block root dircache chain out from
      under a shrink's cut; an 8-step seeded random grow/shrink sequence
      checked against the tree after every step; `RootTargetOccupied`
      engineered directly; and two crash sweeps, one per direction, each
      over every write prefix, asserting `reachable_but_free` is always
      zero once the bitmap claims validity, and — from the same crashed
      state — that `repair` alone leaves only the bitmap-fixed, findings
      it cannot address named and bounded, *and* that a same-target
      `resize` retry finishes the reparenting in full (checked down to
      every direct child's own parent longword) modulo the two named,
      pinned exceptions above. The shrink sweep is the one review asked
      for by name, since shrink is the direction that relocates metadata
      and issues explicit frees — exactly where a double allocation would
      come from if the ordering were wrong — and the grow sweep alone
      never exercises those paths.

      **Left undone**: an oracle leg in `tests/differential.rs` —
      `xdftool` only opens a fixed 1760-block ADF or an
      explicitly-geometried HDF and neither it nor `fstool` implements a
      resize to cross-check against, so the achievable differential value
      (an oracle re-reading a post-resize image) was judged not worth the
      harness work in the time available, and is recorded here rather than
      silently skipped; and a property test driving `resize` interleaved
      with `Mutator` operations (only pure resize sequences are
      seeded-random-tested today).

      **2026-09-09, as-built — two bugs found by independent review,
      both fixed:**

      - **`resize()` could corrupt its own newly-written root.**
        `free_bit_on_disk`'s job is patching a freed block's bit directly
        into whichever *old* bitmap page the pre-resize bitmap says covers
        it — necessary, per this entry's own "what the plan did not
        anticipate" note above, because `repair`'s one-direction rule
        would otherwise keep the old root and excess bitmap pages as
        permanent leaks. The bug: that lookup trusts the *old* bitmap's
        idea of where its pages are, with no check for whether this same
        `resize` call has since **repurposed** that LBA for something
        else — the new root, or a relocated dircache block. On a stock
        1760-block DD floppy, growing to 1761 blocks moves the root from
        880 to 881, which is exactly where the old bitmap's one page
        lived; `root_safe` correctly allows the new root to land there
        (`repair()` rebuilds that page from the reachability walk
        regardless), but `free_bit_on_disk`, run afterward to free the
        *old* root's bit, still resolved page index 0 to LBA 881 — now the
        freshly written root — read it back as if it were a bitmap page,
        OR'd a bit into what it thought was a bitmap word, and stamped a
        bitmap-style checksum (longword 0) over the root's own `OFF_TYPE`
        field. Reproduced first as a failing test
        (`growing_a_floppy_by_one_block_does_not_corrupt_the_new_root`):
        `resize(1761)` returned
        `Alloc(Read(NotHeader { lba: 881, found: 4294950914 }))`, the
        negated-sum checksum sitting where `T_HEADER` should be. Fix:
        `free_bit_on_disk` now takes the same `claimed: &[u64]` set
        `resize` already tracks (the new root plus every dircache block's
        final LBA) and skips any page whose LBA is in it, on the same
        reasoning the function already documented for a page past
        `new_block_count` — a page that has been repurposed by this same
        call has nothing here worth patching either, because `repair`'s
        walk rebuilds it wholesale regardless of what is sitting in the
        stale page reference. Confirmed general rather than
        floppy-specific: a second test
        (`growing_a_1024_byte_ffs_volume_by_one_block_does_not_corrupt_the_new_root`)
        reproduces the identical collision on a 1024-byte-block `Ffs`
        volume, because the root-adjacent bitmap-page placement is this
        crate's own formatter's layout choice (see this module's "Where
        everything goes" documentation), not an artefact of one geometry
        — a scan across variants, block sizes and volume sizes turned up
        the same shape wherever a grow shifts the midpoint by exactly the
        page's offset from the root, on every block size and FFS/OFS
        variant tried.
      - **The boot-block checksum was not recomputed after a resize.**
        `resize()` patches the boot block's advisory root-LBA longword
        but, before this fix, never touched the checksum that longword
        sits inside of. `format.rs`'s own documentation states plainly why
        a zero checksum there is deliberate (it is what makes a `Format`-
        produced boot block *non-bootable*, and a checksum that balances
        over 1012 zero bytes of "code" is strictly worse than one that
        fails) — but a boot block a caller made bootable
        (`FormatOptions::boot_checksum: true`, or a real `Install`) has a
        checksum that balances over the *old* root pointer, and silently
        stopped balancing the moment `resize` patched the pointer without
        touching the checksum, turning a bootable image non-bootable with
        no error anywhere. Fix: after patching the pointer, `resize` now
        gathers the full 1024-byte boot area (`format.rs`'s own
        `BOOT_AREA_LEN` — two blocks at 512-byte block sizes, one at 1 KB
        and up, the same span `format()` itself lays down), checks whether
        the checksum balanced *before* the edit by reusing
        `bootblock_checksum` itself rather than re-deriving the
        end-around-carry sum, and — only if it did — recomputes and
        rewrites it after. An unbalanced checksum (the ordinary, non-
        bootable case) is left exactly as it was, matching `format.rs`'s
        own stated reasoning rather than inventing a checksum where there
        was never a valid one. Tested both directions:
        `resize_recomputes_a_valid_boot_checksum_after_moving_the_root`
        (formats with `boot_checksum: true`, resizes across a 512-byte
        block boundary so the boot area spans both reserved blocks, and
        checks the checksum still balances over the new pointer) and
        `resize_does_not_invent_a_boot_checksum_on_an_ordinary_volume`
        (an ordinary volume's checksum longword stays zero through a
        resize that still updates the pointer).
- [x] **`ResizableMedium` and `Volume::resize_to`** (2026-09-16, an
      ergonomics pass, motivated by amidisk's `resize` command in
      sidick/amiga-tools). `Volume::resize` never extends the backing
      medium — it only ever writes blocks up to whatever size the medium
      already reports — and `Volume::open`/`Volume::open_with` fix a
      volume's `block_count` at open time, so a consumer growing an
      in-memory or file-backed image had to do three things by hand, in
      order: pre-extend the raw bytes with zeros, open at the *old*
      `block_count` over the now-larger buffer via `open_with` (`open`
      alone reads the medium's new, bigger size straight back as the
      volume's own — the wrong value for `resize` to treat as "before"),
      resize, then read back exactly `new_block_count` blocks into a
      fresh buffer to save, since `resize` never shrinks the medium
      either. Folklore living in every caller's own module doc rather
      than in this crate, and precisely the workaround amidisk's
      `tools/amidisk/src/commands/resize.rs` documents and implements.
      `Volume::resize` and `Volume::open`/`open_with` are otherwise
      exactly right for a medium sized by an outside authority (an RDB
      entry, a partition on a real device) — this crate must never be
      the one deciding how big a real device is — so the fix is not a
      redesign of either; it is a new opt-in seam for the other case, a
      medium that *is* its own storage. `ResizableMedium` in
      `src/resize.rs`, a `BlockSource` extension trait with one method,
      `set_block_count(&mut self, new_block_count) -> Result<(), Self::Error>`,
      documented to zero-fill on grow (the same stance this crate's
      write side already takes on a file grown past its old end) and to
      discard on shrink; `Volume::resize_to` composes it with the
      existing, unchanged `Volume::resize`: extend first when growing
      and only when the medium does not already report enough room,
      run `resize`, then truncate after a successful shrink. Every
      refusal `resize` itself can still make is unchanged — `resize_to`
      leaves the medium and the volume exactly where plain `resize`
      would on any error, since a grow's extension only ever adds unused
      capacity before `resize`'s own checks run and a shrink's
      truncation only happens after `resize` has already returned `Ok`.
      Tested in `tests/volumes.rs`: `MemDisk` (the test harness's
      in-memory `BlockSource`/`BlockSink`) gained a `ResizableMedium`
      impl, and `resize_to_grows_the_medium_before_growing_the_volume`
      /`resize_to_shrinks_the_medium_after_shrinking_the_volume` cover an
      ADF-sized floppy grown past a raw-byte-extension workaround would
      ever need to think about (past the HDF-sized end this milestone's
      other tests already exercise) and shrunk back, contents
      byte-identical and `validate()` clean throughout, with the
      medium's own reported block count asserted to match
      `new_block_count` exactly on both sides of the round trip — the
      one thing the hand-rolled workaround had to get right for itself
      that this call now guarantees. A third test
      (`resize_to_leaves_the_medium_untouched_on_a_refused_shrink`)
      pins the "leaves things exactly as plain `resize` would" claim
      directly: a shrink refused with `UserDataPastCut` leaves the
      medium's block count unchanged.

## Milestone 5 — block layout policy and compaction

**Landed.** All three waves: wave 1 — the survey (`docs/layout-survey.md`)
and creation-time policy, `Allocator`'s `Intent` vocabulary and
`allocate_run`, `Populator`'s two-cursor split; wave 2 — compaction,
`src/compact.rs`'s two tiers plus `make_room` and `resize_evacuating`;
wave 3 — passive reorganisation, `Mutator`'s own everyday writes adopting
`Intent` for their own placement. One subject, at two points in a
volume's life. The premise throughout: this crate's output is not only
emulator images. It writes filesystems that land on **real Amiga
hardware** — CF cards, real drives, real floppies — where a seek is a
head stepping across a platter and costs milliseconds, not an offset
into a host file costing nothing. Layout is therefore a durable
property of every image shipped, not a tuning detail.

  **Wave 1 landed**: `docs/layout-survey.md` (the survey this entry
  itself asked for, done first, plus a wave-1 addendum to §4a — see
  below) and policy-at-creation, item 1 below. `Allocator`
  (`src/allocator.rs`) gained an explicit `Intent` vocabulary
  (`DataFor`/`HeaderIn`/`MetadataNearRoot`/`Anywhere`), each reducing to a
  hint over the existing first-fit scan, and `allocate_run` for
  contiguous-extent reservation with a documented graceful-degradation
  fallback — machinery, not yet wired into `Mutator`'s own day-to-day
  writes. `Populator` (`src/populate.rs`) now allocates from two forward
  cursors instead of one: a metadata cursor starting next to the root
  (directory and file headers, dircache blocks, comment overflow — what
  a directory walk touches once, at open or during the walk) and a data
  cursor over the volume's other half (file content, *and* `T_LIST`
  extension blocks interleaved at their natural position in the write
  order). The extension-block placement was not a first guess: an
  earlier version of this wave put them with the header, on the
  (wrong) theory that an index structure is metadata; a real-ROM
  measurement (`examples/frag-bench.rs`, same rig as §4a) showed that
  cost 21 s against 19 s for interleaved, because an extension block is
  fetched mid-stream by a reader already reading the file, not once at
  open the way a header is — pulling it to the root cluster inserts two
  long seeks per extension boundary instead of zero. Interleaved is what
  shipped. The net effect: a large file's data-and-extension sequence is
  one contiguous physical run instead of several — measured on
  `examples/frag-bench.rs`'s 391-block file, both through the run-count
  metric (six runs before, one after, counted over the full fetch-order
  sequence rather than data pointers alone) and through the real-ROM
  timing already on record in §4a. `format()`'s own output is untouched
  (the differential test against xdftool still agrees block for block);
  the policy only changes what `Populator` adds on top of it. Items 2
  and 3 below — compaction, and passive reorganisation through
  `Mutator` — were still open at the end of this wave; so was `Mutator`
  ever adopting `Intent` for its own placement, which was a deliberate
  wave-1 non-goal, not an oversight (frag-bench's *fragmented* image was
  still built through `Mutator`, unchanged, on purpose — and still is,
  now through `.layout_policy(false)`, since wave 3 changed the default).

  **Wave 2 landed**: item 2 (compaction) and, for the one refusal it
  named, item 3 (closing `resize`'s gap). New module `src/compact.rs`,
  as inherent methods on `Mutator` (the mutation façade already owns
  every other write-path operation; a separate `Compactor` type would
  have been a second one). Two tiers, matching the survey's own §6b
  ordering:

  - **Tier 1**, `Mutator::defragment_file`: relocates one file's data
    blocks and `T_LIST` extension blocks (fetch order, per wave 1's own
    interleaving finding — not the data pointers alone) into one
    ascending run, without moving the header. Atomicity is exactly
    `edit_file`'s own shape, generalized from length to position:
    allocate the whole destination first, write every block at its new
    home, then **one** header write swaps the table over — before it the
    file is entirely the old one, after it entirely the new one.
    `bitmap_flag` never leaves −1; this is ordinary `Mutator` discipline,
    not a new safety story.
  - **Tier 2**, `Mutator::relocate_header`: moves a directory or file
    header to a new LBA. This is where the survey said the bugs would
    live, and the reason is concrete: `T_LIST` parent pointers, OFS data
    blocks' `header_key`, and `T_DIRCACHE` blocks' owning-directory field
    are all *hard*-checked on an ordinary read (typed errors, not
    `validate()` findings) — so patching one in place, on either side of
    the pointer that names the header, leaves a crash window in which the
    file or directory is unreadable through whichever number is
    currently authoritative. The fix: every hard-checked dependent gets a
    **fresh copy**, built already naming the header's new number, before
    anything points at any of it; a single write then retargets whichever
    pointer currently names the header (the parent's hash slot, or the
    previous entry's hash-chain longword). This tier does **not** keep
    `bitmap_flag` at −1 throughout — it is not a single commit the way
    tier 1 is, so it is honestly flagged mid-update for its duration,
    the same choice `resize`'s own root move and `Populator` already
    make, closed by a same-relocation retry exactly `resize`'s own
    "Retrying" contract, generalized from "the root" to "any header."
    The honest cost, stated rather than buried: on FFS a header move
    touches only its extension blocks; **on OFS it touches every data
    block**, because `header_key` is real and enforced. Full re-pointing
    list: parent's hash slot/predecessor's chain longword (the commit);
    every extension block's `TL_PARENT`; every OFS data block's
    `header_key`; a moved directory's own dircache chain (fresh copies)
    and its direct children's `parent` longwords (soft, patched after the
    commit, matching `resize`'s own reparenting); an LNFS overflow
    comment block's `HeaderKey` (fresh copy); hard-link `real_entry` in
    every link naming a moved target, and the one predecessor pointer in
    a moved link's own chain.
  - `Mutator::make_room(range)`: evacuates every movable allocated block
    (headers via tier 2; a file's data/extension blocks via a
    range-scoped tier 1 that moves only what is actually in the range,
    not the whole file — an unqualified whole-file move turned out not
    to fit for a large file straddling a small target range, caught by
    this wave's own resize-integration test) out of an LBA range. The
    root, bitmap pages and bitmap extension blocks are deliberately
    excluded — that is `resize`'s own `movable_metadata`, not
    duplicated here.
  - `Volume::resize_evacuating` closes the one refusal item 3 named:
    `ResizeError::RootTargetOccupied` retried once after an internal
    `make_room` of the target block. **Not** the default — `resize()`
    itself is unchanged and keeps refusing, on purpose (a caller that
    only wanted to *try* a shrink should not have a cheap, side-effect-
    free refusal silently turn into an invasive relocation it did not
    ask for) — a separate, explicitly opt-in method instead.
    `Volume::minimum_size_floor` reports the true floor (allocated blocks
    plus metadata overhead) alongside the existing, more conservative
    `Volume::minimum_size`.
  - `Mutator::compact`/`compact_with`: the policy pass, tier 1 across
    every file by default (`CompactOptions`, dry-run and progress
    callback included), tier 2 opt-in
    (`CompactOptions::relocate_headers`, default off — full metadata
    clustering by default was judged a bigger blast radius than a
    defragmentation pass should have without being asked). Measured on
    `examples/frag-bench.rs`'s own fragmented image: 80 runs in fetch
    order before, 1 after (`cargo run --example frag-bench`, which now
    also builds and reports a `defragmented.adf` third image, for
    replaying the real-ROM timing script against the compacted result by
    hand).

  Tested: relocate-then-verify for FFS and OFS (tier 1 and tier 2, files
  and directories), the frag-bench run-count claim above, `make_room`
  evacuating an occupied range, dry-run report accuracy, the
  `resize_evacuating` integration, both differential oracles reading a
  compacted image (`tests/differential.rs`), and three dedicated crash
  sweeps (tier 1, tier 2, `make_room`) each replaying every write prefix
  and checking both `repair()` alone (never `ReachableButFree`, never
  anything but `OrphanBlock` left over) and `repair()`-then-retry
  (byte-identical, and — for tier 1 — fully clean; tier 2/`make_room`
  can leave one superseded destination's blocks as a harmless leak,
  which a retry does not recover, on the same accepted terms `resize`'s
  own crash sweep already documents). **Narrower than the milestone's
  full ask, honestly**: the variant/block-size sweep is FFS/OFS at 512
  bytes with a handful of variants (not the full 512/4096 ×
  every-variant-incl.-LNFS matrix); the seeded property test
  (`a_random_interleave_of_mutations_agrees_with_a_model`) was not
  extended with compaction interleaved among its mutations. Both are
  gaps worth closing before this lands as anything more than "landed and
  working," not oversights papered over.

  **2026-09-09, as-built — a third bug found by independent review, in
  tier 2's hard-link handling.** `relocate_header_core`'s re-pointing
  list above says plainly what has to change when an entry moves: "hard-
  link `real_entry` in every link naming a moved target, and the one
  predecessor pointer in a moved link's own chain" — two different
  directions for two different cases. The bug was collapsing them onto
  one condition. The code called `retarget_link_chain` (walk from
  `entry.next_link`, stamp every block's `real_entry` at the mover's new
  address) whenever `entry.next_link != 0` — true both when the entry
  being relocated is the chain's *target* (`next_link` is the chain's
  head, and the walk is exactly right) and when it is itself a *link*
  with others chained after it (`next_link` names the *next link*, per
  `read.rs`'s own documentation of the field — a fact about that link's
  position, unrelated to where this one now lives). Relocating a mid-
  chain link therefore walked into the unrelated link after it and
  overwrote that link's `real_entry` with the *mover's* new address,
  breaking `resolve_link` for every link past the one that moved.
  `retarget_link_predecessor` (the correct fix for that direction —
  find whoever's `next_link` names the mover, the same predecessor-
  search shape the hash-chain and directory-chain re-pointing in this
  same function already use) was already being called too, just never
  exclusively: both ran whenever the moved entry happened to have a
  nonzero `next_link` and also be a link itself. Fix: the two paths are
  now mutually exclusive on `entry.real_entry == 0` (exactly the target
  case, per `parse_entry`'s own field-zeroing rule — `real_entry` is
  nonzero only on `LinkFile`/`LinkDir`) rather than `entry.next_link !=
  0` alone. Tested with a first-fails-then-fixed pair: a file `T` with
  hard links chained `T -> L1 -> L2 -> L3`, relocating the mid-chain
  `L1`, and asserting every one of `resolve_link(L1/L2/L3)` still
  resolves to `T`, `T`'s own `next_link` finds `L1` at its new address,
  and `L1`'s own `next_link` and `L2`'s `real_entry` are byte-for-byte
  untouched
  (`tier2_relocating_a_link_in_a_chain_does_not_repoint_the_links_after_it`);
  and the companion direction, relocating the chain's target `T` itself,
  confirming every link's `real_entry` moves to `T`'s new address
  (`tier2_relocating_a_link_targets_header_repoints_every_link`) — the
  behaviour the `entry.real_entry == 0` branch already got right, pinned
  alongside the fix rather than left implicit.

  **2026-09-11, as-built — a redundant O(n) scan in the same function's
  extension-pointer rebuild, turning it O(n²) on OFS, found by
  independent review.** `relocate_header_core`'s loop rebuilding an
  extension block's data-pointer table reads `old_d = ch.blocks[first +
  i]` and then, on OFS (where the fresh data-block copies are not at the
  same LBAs as the old ones, unlike FFS), searched `ch.blocks` all over
  again with `.position()` to find `old_d`'s index — a value that is, by
  construction, always `first + i`, the index it was just read from one
  line above. The search could only ever land back where it started;
  simplified to `first + i` directly, which is provably equivalent
  rather than a behaviour change, so no new test — the existing OFS
  compaction/defrag suite (`compaction_matrix_covers_every_variant_and_
  block_size` and the tier-2 OFS tests above) covers the code path
  unchanged and still passes.

  **2026-09-09, as-built — a separate independent-review pass, this one
  over `allocate_run`.** `allocate_run_from`'s exact-match search split
  the covered range into two disjoint scans — `from..covered` then
  `0..from`, where `from` is the hint's own bit position — and the
  "longest run" fallback did the same split. Neither piece is a real
  boundary in the bitmap; it is only where the scan happens to prefer to
  start. A single, genuinely contiguous free run that has the hint land
  in its *middle* was therefore measured as two shorter runs, one per
  side of the split, and an exact-length match that plainly exists in
  the unbroken run could be missed because neither half alone reached
  `n` — and the longest-run fallback under-reported for the same reason.
  Confirmed with a failing test
  (`allocate_run_finds_a_run_that_straddles_the_hints_own_split_point`):
  a volume with every block allocated except one genuine 60-block run,
  hinted dead in the middle of it (30 blocks either side), asking for 50
  — before the fix, `allocate_run(50, ...)` returned only 30 blocks, the
  length of whichever half the split happened to leave larger, despite
  60 contiguous free blocks actually being there. **Reachable in
  practice, not just a theoretical external-caller edge**: every
  `Intent` hint (`DataFor`'s header LBA, `HeaderIn`'s directory LBA,
  `MetadataNearRoot`'s root LBA) is a real, ordinary block number that
  can legitimately land inside a large free extent — a deleted big file
  leaving a large hole, and the next write's hint happening to fall
  inside it, is a completely unremarkable sequence of events, not
  something only a hostile caller could construct. Fixed by measuring
  every maximal free run over the *whole* covered range in one
  unbroken pass (`self.runs(0, covered)`, already the primitive both
  the old `find_run` and `longest_run` helpers — now removed, their one
  caller inlined — were built on) and choosing among the results by a
  `rank` function restating the same preference order the two-piece
  split used to give for free (distance forward from the hint,
  wrapping once at `covered`, so a run at or after the hint always
  outranks one before it) — without ever cutting a run's *measured
  length* at that boundary. The exact-match search takes the
  lowest-rank run of length at least `n`; the fallback takes the single
  longest run found, full stop. `allocate_run_reserves_one_contiguous_extent`
  and `allocate_run_degrades_gracefully_when_no_run_is_that_long`,
  already covering the non-straddling cases, still pass unchanged.

  **2026-09-11, as-built — `make_room`'s one silent no-op, found by
  independent review.** `make_room`'s eviction loop has a branch for a
  block in range that is allocated, not root furniture, and has no
  header and no `Survey::owner_of` entry — deliberately, for the root's
  own dircache chain (`Survey`'s own doc comment excludes it: relocating
  that chain is `resize`'s job, not a survey-driven relocation's) as well
  as for a genuine orphan. That branch just `break`s, leaving
  `report.blocks_evacuated` however it already stood and returning `Ok`
  — success, even though this call evacuated nothing for the one block
  that mattered. `Volume::resize_evacuating` only checked `.is_err()` on
  the result, so hitting this branch on the exact block a
  `RootTargetOccupied` collision named meant it retried `vol.resize(...)`
  against a collision guaranteed to still be there, reaching the
  identical refusal a second time for nothing. Fixed by giving
  `MakeRoomReport` a new field, `not_evacuated: Vec<u64>` — every
  allocated block still in the requested range when the call returns,
  which is empty on an ordinary success and non-empty exactly when this
  branch (or the loop's own runaway-guard) fired — and having
  `resize_evacuating` check whether the one block it asked to clear is
  still in that list, returning `EvacuationFailed` immediately instead of
  retrying when it is. Regression:
  `resize_evacuating_reports_a_collision_with_the_roots_own_dircache_chain_distinctly`
  (`tests/volumes.rs`) — a volume built directly with the low-level
  `Builder` (so the dircache chain's exact block address is known rather
  than assumed) with a one-block root dircache chain, grown to a size
  whose new root lands exactly on that block: `make_room` alone is shown
  to report `not_evacuated == [dc]` with zero blocks evacuated, and
  `resize_evacuating` end to end is shown to return `EvacuationFailed`
  rather than a second identical `RootTargetOccupied` reached by a wasted
  retry, with the volume left at its original size and root position
  either way.

  **2026-09-11, as-built — a documentation gap in `resize_evacuating`'s
  safety claim, found by independent review alongside the item above.**
  `resize_evacuating`'s own doc comment claimed its ordinary refusal path
  is "unconditionally safe (nothing is written before it fires)". That
  is true whenever `bitmap_flag` already reads `-1` on entry — the
  ordinary case — but not when a caller hands `resize_evacuating` a
  volume already left with `bitmap_flag != -1` by some earlier,
  unrelated interrupted operation: `resize()` itself repairs the bitmap
  first in that case (see its own doc comment, and the module's
  "Retrying" section), which is real writes to the medium, *before* its
  `RootTargetOccupied` check ever runs. Checked whether this is reachable
  through `resize_evacuating`'s own call pattern rather than assumed:
  it is not self-inflicted — a `Mutator` session's `bitmap_flag` stays
  `-1` throughout (PLAN.md's own M3 allocator entry: "nothing in this
  module ever lets the bitmap say 'free' about a block something
  reaches, so the flag stays −1 throughout"), so the second, post-
  `make_room` call `resize_evacuating` makes to `resize()` always finds
  `bitmap_flag == -1` regardless of what the first call found — but it
  *is* reachable at the outer boundary, from a volume `resize_evacuating`
  did not itself leave mid-update.
  Chose the doc fix over reordering the check: moving `RootTargetOccupied`
  ahead of the repair-first branch would make it read an unrepaired
  bitmap, and an unrepaired bitmap's "allocated" bits cannot be trusted
  to answer "is the target really occupied" correctly in the first place
  — the check needs exactly the write it would otherwise be moved ahead
  of. Not itself a correctness or data-integrity bug (`repair()` only
  ever rebuilds bitmap bookkeeping from a reachability walk — idempotent,
  and never a write either refusal needs undone), so the fix is
  documentation only, on both `Volume::resize` (its "Refuses (without
  writing anything) if" list gained the same caveat) and
  `Volume::resize_evacuating` — no test added, because nothing here
  changes behaviour to pin.

  **Wave 3 landed**: passive reorganisation — `Mutator`'s own day-to-day
  writes now place blocks by `Intent` instead of by the bare hint they
  used through wave 2, so a volume gets a little better every time it is
  written to rather than only when a compaction is run by hand. Three
  changes, all gated by a new `Mutator::layout_policy(bool)` (on by
  default — the policy only changes *where* a write that was already
  going to allocate blocks puts them, never what gets written, so there
  is no compatibility risk in leaving it on):
  `Mutator::create_file` allocates its data-and-extension sequence as one
  `Allocator::allocate_run` instead of one block at a time, so a new file
  is born as one run whenever a run exists, not merely "usually
  contiguous by accident"; `Mutator::create_dir`'s `T_DIRCACHE` block
  places itself with `Intent::MetadataNearRoot` instead of next to its
  own header, matching wave 1's finding that dircache blocks belong near
  the root; and `Mutator::edit_file` recognises the copy-out-copy-back
  shape a full-content rewrite produces (`offset == 0` and the write
  reaches at least as far as the old end of file — an append or a
  mid-file write never qualifies, deliberately, since passive means the
  caller's own operation dictates the work and this only chooses *where*
  a write that was already discarding the whole old chain puts the
  replacement) and lands the replacement as one contiguous run, the same
  `interleaved_positions` fetch-order splicing `create_file` uses. A
  small append still extends near the file's own last block
  (`Intent::DataFor`'s own hint), unchanged from wave 2.

  This is the PFS2DefragTry idea (credited below) closed for real:
  Aminet's tool copies a file out and back trusting the filesystem to lay
  it down afresh, which works on PFS2/AFS because *their* allocators seek
  contiguous runs and does not transfer to stock FFS's bare next-fit
  rover (`docs/layout-survey.md` §4) — but this crate *is* the writer
  whenever `Mutator` mutates, and `examples/frag-bench.rs`'s own
  fragmented image, copied out, deleted and recreated through `Mutator`
  with the policy at its default, now comes back as the same one run
  `defragment_file` produces by hand (`reorged.adf`, alongside
  `contiguous.adf`/`fragmented.adf`/`defragmented.adf`).

  A fourth item, light-touch directory pull, made its stated budget:
  `Mutator`'s dircache regeneration (`refresh_dircache`, `DOS\4`/`DOS\5`)
  already rewrites every *kept* cache block's content on every create,
  delete, rename and metadata change regardless of whether the chain grew
  or shrank, so trying to relocate one nearer the root costs nothing
  beyond the relocated block itself: a declined attempt (the candidate
  turns out no closer than what is already there) is freed again before
  it is ever flushed, so it never reaches the disk. The chain's own
  *first* block is left out of this — moving it would need a second write
  to `dir`'s own `TL_EXTENSION` pointer, which every other kept block does
  not, and that write is outside the budget this item was given, so it is
  skipped rather than forced, honestly, per this item's own instruction.

  Tested: `a_new_file_is_born_as_one_run_across_variants_and_block_sizes`
  (every variant, 512/1024/4096); `pfs2defragtry_pattern_yields_one_run`
  and `a_full_content_overwrite_lands_as_one_run` (the two shapes the
  copy-out-copy-back pattern takes — delete-and-recreate, and a straight
  `write_file`); `a_small_append_stays_adjacent_to_the_files_last_block`
  (growth does not get swept into a rewrite it did not ask for);
  `layout_policy_off_reproduces_the_pre_wave_3_placement` (the knob's own
  contract, pinned directly). The wave 2 seeded property test
  (`a_random_interleave_of_mutations_and_compaction_agrees_with_a_model`)
  and the general mutation ones
  (`a_random_interleave_of_mutations_agrees_with_a_model`,
  `a_random_interleave_of_writes_agrees_with_a_model_file`) pass unchanged
  with the policy on by default, confirming placement changes do not
  change semantics; every existing crash sweep passes unchanged too,
  since placement is chosen before any write begins, so the ordering
  discipline every sweep checks is untouched. Six existing tests needed
  the off switch or an updated expectation once the policy defaulted to
  on: `fragmented_volume` (the `tests/volumes.rs` fixture four compaction
  tests build on) now builds with `.layout_policy(false)`, because its
  whole job is threading a file through small holes on purpose, which
  `create_file`'s new `allocate_run` would otherwise skip past for a
  larger clean run — correct behaviour for a real caller, wrong for a
  fixture whose fragmentation *is* the point;
  `an_interrupted_overwrite_leaves_old_bytes_or_new_bytes_and_nothing_else`
  is about the in-place overwrite path specifically (its own doc comment
  says so), which a same-offset whole-content rewrite now opts out of by
  design, so it too now builds with the policy off, with a note pointing
  at the general write crash sweep for the relocation path's own crash
  safety (the ordinary allocate-then-one-header-commit shape, already
  exercised with the policy on). `examples/frag-bench.rs`'s own
  `build_fragmented` got the same fix, for the same reason.

  **Narrower than this item's full ask, honestly**: the directory-pull
  relocation is exercised only incidentally, by the existing multi-block
  dircache tests continuing to pass — no dedicated test constructs a
  directory whose second cache block starts far from it and checks the
  pull closes the gap, because doing so deterministically through
  `Mutator`'s own public surface (rather than by poking bytes directly,
  which every other synthetic-volume test in `tests/volumes.rs` does, but
  which this item's own modest scope did not seem to warrant) turned out
  to need more scaffolding than the item's budget justified. And wave 2's
  own honestly-recorded gaps are still open: the extension-boundary
  property-test interleaving noted in `f55994e`, and the full
  512/4096 × every-variant-including-LNFS compaction sweep (narrowed to
  FFS/OFS at 512 for the crash sweeps specifically, though
  `compaction_matrix_covers_every_variant_and_block_size` does cover the
  full variant/block-size matrix for the non-crash-sweep assertions).

  The format itself is the evidence. The root sits at the *midpoint* of
  the volume — the whole reason `canonical_root_lba` exists and the
  reason a resize has to move it — so that a seek to the root is on
  average half a disk from anywhere. On a DD floppy (80 cylinders × 2
  heads × 11 sectors = 1760 blocks, so 22 blocks to a cylinder) block
  880 is dead centre, and a file laid contiguously inside one cylinder
  costs *zero* head steps to read where the same file scattered costs a
  step per block. A design that put its root in the middle was designed
  around seek.

  So, in the order they were built (1 is wave 1, 2/3 are wave 2, passive
  reorganisation below is wave 3 — all landed above; this numbered list is
  kept as the original planning record rather than rewritten):

  1. **Policy at creation.** `format` and `Populator` already produce
     contiguous files, but by accident of a forward-cursor allocator
     rather than by stated policy. Make it a policy with the reasoning
     attached and the allocator hint-driven: a file's data adjacent to
     its header, a directory's children near the directory, the
     metadata a boot touches clustered near the root. This is where
     most of the value is, because it costs nothing at build time and
     every image gets it.
  2. **Compaction of an existing volume** — the same policy applied
     retroactively, which is the defragmenter. Relocating a block is
     copy-then-flip (allocate, write content, update the one pointer
     that names it, free the old), so every intermediate state is the
     old volume plus a leak: *safer* than the in-place file overwrite
     this crate already ships, and covered by the M3 allocator's
     mark-then-use discipline and `repair()`. Two tiers, very different
     in cost: **data blocks only** touches just that file's own header
     and extension tables, and gets most of the benefit; **header
     blocks** additionally require re-pointing the parent's hash chain,
     the children's parent longwords, `real_entry`/`next_link` chains
     and the dircache record — doable (resize does the root-move
     version of exactly this) but where the bugs would live.
  3. **Which closes resize's real gap.** `resize()` today refuses a
     shrink when user data occupies the new root's target
     (`RootTargetOccupied`), so on a volume packed from `reserved`
     upward — this crate's own allocator's normal output — the
     achievable minimum can be about twice the theoretical floor.
     Relocation is precisely what turns that refusal into a move, and
     makes `minimum_size()` report the floor rather than what the
     current implementation will accept. Compaction is not merely a
     companion to shrink; it is what lets shrink reach its limit.

  **Passive reorganisation** is the third strategy, and the one worth
  building alongside the policy rather than after it. Aminet's
  `PFS2DefragTry` (Martin Steigerwald, 1998 — crediting Simon for the
  idea) defragments by copying each fragmented file out and back,
  letting the filesystem lay it down afresh. That works on PFS2/AFS
  because *their* allocators deliberately seek contiguous runs; it does
  **not** transfer to FFS, whose allocator is a volume-wide next-fit
  rover with no such intent, so a recopy just lands the file wherever
  the rover happens to be. But this crate *is* the writer when it is
  the one mutating, and it chooses placement rather than hoping — so it
  can do passively what PFS does natively: when a file is being
  rewritten anyway, place the new blocks as one run; when a mutation
  passes through a directory, prefer allocations that pull its entries
  toward it. No separate pass, no tool to run, just leaving a volume
  better than it was found. The cost is that it makes writes place
  blocks by policy rather than by cursor, which is the same machinery
  item 1 needs — which is why the two belong in one piece of work.
  **Landed as wave 3, above**, including the directory-pull half within
  the budget it was given.

  **Survey before designing the policy**, the way the allocator and the
  LNFS layout were surveyed: ReOrg and the other commercial Amiga
  defragmenters had opinions tuned against real drives and real FFS
  read-ahead, and those opinions are worth more than first principles.
  One modern wrinkle for the notes: CF and SD-via-adapter have no seek
  cost but do have erase blocks, so contiguity still pays while
  cylinder-alignment reasoning does not transfer.

## Consolidation wave: the three items deferred from `26c994d`

`26c994d` consolidated four duplications an independent review found, and
deliberately deferred three more because all three touch the exact
write-ordering code `a4b3c1d`/`3a4a4ea` had just spent real effort
hardening against real bugs — consolidating crash-ordering-sensitive code
right after fixing crash-ordering bugs in it is exactly the moment to be
most careful, not least. **2026-09-09, as-built.** Worked one item at a
time, in increasing risk order, with the full test matrix and every crash
sweep (`tests/volumes.rs`'s `*_crash_sweep_*` and
`an_interrupted_overwrite_leaves_old_bytes_or_new_bytes_and_nothing_else`)
re-run after each change.

- **Item A — `resize.rs`'s `free_bit_on_disk` re-implements the bitmap's
  bit conventions.** Investigated and **left alone, documentation
  tightened only.** The four conventions themselves are *not*
  reimplemented independently — `free_bit_on_disk` already computes its
  word offset and bit index through the same shared layout constants
  (`OFF_BITMAP_BITS`, `BITMAP_CHECKSUM_INDEX`, `bitmap_bits_per_block`)
  `Bitmap` itself is built on, so there is exactly one place those four
  facts are stated. What has no shared home is the narrow *procedure* —
  flip one bit in one already-known-free page buffer and rewrite that
  page's checksum, ahead of and separate from a full rebuild — and no
  existing session type can be asked to do that safely at the point this
  function runs: an `Allocator::load` session assumes a *currently valid*
  bitmap over the volume's *current* geometry, and by the time
  `free_bit_on_disk` is called, `resize` has already forced
  `bitmap_flag` to 0 on the freshly written new root and already flipped
  `self.block_count`/`self.root.lba` to the new geometry in memory — a
  session over the *old* bitmap's page layout is not a thing that can
  exist at that point, and `Allocator::rebuilt` is not a shortcut either,
  since building one *is* the reachability walk this function's caller
  runs immediately afterward via `Volume::repair`, on bits this function
  has not finished pre-seeding as free. Confirmed by reading
  `free_bit_on_disk` fresh against the current tree, not from memory of
  an earlier pass. `src/resize.rs`'s doc comment on the function now
  states this reasoning directly ("Why this re-derives the bit math
  instead of calling `Allocator`") so a future reader does not have to
  re-derive it.

- **Item B — five chain-walk "find the predecessor" copies, one with a
  defensive re-read the other four lack.** Investigated and
  **consolidated into two shared primitives**, no write-order or
  written-value changes. `Mutator::splice_from_hash_chain`'s re-read of
  the spliced entry's successor (rather than trusting the caller's
  already-in-hand `Entry`) turned out to be necessary for *what splice
  writes* — promoting a removed node's successor into its predecessor —
  and structurally inapplicable to `compact::retarget_hash_chain` and
  `compact::retarget_link_predecessor`, which never remove a node from a
  chain at all: they redirect a predecessor's pointer to a caller-given
  new address, so there is no successor value to go stale in the first
  place. That is a real asymmetry, but not a gap — retarget's write does
  not derive from anything that could have been re-chained since it was
  read. `mutate.rs`'s `unlink_from_link_chain`, by contrast, *is* a
  removal (the link-chain analogue of `splice_from_hash_chain`) and does
  **not** re-read its own promoted value (`link.next_link`) the way
  `splice_from_hash_chain` does. Checked its one call site
  (`Mutator::unlink`) rather than assuming: `link` is captured at the top
  of `unlink` and nothing writes to `link.lba` before
  `unlink_from_link_chain` runs, so there is no window in the current
  call sequence for `link.next_link` to go stale — a failing test could
  not be constructed, so the asymmetry was documented rather than
  "fixed" into a behaviour change nothing demonstrates is needed. Two new
  `pub(crate)` primitives on `Mutator` do the shared walk-and-find-prev
  work: `locate_in_hash_chain(dir, slot, target) -> Option<u64>` (used by
  `splice_from_hash_chain`, `compact::retarget_hash_chain`, and
  `Mutator::release`'s membership check — three copies collapsed into
  one) and `locate_link_predecessor(target, entry_lba) -> u64` (used by
  `unlink_from_link_chain` and `compact::retarget_link_predecessor` —
  two copies collapsed into one). Each caller still decides, on its own,
  what to do with the predecessor once found — splice/unlink promote a
  successor, retarget writes a caller-given new address, `release` only
  asks whether the chain still names the block — so nothing about *what*
  gets written or in what order changed; only the walk that finds
  "whoever's pointer names this block" is now stated once per chain
  kind. Full test suite (both feature sets) and every crash sweep pass
  unchanged after the change, which is the property this consolidation
  had to preserve rather than merely hope for.

- **Item C — the checked-read/checksum-write-with-root-reload primitive
  exists four times, automatic in only one.** Investigated and **left
  alone**, with the verification the item asked for written down rather
  than assumed. `Mutator::put` (`src/mutate.rs`) is the one copy that
  automatically calls `reload_root()` when the written LBA is the root.
  The other three do not — but checked against the current tree, not
  stale line numbers, all three are either already covered by an
  adjacent manual reload or do not carry the hazard at all:
  `repair.rs`'s `put_block` has two call sites that write the root
  (inside `sever`'s `cut`, and the bitmap-pointer rewrite in `repair`
  itself), and both are followed by an explicit `self.reload_root()` in
  `repair()` (after `sever()` returns, and again after
  `mark_bitmap_valid`) before `repair()` returns. `resize.rs`'s inline
  write path writes the root twice (the first-write root copy, and the
  dircache-extension-pointer patch) and calls `self.reload_root()`
  immediately after each. `populate.rs`'s `put_checked` writes the root
  (`set_bitmap_flag`) with no reload at all — but `Populator` holds no
  `Volume` and therefore no cached root parse to go stale in the first
  place; the invariant this item is about does not apply to a type that
  never exposes a `Volume`-shaped read of its own root mid-session. A
  shared primitive would have to unify four genuinely different things:
  `Mutator<S>`'s bound-checked write returning `MutateError`,
  `Volume<S>`'s unbound-checked write (`repair::put_block`, which writes
  straight to `self.src` with no LBA range check at all) returning
  `AllocError`, `resize`'s deliberately-differently-bounded write
  (bounds-checked against `write_bound`, not `self.block_count()` — see
  Item A's finding that `resize` is mid-geometry-transition and cannot
  use the volume's own current bound) returning `ResizeError`, and
  `Populator`'s write returning `PopulateError` against a type with no
  `Volume` and no root cache to protect. Forcing one primitive across
  four different bound-checking semantics and four different error
  types, to fix a hazard that either does not exist at the current call
  sites or does not apply to the type at all, is exactly the kind of
  change the ground rule for this wave says not to force. No behaviour
  changed; no test added, because there is no new structural invariant
  to pin — the existing manual reloads, verified present and correctly
  placed, are what the invariant already rests on.

## In scope, not scheduled

- **Changing a volume's dostype in place, with data on disk** —
  tracked as GitHub issue #3. Three axes (intl, dircache, long-name),
  not four: FFS/OFS is excluded, waiting on `convert` instead.

- **CLI tools**, matching and eventually exceeding amitools' `xdftool`
  and `xdfscan` — tracked as GitHub issue #4. `xdftool`-equivalent
  inspection/editing/pack/unpack (through `.uaem` sidecars, the
  WinUAE/FS-UAE/Copperline convention) and repack/defrag,
  `xdfscan`-equivalent batch validate with an optional `--repair`
  this crate can offer that `xdfscan` cannot, and `convert` (new, no
  `xdftool` equivalent — building a fresh volume at a different block
  size, gated on a small `amiga-rdb` composition gap: `convert`'s
  destination needs one type satisfying `BlockMedium`, and
  `PartitionSource`/`PartitionSink` are two separate types today).

- **muFS**: in scope, deferred — tracked as GitHub issue #5. Needs a
  survey of real muFS volumes' dostype values before scheduling.

## Cross-cutting

- **Oracle discipline, on record so nobody "harmonises" it.** Each
  oracle is used exactly as its licence permits, and the asymmetry is
  deliberate, per-oracle, and already practised at every point of use:
  **xdftool** (amitools) is GPL — *run, never read*; a disagreement
  with it is investigated from our side and from MIT sources only.
  **fstool** is MIT — read and cited freely; reading its source is how
  its two `DOS\4`/`DOS\5` bugs were found and reported upstream
  (issues #42/#43). **AmiPart**'s `ffsresize.c` is MIT — its algorithm
  was ported with attribution, per the resize milestone. **AROS**
  (`afs.handler`) is licence-incompatible for copying — read for
  format *facts* only, always cited as such, and most valuable run
  as-is inside a guest, where the licence boundary is also a process
  boundary. **The ROM FFS itself** is the one oracle never read at
  all: it is observed, deterministically, through Copperline. A
  sibling PFS crate, if and when it exists, will have the opposite
  situation — tonioni's `pfs3aio` reference is 4-clause BSD and can be
  read and cited — and that difference should be stated in *that*
  crate's plan the way this entry states ours, rather than either
  crate borrowing the other's rule.

- [x] Errors: `Display` everywhere, `std::error::Error` under `std` —
      `Error<E>` and every `validate()` `Finding` state their
      consequence, and the transport error survives via `source()`
- [x] crates.io publish — **0.2.0 is out** (`v0.2.0`, tagged and
      released), carrying all three milestones rather than the
      read-only surface this box originally anticipated. Version 0.1
      is the primitives section above; 0.2 is read, create and mutate.
- [x] `#![deny(missing_docs)]` and the API-surface pass that earns a
      1.0 — landed (commit `e278335`). The pass found no renames
      needed: `Populator`/`Mutator` sharing verbs, `Metadata` vs
      `MetaUpdate`, and the error-type-per-operation-family shape all
      turned out to be contract, not accident, and are now stated once
      in `lib.rs`'s "shape of the API" section. `deny(missing_docs)`
      found exactly one gap (`BlockSource::Error`) across the whole
      public surface.

## Non-goals

- **Partition tables** — amiga-rdb's job, composed via the seam.
- **Other filesystem families** (PFS3, SFS) — own crates, per the
  one-family-per-crate rule. (muFS is *not* on this list: it is FFS
  with the owner/permission fields used, and is in scope — see
  milestone 1.)
- **Hunk loading, bootblock execution** — bytes in, bytes out.
- **A VFS/handler layer** — DosPacket semantics live in the consumer
  (m68k-machine's transport card, a FUSE wrapper, whatever); this
  crate is the format, not the filesystem *service*.
