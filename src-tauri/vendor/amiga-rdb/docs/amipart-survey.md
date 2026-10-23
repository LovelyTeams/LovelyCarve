# AmiPart survey — design input for milestone 3

Design input for the mutate-in-place milestone: what a real, actively
used RDB editor actually lets a user do, how it does it, and where its
choices should and should not become this crate's.

**Subject.** AmiPart (formerly DiskPart), a native AmigaOS 2.x partition
editor by John Hertell, MIT-licensed, at
<https://github.com/ChuckyGang/AmiPart>. Read at commit `4405376`
(2026-09-01). It builds both as an Amiga GadTools application and as a
native host CLI operating on `.hdf` images, from one source tree, so the
same write path is what runs against images.

> Portions of this document quote or paraphrase AmiPart source.
> AmiPart is Copyright (c) 2026 John Hertell, MIT License. The MIT
> licence's notice requirement is satisfied by this attribution; no
> AmiPart code is copied into this crate.

Sections 1–6 are **observations** — every claim cites AmiPart source.
Section 7 is **recommendations** for this crate and is entirely our own
opinion.

---

## 0. The one architectural fact everything else follows from

AmiPart has no incremental writer. `src/rdb.c:RDB_Read` parses the whole
RDB into a fixed in-memory model (`struct RDBInfo`: up to 64 `PartInfo`
and 32 `FSInfo` entries, `src/rdb.h:37-38`, with each filesystem's LSEG
payload reassembled into one `AllocVec` buffer). Every user operation
edits that model. `src/rdb.c:RDB_Write` then **regenerates the entire
RDB area from scratch** and writes it, at a fixed contiguous layout
(`src/rdb.c` layout comment above `RDB_Write`):

```
rdb_block_lo + 0          RDSK
           + 1 .. +N      PART   (N = num_parts)
           + N+1 .. +N+F  FSHD   (F = num_fs)
           + N+F+1 .. end LSEG   (each FS's chain, in FS order)
```

Consequences, all of them relevant to us:

- there is **no free-block management**, because there are never holes —
  every write repacks from `rdb_block_lo` with no gaps (section 4);
- **block numbers are not stable**: `pi->block_num` and `fi->block_num`
  are assigned during the write, so deleting the first partition
  renumbers every `PART` block after it;
- **`rdb_RDBBlocksHi` is recomputed, not preserved** — it is set to
  `last_used_blk`, the highest block this write actually used, so the
  declared reserved area *shrinks* when a partition or filesystem is
  deleted (section 3);
- anything in the RDB that AmiPart does not model is **dropped on the
  next write** (section 6);
- the RDSK block is the *first* block written, not the last (section 5).

The GUI and CLI/script front-ends are three different editors over that
one model: `src/partview.c` (GUI event loop), `src/cli.c` (one-shot
commands, each of which reads, edits and writes in a single call), and
`src/script.c` (accumulates edits, commits on an explicit `WRITE`).

---

## 1. Operation inventory

Blocks-touched column below means *device blocks written*. Because of
section 0, almost every table-editing operation writes the same thing:
the full RDB area, `rdb_block_lo .. last_used_blk`, in ascending block
order, plus a read-back verification pass over the same range.

### 1a. Partition-table operations (all go through `RDB_Write`)

| Operation | Where | What it changes | Blocks written |
|---|---|---|---|
| Add partition | `src/cli.c:cmd_addpart`, `src/script.c` `ADDPART`, `src/partview.c` `GID_ADD` / drag-in-free-space | appends a `PartInfo` to the array | whole RDB area |
| Delete partition | `src/cli.c:cmd_delpart`, `src/script.c:do_delpart`, `src/partview.c` `GID_DELETE` (~line 3405) | `memmove`-down of the array, `num_parts--` | whole RDB area |
| Rename / retype / bootpri / bootable / automount / dev-flags | `src/partview_dialogs.c:partition_dialog` (fields applied around lines 869–941) | `drive_name`, `dos_type`, `boot_pri`, `flags` | whole RDB area |
| Edit envec detail (reserved blocks, interleave, buffers, buffer mem type, boot blocks, `MaxTransfer`, `Mask`, `Control`, `pb_DevFlags`) | `src/partview_dialogs.c:partition_advanced_dialog` (lines 447–462) | the named `PartInfo` fields | whole RDB area |
| Resize (end only) | GUI right-edge drag (`src/partview.c` ~2554, commit ~2777); `GROW`/`SHRINK` (`src/cli.c:cmd_grow`, `cmd_shrink`) | `high_cyl` | whole RDB area (+ filesystem blocks for `GROW`/`SHRINK`) |
| Move (start changes) | `src/partmove.c:PART_Move` via `src/partview_move.c:offer_move_partition` | copies partition **data**, then sets `low_cyl`/`high_cyl` | partition extent, then whole RDB area |
| Add filesystem driver | `src/cli.c:cmd_addfs`, `src/script.c:do_addfs`, `src/partview_fs.c` `FSDLG_ADD` | appends an `FSInfo`, loads the binary into `fi->code` | whole RDB area (LSEG chain regenerated) |
| Edit filesystem entry | `src/partview_fs.c` `FSDLG_EDIT` | replaces the `FSInfo` in place | whole RDB area |
| Delete filesystem entry | `src/partview_fs.c` `FSDLG_DELETE` | `memmove`-down, `num_fs--`, **and resets every partition whose `dos_type` matched to `DOS\1` (FFS)** | whole RDB area |
| Change disk geometry (cylinders) | `src/cli.c:cmd_init_newgeo` | `cylinders`, `hi_cyl`; keeps heads/sectors and `lo_cyl` | whole RDB area |
| Fresh RDB | `src/cli.c:cmd_init_new` → `src/rdb.c:RDB_InitFresh` | resets everything | whole RDB area |
| Fresh RDB with MBR | `src/cli.c:cmd_init_newmbr` | as above with `rdb_block_lo = 1` | whole RDB area |
| Raise the reserved area | `src/partview.c` `GID_WRITE` overflow handler (~3508) | raises `lo_cyl` after a failed write | whole RDB area (retry) |

### 1b. Operations that bypass `RDB_Write`

| Operation | Where | Notes |
|---|---|---|
| Write a `BADB` chain | `src/partview_rdb.c:write_badb` | appends `BADB` blocks at `rdb_block_hi + 1` and **patches the RDSK block in place** (read-modify-write of `rdb_BadBlockList`, `rdb_RDBBlocksHi`, `rdb_HighRDSKBlock`). GUI only. |
| Restore single block / extended (ERDB) backup | `src/cli.c:cmd_restore`, `cmd_restoreext` | raw block writes into `rdb_block_lo..` |
| Zero a partition | `src/partmove.c:PART_Zero` (`ZEROPART`) | writes partition data only, RDB untouched |
| Partition clone / image in-out / copydisk | `src/partclone.c`, `src/imagecopy.c` | data-level |

### 1c. What is recomputed on every write

`src/rdb.c:RDB_Write`, in order of the code:

- `rdb->part_list` / `rdb->fshdr_list` — set to the first `PART` / `FSHD`
  block, or `0xFFFFFFFF` when the count is zero;
- every `pb_Next`, `fhb_Next`, `lsb_Next` — chains are rebuilt as
  consecutive ascending block numbers;
- every `pi->block_num`, `fi->block_num`, `fhb_SegListBlocks`;
- `pb_SummedLongs` = `block_size / 4` for `PART` and `RDSK`, a constant
  128 for `FSHD`, and `5 + data_longs` on the final `LSEG` of a chain
  (`src/rdb.c:fill_lseg_chain`) so the boot ROM copies the right partial
  length;
- all checksums, via `src/rdb.c:block_checksum` (sum of `num_longs`
  big-endian longwords, store the negation);
- `rdb_RDBBlocksLo` (preserved from the read), `rdb_RDBBlocksHi` =
  `last_used_blk`, `rdb_HighRDSKBlock` = `last_used_blk` — **the two are
  always written equal**, so AmiPart never leaves declared headroom;
- `rdb_CylBlocks` = `heads * sectors`, `rdb_Park` = `cylinders`;
- `rdb_Flags` — `RDBFF_LASTTID` is forced on regardless of the model;
- `rdb_Reserved1[6]` = `0xFFFFFFFF` each;
- `de_TableSize` is hard-coded to 19 for every partition, and the whole
  envec is regenerated from the modelled fields (`de_SecOrg` and
  `de_PreAlloc` forced to 0);
- `rdb->block_num` is normalised to `rdb_block_lo` — the RDSK is always
  written at `rdb_block_lo` even when the read found it elsewhere in
  blocks 0..15, with a comment explaining that using the found location
  would index outside the staging buffer.

---

## 2. Resize and move edge cases

**Everything is in whole cylinders; there is no sub-cylinder unit
anywhere in the model.** `PartInfo` stores only `low_cyl`/`high_cyl`, so
rounding happens entirely at parse time of the user's request.

**Rounding direction.** All byte→cylinder conversions round *down*:
`src/cli.c:cli_parse_high` computes `cyls = bytes / (heads * sectors *
512)` and rejects a request that rounds to zero; `cli_parse_low` with a
`K`/`M`/`G` suffix computes `bytes / cylsize`, i.e. "the cylinder
containing that byte offset". A bare number is a literal cylinder.
`src/cli.c:cmd_grow` likewise uses `add_cyls = bytes / (blks_cyl * 512)`.
**Note the hard-coded 512**: cylinder byte arithmetic in the front ends
ignores `bd->block_size`, so on a non-512-byte device the size a user
asks for and the size they get diverge.

**Growing.** Only upward, and only into a gap. `src/cli.c:cmd_grow`
computes `gap_max` as `rdb->hi_cyl` capped by the lowest `low_cyl` above
the partition, minus one; a request beyond that is **clamped** with a
message rather than refused (`END`/`MAX` means "take the whole gap").
`ADDPART` clamps the same way — `HIGH` is pulled back to one cylinder
before the next partition — unless `ENFORCESIZE` is given, which turns
the clamp into an error (`src/cli.c:cmd_addpart`, overlap block).

**Growing downward is not offered.** There is no operation that lowers
`low_cyl` in place. The GUI explicitly refuses a left-edge drag with a
"cannot resize the start" requester (`src/partview.c` ~2585,
`MSG_PV_NORESIZE_START_BODY`). The rationale is stated in the commit
path around `src/partview.c:2786`: a `low_cyl` change relocates every
filesystem block relative to the partition start, so it destroys data.

**Data movement.** The GUI resize drag edits *only the table entry* —
the drag handler assigns `rdb->parts[..].high_cyl` directly
(`src/partview.c` ~2956). On release, a pure end-grow offers to grow the
filesystem too (`offer_ffs_grow` / `offer_pfs_grow` / `offer_sfs_grow`)
and a pure end-shrink routes to `offer_shrink`, which scans the
filesystem's allocation bitmap and performs real filesystem surgery; a
user who declines gets a plain destructive-change confirmation and the
table entry changes anyway.

**Move is the only operation that relocates partition data.**
`src/partmove.c:PART_Move` copies `cyl_count * heads * sectors *
phys_per_lb` device blocks in `MOVE_CHUNK` runs, front-to-back when
moving down and back-to-front when moving up (so a self-overlapping move
is safe), then patches SFS root blocks' absolute `firstbyte`/`lastbyte`
for the new location, then updates `low_cyl`/`high_cyl` and lets the
caller call `RDB_Write` (`src/partview_move.c:offer_move_partition`
around line 565). `PART_CanMove` refuses: an unchanged start, a start
below `rdb->lo_cyl`, an end above `rdb->hi_cyl`, and any overlap with
another partition. FFS and PFS are *not* offset-patched — only SFS
carries absolute byte offsets that need it.

**Last cylinder.** `hi_cyl` is the inclusive last usable cylinder and is
sanitised at read time: `src/rdb.c:RDB_Read` silently clamps
`hi_cyl >= cylinders` down to `cylinders - 1` (naming `lide` on large
drives as the tool that writes the off-by-one), and if `lo_cyl` is out of
range or above `hi_cyl` it resets both to `1` and `cylinders - 1`.
`RDB_Write` then rejects any partition with `high_cyl > rdb->hi_cyl`.
There is no partial-cylinder handling at the end of the disk: whatever
`cylinders` says is addressable, is.

**Can the first partition move to free space so the RDB area can grow?**
Yes, and this is exactly the documented workflow — but as two separate
user actions, not one operation. When a write overflows (section 6),
`src/partview.c` `GID_WRITE` computes the `lo_cyl` the metadata needs; if
a partition starts below that, it emits `MSG_PV_OVERFLOW_BLOCKED` naming
the partition and the cylinder it must move to. The user then runs the
move themselves and retries the write.

---

## 3. Delete behaviour

`src/cli.c:cmd_delpart`, `src/script.c:do_delpart` and `src/partview.c`
`GID_DELETE` are the same three lines: shift the array down over the
deleted entry, decrement the count, mark dirty. The filesystem-entry
delete (`src/partview_fs.c` `FSDLG_DELETE`) is the same, plus the
`dos_type` reset described in section 1a.

What that means on disk, all of it a consequence of the full rewrite:

- **the `PART` block is neither zeroed nor unchained** — it ceases to
  exist because the whole area is regenerated one block shorter. The
  block that used to hold the last `PART` entry is simply not written
  this time, so its **previous contents remain on disk**, now above
  `rdb_RDBBlocksHi` and referenced by nothing. AmiPart never zeroes a
  block it stops using;
- **`rdb_HighRDSKBlock` decreases**, to the new `last_used_blk`, as does
  `rdb_RDBBlocksHi` (they are always written equal);
- **the chain order is not the old order**: `src/rdb.c:RDB_Read` sorts
  the partition array by `low_cyl` with an insertion sort immediately
  after the chain walk, so the rewritten `PART` chain is in cylinder
  order regardless of how it was chained before. A round-trip with no
  edits at all can therefore reorder the chain;
- **no holes are ever left**, so nothing tracks or reuses them;
- deleted drives are remembered by name and `UnmountDevice`d after the
  write (`src/cli.c:cmd_delpart`, `src/partview.c:unmount_deleted_partitions`)
  so no reboot is needed.

---

## 4. Free-block management

There is none, and none is needed: the layout is a pure function of
`(rdb_block_lo, num_parts, num_fs, per-FS code size)`, computed in one
pass at the top of `src/rdb.c:RDB_Write` before anything is written.
There is no scan, no bitmap, no allocator, and no compaction step —
compaction is unconditional and implicit, because every write is a
compaction.

The only exception is `src/partview_rdb.c:write_badb`, which is a true
append: it places its blocks at `rdb->rdb_block_hi + 1` and pushes
`rdb_RDBBlocksHi`/`rdb_HighRDSKBlock` up to cover them. That append does
not survive: the next `RDB_Write` writes `rdb_BadBlockList =
0xFFFFFFFF` unconditionally and recomputes `rdb_RDBBlocksHi` back down,
orphaning the `BADB` blocks.

LSEG sizing is fixed at 492 payload bytes per block regardless of device
block size (`src/rdb.c:fill_lseg_chain`, and the same constant in
`RDB_Write`'s pre-calculation) — i.e. the 512-byte assumption is baked
into the driver payload layout too.

---

## 5. Crash safety

**Write ordering is ascending block number, which puts the RDSK first.**
`RDB_Write` builds every block in one `big_buf` staging buffer — and
fills the RDSK *last in memory*, because it needs the chain heads — but
then writes `for (b = 0; b < total_blocks; b++)` from `rdb_block_lo`
upward. `rdb_block_lo + 0` is the RDSK. So the pointer that publishes the
new table is committed **before** the `PART`, `FSHD` and `LSEG` blocks it
points at. An interrupted write leaves a valid, checksum-correct RDSK
whose chains lead into whatever was there before.

The header comment above `RDB_Write` says so plainly: *"The write is NOT
atomic: a power failure or driver error mid-write leaves a partial RDB.
Use backup/restore to protect against that."* There is nothing resembling
staged commit ordering, and no journal or shadow copy. The mitigation is
procedural: `BACKUP`/`BACKUPEXT` before, `VERIFY`/`VERIFYEXT` after, plus
the GUI's move dialog requiring a "I have a backup" checkbox
(`src/partview_move.c`, `MVDLG_BACKUP`) before it will move a partition.

Two things AmiPart *does* do that are worth keeping:

- **read-back verification**: after the write loop, every block is
  re-read and `memcmp`'d against what was written, recording the failing
  block and byte offset (`bd->last_verify_block`, `last_verify_off`) for
  the error message. It deliberately verifies after the whole write
  rather than per block, to dodge an A3000 `scsi.device` write-cache
  interaction;
- **single-block writes**: multi-block SCSI write DMA on the A3000 SDMAC
  is documented (same comment) to shift data by four bytes, so
  `BlockDev_WriteBlock` issues one block per command.

The one operation that *is* correctly ordered is the move
(`src/partmove.c:PART_Move`): the data copy completes first and the table
entry is updated afterwards, so an interrupted move leaves the RDB
pointing at the intact original — which is only safe because
`PART_CanMove` guarantees the destination does not overlap another
partition. `GROW`/`SHRINK` are ordered the other way: the filesystem is
resized first and the RDB written after (`src/cli.c:cmd_grow`, near the
end), so a crash in between leaves a filesystem that believes it is
larger than its partition.

---

## 6. Invariants maintained and violated

### Maintained

- **Overlap is checked before writing.** `RDB_Write` refuses (returns
  `FALSE` before allocating the buffer) if any partition has `low_cyl >
  high_cyl`, `low_cyl < rdb->lo_cyl`, `high_cyl > rdb->hi_cyl`, or if any
  two partitions' cylinder ranges intersect — an O(n²) sweep, deliberate
  and commented as a self-contained safety net behind the front-end
  checks. `src/rdb.c:RDB_IntegrityCheck` reports the same conditions
  read-only, plus per-block ID and checksum verification.
- **Metadata cannot spill into the first partition's cylinder.** Before
  writing, `RDB_Write` computes `reserved_end = lo_cyl * heads * sectors`
  and fails if `last_used_blk >= reserved_end`, stashing
  `last_overflow_need` / `last_overflow_avail` on the device handle so the
  caller can report the shortfall in blocks. This is the exact
  refuse-over-overlap discipline our plan asks for, at the block level.
- **Chain walks are hostile-input safe.** `src/rdb.c:chain_seen` gives
  every walk (PART, FSHD, LSEG, and the integrity checker) cycle
  detection; block 0 and the RDSK block are treated as terminators; IDs
  and checksums are verified per block; LSEG reassembly is bounded by the
  exact block count from a counting first pass.

### Violated, or simply not upheld

- **`rdb_RDBBlocksLo..=Hi` is not the boundary AmiPart respects.** It
  respects `0 .. lo_cyl * heads * sectors - 1` and *rewrites*
  `rdb_RDBBlocksHi` to whatever it used. An image whose RDB area was
  declared generously loses that declaration on the first edit; a `BADB`
  chain placed above the old `Hi` (section 4) is orphaned by it.
- **Checksum validation is skipped when `SummedLongs` is out of range.**
  `RDB_Read` verifies only when `2 <= SummedLongs <= 128` and otherwise
  trusts the four-byte magic, with a comment justifying it as tolerance
  for non-standard tools. Combined with the fixed 512-byte read buffer in
  `RDB_Read` (`AllocVec(512, ...)`) and `rdb->blk_size = bd->block_size`,
  AmiPart effectively assumes 512-byte metadata blocks and ignores
  `rdb_BlockBytes` entirely on read.
- **Unmodelled fields are dropped on write.** `RDB_Read` never reads
  `rdb_BadBlockList`, `rdb_DriveInit`, or the controller
  vendor/product/revision strings, and `RDB_Write` writes
  `0xFFFFFFFF` for the first two and leaves the controller strings zeroed
  (the staging buffer is `MEMF_CLEAR`). `de_TableSize` becomes 19 for
  every partition whatever it was, `de_SecOrg`/`de_PreAlloc` become 0,
  `rdb_Interleave`/`WritePreComp`/`ReducedWrite`/`StepRate`/`AutoParkSeconds`
  become 0 and `rdb_Park` becomes `cylinders`. In the GUI, `pb_Flags` is
  rebuilt from four checkboxes (`src/partview_dialogs.c` ~916), so any
  other flag bit set by another tool is cleared by an unrelated edit.
- **The stale RDSK case.** If the RDSK was found at a block other than
  `rdb_block_lo`, the rewrite puts a new RDSK at `rdb_block_lo` and leaves
  the old one where it was; nothing zeroes it.
- **Endianness discipline stops at `rdb.c`.** `src/rdbbe.h` documents the
  BE32R/BE32W rule and `rdb.c` follows it, but
  `src/partview_rdb.c:write_badb` writes `BadBlockBlock` fields by direct
  struct assignment — correct on m68k, wrong on the little-endian host
  build. (GUI-only path, so the host CLI does not reach it.)

### The historically tiny RDB area

AmiPart handles this at the *cylinder* level rather than by growing
`rdb_RDBBlocksHi`, because for it the RDB area simply *is* everything
below `lo_cyl`:

1. `src/rdb.c:RDB_InitFresh` sizes new disks generously — `lo_cyl =
   ceil((1 + 64 + 32 + 533) / (heads * sectors))`, about 630 blocks, i.e.
   room for a full table plus ~256 KB of driver code (one cylinder on a
   1008-block-per-cylinder geometry, more on small geometries).
2. On an existing disk with a small `lo_cyl`, the write fails the
   `reserved_end` guard and the GUI's `GID_WRITE` handler
   (`src/partview.c` ~3508) computes `new_lo = ceil(need / blks_per_cyl)`:
   if no partition starts below `new_lo`, it offers to raise `lo_cyl` and
   retries the write; if one does, it names that partition and tells the
   user to move it first.

So "expand the area" exists, is user-confirmed, and is guarded by exactly
the right predicate (no partition inside the newly claimed space) — but
it moves `rdb_LoCylinder`, and `rdb_RDBBlocksHi` follows only as a
recomputed side effect.

---

## 7. Recommendations for this crate

Opinion from here on.

### 7.1 The model to adopt, and where to differ

Adopt AmiPart's **read-model-edit-write-whole-area** shape. It is the
reason AmiPart needs no allocator, no hole tracking, no compaction, and
no chain surgery, and it makes the layout a pure function of the model —
which is exactly the property that lets us validate a complete layout
before touching the sink, the way `RdbBuilder::build` already does. An
incremental edit path would buy nothing here: an RDB area is a few dozen
blocks.

**Differ on four points**, each for a stated reason:

1. **Never shrink `rdb_RDBBlocksHi`.** Treat the declared area as a
   *lease*: `rdb_RDBBlocksHi` is preserved from the parse and only ever
   grows (deliberately, section 7.3); `rdb_HighRDSKBlock` is the
   recomputed high-water mark. AmiPart writing the two equal is why its
   `BADB` support silently self-destructs, and why an image loses its
   headroom on the first edit.
2. **Preserve everything we parsed.** `Rdb` already carries
   `drive_init`, `bad_block_list`, the controller strings, the raw envec
   longwords and `rdb_Flags` as a bare `u32` precisely so a rewrite does
   not drop them. Mutation must round-trip every one of those; AmiPart's
   silent field loss is the single most-copyable mistake in this survey.
   This needs to be a *tested* property, not a documented intention.
3. **Zero the blocks we stop using**, inside the RDB area only. A shorter
   table after a delete should not leave a checksum-valid orphan `PART`
   block on disk for the next tool's RDSK scan or a forensic reader to
   find. Cheap, and it makes "the area contains exactly what the chains
   say" true.
4. **Order writes for crash shape** (section 7.4). AmiPart's ascending
   write order is its weakest point and there is no compatibility reason
   to reproduce it.

### 7.2 Proposed operation set

Modelled on what AmiPart's users actually do, minus what our founding
non-goal excludes.

**Partition table**

- `add_partition(PartitionSpec) -> Result<..>` — reuse the existing
  `PartitionSpec`/`Placement` vocabulary from milestone 2 so create and
  edit speak one language.
- `remove_partition(index)`.
- `set_name`, `set_dos_type`, `set_boot_priority`, `set_bootable`,
  `set_automount`, `set_flags_raw` — AmiPart's dialog proves these are
  the everyday edits. Offer the raw-flags escape hatch so we never do
  what its checkbox rebuild does.
- `set_envec_field` / a typed envec accessor covering the advanced
  dialog's set (reserved blocks, prealloc, interleave, buffers, buffer
  mem type, boot blocks, `MaxTransfer`, `Mask`, `Control`, `Baud`) plus
  `pb_DevFlags`.
- `resize_partition(index, new_high_cyl)` and
  `set_extent(index, low, high)` — **table entry only**, cylinder-aligned,
  refusing any overlap. See 7.5 on why this crate does not move data.

**Filesystem drivers**

- `add_filesystem(FileSystemSpec)`, `remove_filesystem(index)`,
  `replace_filesystem(index, spec)`. Note AmiPart's `ADDFS` documents
  "add or replace" but `cmd_addfs`/`do_addfs` always append with no
  dedupe by `dos_type` — do not copy that; either dedupe or make
  replacement explicit.
- Deliberately **do not** silently rewrite partitions' `dos_type` when a
  driver is removed, as `FSDLG_DELETE` does. Report the affected
  partitions and let the caller decide.

**Disk level**

- `set_geometry_cylinders(n)` (AmiPart's `INIT NEWGEO` — the
  disk-got-bigger case, keeping heads/sectors and `lo_cyl`).
- `set_disk_identity(vendor, product, revision)` and flags.
- `set_bad_blocks(Vec<BadBlockEntry>)` — we already parse `BADB`; being
  the tool that can *preserve and rewrite* one is a differentiator, and
  it costs almost nothing once the area is laid out as a whole.

**Area**

- `expand_rdb_area(new_hi)` and `set_lo_cylinder(n)` (7.3).

### 7.3 Expanding the area — take both levers, keep the predicate

AmiPart's predicate is right and worth copying verbatim in spirit: *raise
the boundary only when no partition starts inside the space being
claimed*. Our version should be stricter and stated in blocks, not
cylinders, because we validate extents in blocks already: an expansion is
permitted iff the claimed blocks intersect no partition's
`start_lba..start_lba+block_len` — the same arithmetic
`ValidationIssue::PartitionOverlapsRdbArea` already performs. Growing
`rdb_RDBBlocksHi` and raising `rdb_LoCylinder` are two different levers
(the first can often be done with no partition change at all, if the
first partition starts above the area) and both should exist. The error
on refusal should name the blocking partition and the value it would have
to move to — AmiPart's `MSG_PV_OVERFLOW_BLOCKED` is a good model for the
message, and it is the shape our "error naming the shortfall" item wants.

### 7.4 Crash shape

The write order should be, explicitly:

1. new `PART`/`FSHD`/`LSEG`/`BADB` blocks, into space not referenced by
   the current on-disk chains where the layout allows it;
2. the `RDSK` block last, as the single pointer flip;
3. only then zero the blocks the old layout used and the new one does
   not.

Step 1's "where the layout allows it" is the honest caveat: with a
contiguous repack, new blocks frequently land on the same LBAs as old
ones and a crash there is unrecoverable regardless of ordering. Two
options worth prototyping when the item lands — (a) accept it and rely on
RDSK-last, which already reduces the failure to "old table, possibly
scribbled", or (b) when the area has headroom, lay the new chains out in
the *unused* part of the area and flip the RDSK, which is a genuine
atomic swap. Option (b) is another reason not to shrink
`rdb_RDBBlocksHi`. Whatever we choose, the ordering must be asserted by a
test that truncates the write at every block index and re-parses.

### 7.5 What we deliberately will not do

- **No partition data movement, and no filesystem resize.** AmiPart's
  `PART_Move`, `GROW` and `SHRINK` reach into FFS/SFS/PFS internals; this
  crate stops at the partition boundary by its founding non-goal. Our
  resize is a table-entry edit, which means shrinking is *destructive to
  the filesystem* and must be documented as such — AmiPart's
  `MSG_PV_RESIZE_SHRUNK` requester exists for the same reason.
- **No sub-cylinder extents.** Confirmed by AmiPart's model: cylinders
  are the only unit anywhere.
- **No 512-byte assumptions.** AmiPart has them in three places (front-end
  cylinder byte math, the 512-byte read buffer, the 492-byte LSEG
  payload). Milestone 1 already decided against that; the mutation path
  must not reintroduce it.

### 7.6 AmiPart as a differential oracle — a scope correction

The plan's last milestone-3 item expects to "apply the same edit in both
tools and diff the resulting images block-by-block". **A byte-level diff
will not match, and not because either tool is wrong.** From sections 1c,
3 and 6, an AmiPart write will differ from ours on at least: chain order
(it sorts by `low_cyl`), `rdb_RDBBlocksHi` (it shrinks it to
`HighRDSKBlock`), `rdb_BadBlockList` and `rdb_DriveInit` (it zeroes
them), the controller identity strings (zeroed), `de_TableSize` (forced
to 19) and several dead geometry fields, and the RDSK's location when the
original was not at `rdb_blocks_lo`.

The oracle is still valuable — it is a second independent implementation
whose source we may read — but the comparison has to be **semantic**:
parse both results with this crate and compare the models, on the fields
both tools claim to own. That, and the crash-truncation test, are
different things and both should exist. PLAN.md has been amended to say
so.

---

## 8. PLAN.md amendments arising

Per the plan's own rule, the gaps this survey found were added to
PLAN.md's milestone 3 before anything else:

1. **Preserve unmodelled fields** — a new item. AmiPart's most damaging
   behaviour has no counterpart item in our plan, and "we parse it so we
   will write it" is not automatic once an editor is in the middle.
2. **Zero what we stop using** — a new item, covering deleted `PART`
   blocks, the shrunken tail of the area, and the stale-RDSK case.
3. **Edit existing structures** — scope note added: table-entry edits
   only, no partition data movement, so shrink is documented as
   destructive.
4. **AmiPart as second differential oracle** — re-scoped from a
   block-level diff to a semantic model comparison, for the reasons in
   7.6.
