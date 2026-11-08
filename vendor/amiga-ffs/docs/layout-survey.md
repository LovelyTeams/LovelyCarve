# Block layout: prior art, runtime behaviour, media reality, and a policy

Research survey for the PLAN.md entry **"Block layout policy, and
compaction"**, done before writing any allocator or compactor code, per
that entry's own instruction. Scope: what block-level layout policy this
crate should use at creation time (`format`/`Populator`) and what a
compaction pass should do, for volumes that end up on real hardware
(mechanical drives, CF/SD via adapter, real floppies) as well as
emulators.

Three background research passes fed this: commercial Amiga
defragmenters and their manuals/reviews/Usenet discussion; what FFS and
AROS's `afs.handler` actually do with the I/O path (read-ahead,
`AddBuffers`, the allocator's own placement policy); and the physical
cost model for the three target media, including whether the RDB's
geometry fields can be trusted. Every claim below is tagged with its
provenance and confidence; §5 collects everything the research could not
pin down, and §6 is the recommendation, split explicitly into
evidence-backed and reasoned-from-first-principles.

## 1. What the commercial defragmenters actually did

### ReOrg (Holger Kruse) — the one that matters

One continuous lineage, not two unrelated tools sharing a name as the
research brief guessed: V1.1 (1992) through V3.1/V3.11 (patched
1993-09-11) to a final Aminet upload in 1997. Shareware, US$10.
Sources: the Aminet package readmes for `ReOrg3_1` and `ReOrg3_11pch`
(primary, Kruse's own text); the AmigaGuide manual, reachable only
through search-engine-cached fragments of a now-unreachable mirror
(`memphisamigagroup.net/diskmags/199510/ReOrg/...` — **secondary
relay of primary content**, quotes below are literal fragments as
indexed, not independently re-verified against the original file);
*Amiga Magazine* issues 18 and 20 review metadata via amr.abime.net
(review existence and bibliographic detail confirmed — Nov–Dec 1992 and
Mar–Apr 1993, reviewers André Viergever and Ruud Dingemans — but the
article bodies did not come through, so no review *content* is quoted
here); a Usenet thread ("ReOrg vs ABTools",
`comp.sys.amiga.applications`) reachable only via search-snippet
indexing after Google Groups rate-limited every direct fetch.

**The load-bearing finding.** ReOrg's own author judged *directory and
file-header scatter*, not file-data fragmentation, as the dominant
real-world cost on Amiga FFS/OFS, and built the tool's own fragmentation
metric around that belief:

> "The main reason for poor disk performance on Amigas is directory
> fragmentation, i.e. the individual file header blocks that make up a
> directory are sometimes scattered over the disk."

> "the value ReOrg reports as fragmentation is a heuristic value based
> on the average distance of file header blocks compared to the optimal
> average distance" — and explicitly — "ReOrg does not measure file
> fragmentation because that does not have much impact on overall disk
> speed."

This is a direct contradiction of the natural first assumption (that
"defragmentation" on Amiga FFS chiefly means making file *data*
contiguous, the way it does on FAT). ReOrg still reorders file data
during a pass ("ReOrg usually optimizes directory AND file layout"), but
treats it as secondary. **Confidence: primary content, high** — this is
Kruse's own stated design rationale, not an inference.

**Placement policy, as far as it could be recovered:**

- Whole-volume, single-pass, seek-minimizing: "ReOrg always optimizes
  the whole partition at a time with as few head movements as
  possible" — a global layout is computed, then written in an order
  chosen to minimize head travel, not a per-file relocate-and-retry.
- Free space after optimization is **placed by user choice**, and one
  documented mode is explicitly to make "the directory area and the
  file area... stored consecutively" — i.e. ReOrg's model treats
  directory metadata and file data as two separately-placed, each
  internally-contiguous regions, not an interleaved layout. Other
  surfaced options pack free space toward the front or the middle of
  the partition, used by advanced users staging a subsequent
  `HDToolBox` partition resize (front-pack one partition, middle-pack
  its neighbour, to open contiguous room for growing one at the
  other's expense) — the exact move this crate's own `resize()` +
  compaction is meant to enable per PLAN.md point 3.
- Directory entries are reorderable ("arrange the files within a
  directory in any arbitrary order," said to speed up Workbench icon
  display), but whether alphabetical sort is an actual mode or an
  emergent side effect of the chosen order **could not be confirmed**.
- A per-file "option files" override exists for "FileExt blocks"
  handling, plausibly a growth-reservation knob, but its exact
  semantics and default **could not be confirmed** from the material
  reached.
- OFS→FFS-with-dircache (`DOS\0`→`DOS\5`) conversion happens as a side
  effect of a reorg pass on some versions, consistent with the
  directory-locality-first philosophy, but specific claims about where
  dircache blocks land relative to the root are **inferred, not
  quoted**.

**Safety model — concrete and worth adopting a stance on.** ReOrg is
explicitly **not** claimed to be crash-safe during the block-move phase:

> "If ReOrg reports an error in the 'disk scan' phase, it has not
> destroyed or changed your disk yet... But when the 'moving blocks'
> phase starts, ReOrg must not be interrupted any more, because during
> this phase the disk layout is inconsistent."

Two-phase (read-only scan, then a non-atomic, non-resumable move), with
an explicit backup warning for the floppy-oriented low-spare-space mode.
This is the bar this crate's own compactor should clear *more* than
match: PLAN.md's own reasoning (copy-then-flip, mark-then-use) already
gives every intermediate state during compaction the same safety
property `Allocator` gives ordinary mutation — "the old volume plus a
leak" — which is strictly better than ReOrg's admitted "inconsistent,
do not interrupt" state. Worth stating in the implementation as a
deliberate improvement over the best-known prior art, not a novel claim
invented from nothing.

**No performance numbers found.** No vendor-published benchmark
("N minutes for M MB on hardware X") turned up in any reachable source.
One secondhand, unverifiable Usenet anecdote (28% reported fragmentation,
a 34-minute optimization pass, "visibly faster") is not worth citing as
a number, only as evidence that reorg passes were minutes-long on
period hardware.

### ABTools (Moonlighter) — thin, but a useful contrast

Known only through the "ReOrg vs ABTools" thread, itself only reachable
via search-engine paraphrase (Google Groups blocked direct fetches
throughout with HTTP 429). The one substantive, if unverified, claim: ABTools
measured fragmentation by **counting fragmented files**, a metric framed
in the thread as suited to an MS-DOS-style contiguous-allocation
filesystem and less meaningful for FFS — i.e. contemporaries were
already arguing, in public, that file-data-fragment-counting is the
wrong metric for this filesystem family. That argument is consistent
with ReOrg's own design thesis above. **Confidence: secondary,
low-to-moderate** — paraphrase of a paraphrase, but two independent
threads point the same direction.

### Quarterback Tools (Central Coast Software) — existence confirmed, mechanism not

Existed (product listings, 1991 date, primary but non-technical) and
Quarterback (the *backup* tool from the same company, a different
product) had its source GPL'd and archived via the Amiga Source
Preservation project — a promising unexplored lead if a future pass
wants a from-source-not-from-review answer, but Quarterback Tools'
optimizer specifically was not confirmed to be included, and the forum
threads that might resolve it (`forum.amiga.org`) returned HTTP 503
throughout this research pass. **Nothing about its actual block-level
policy could be established.**

### DiskOptimizer (Jörg Strohmayer, 1999)

A later, post-classic-era tool by an author who also wrote SmartFileSystem.
Aminet package metadata only ("optimize your partitions to speed up
directory-scan, file-access etc.," NSD/TD64-aware for large modern
drives). No algorithm, ordering rule, or safety/performance claim was
recoverable beyond that one marketing line.

### The opposite pole: AFSFileDefrag (Kirk Strauser)

Not FFS/OFS — for AFS (Ami-FileSafe) — but worth citing as a design
contrast, and confirmed from a primary Aminet readme. Its entire
algorithm: run `DiskValid`, parse its report for fragmented files, then
**copy each fragmented file to a temp location and copy it back**,
relying wholly on the underlying filesystem's own allocator to write it
out contiguously on the copy-back. No block-level surgery at all.
Explicit safety claim: "No files are deleted (except for the temp files,
after the program is done), and all operations are normal DOS calls."
This is the safety-by-delegation extreme (trading efficiency and free-space
headroom for using only ordinary filesystem operations) against which
ReOrg's direct-block-surgery approach, and this crate's planned
mark-then-use approach, can both be read as more sophisticated points on
the same spectrum.

## 2. What FFS's runtime behaviour rewards

Sources: AROS's `afs.handler` (`rom/filesys/afs/*` on
github.com/aros-development-team/AROS — read for behavioural facts and
cited, never copied, same discipline `allocator.rs` already uses for
Linux `affs`), read together with its git history; Linux `fs/affs`
(previously read for the M3 allocator survey); RKRM Devices' trackdisk
and scsi.device chapters (primary, Commodore, via the `rkrm-devices`
skill).

### FFS issues one I/O request per block, not batched multi-block reads

AROS's `getBlock()` (`rom/filesys/afs/cache.c`) called `readDisk(...,
blocknum, 1, ...)` — one block per call — for essentially its entire
history, and `readwriteDisk()` (`os_aros_support.c`) issues exactly one
`DoIO()` per call. This held regardless of whether the target block was
file data, a directory header, or a bitmap page: no batching by
contiguity anywhere in the read path. AROS is a from-scratch
reimplementation built for behavioural (not source) compatibility with
the real ROM FFS, so this is **inferred, not directly proven, for
Commodore's original 68k FFS** — but AROS having gone roughly three
decades without this optimization, then adding it abruptly (see below),
is circumstantial evidence the original never had it either.

One important, oddly-timed wrinkle: the research found an AROS commit
(`fb2d26dc`, message "afs-handler: read runs of blocks in one request")
adding a bulk-read-ahead path, dated within days of this research being
run. That date is close enough to "now" (repository history reaching
right up to the point of inquiry) that it deserves independent
verification — either by checking the commit directly against
`github.com/aros-development-team/AROS` or by treating it as **unverified
until re-checked**, rather than as settled fact. Either way it changes
nothing about the historical behaviour this document cares about: even
if genuine, it postdates every version of real FFS this crate targets
by ~35 years, and its existence as a *new* feature is itself evidence
that batching was absent before it.

**Consequence for a layout policy:** contiguity does not reduce the
*count* of I/O requests FFS issues while reading a file — it reduces
what each request physically costs once it reaches the device (seek
distance, and on floppies, whether the request has to trigger a fresh
track-buffer fill). A compaction or creation-time policy justified by
"fewer requests" would be reasoning about a benefit FFS's own read path
does not deliver; the real payoff is entirely in per-request physical
cost, covered in §3.

### `de_NumBuffers` / `AddBuffers` is a hold-cache, not read-ahead

AROS's `cache.c` allocates `numBuffers` fixed slots and evicts by a
`newness`-counter LRU scan; `getCacheBlock()` either finds the block
resident or evicts the least-recently-touched slot. There is no
prefetch logic tied to buffer count anywhere in that path prior to the
same recent patch mentioned above. This matches the plain-language
description found in secondary sources (buffer count trades RAM for
fewer re-reads of recently-touched blocks — directory chains, repeatedly
opened headers, the bitmap page — not for reading further ahead in a
file). **Confidence: primary for AROS's code, secondary corroboration
for the general description.**

Consequence: a layout policy that clusters directory metadata near the
root (or near each other) pays off specifically because that clustering
raises the hit rate of a bounded-size hold-cache during a directory
walk — the same blocks get touched repeatedly (every `Examine`, every
path lookup through a busy directory) and staying resident matters more
when they are few and close together than when the same working set is
scattered across the volume. This is a *cache-locality* argument, not a
seek-avoidance argument, and it applies even to media with no seek cost.

### FFS's own allocator does not try to preserve per-file contiguity

AROS's `bitmap.c`: a **single, volume-wide rover** (`volume->lastaccess`),
updated on every allocation *or* deallocation anywhere on the volume,
scanned forward from its current position (next-fit), wrapping at the
volume's data-area boundary. There is no per-file "goal" the way Linux
`affs` has one (already documented in `allocator.rs`'s own module
comment) — under AmigaOS's cooperative multitasking, any interleaved
write from another process (Workbench icon updates, another open file,
a `Snapshot`) pulls the shared rover away and back, breaking the
contiguity a serial write would otherwise get "for free." This is
**inferred for the real Commodore FFS**, not confirmed against its own
source, but it matches long-standing community folklore about why FFS
volumes fragmented in the first place, and explains why a whole
commercial-tool category (§1) existed to fix it after the fact.

**This directly complicates one of PLAN.md's framings.** The root
sitting at the volume midpoint is a real, structural, one-time
seek-minimization choice baked into the format itself (§4 confirms the
underlying seek/rotation physics justify it). But the *allocator* FFS
uses while writing is not similarly seek-aware — it is a bare next-fit
bitmap scan with no locality preference beyond incidental effects of
serial writes. So "FFS was designed around seek" is true at the format
level (root placement) and not true at the allocation-policy level
(block placement). This crate's `Populator` already produces contiguous
files by accident of its own forward cursor (documented in
`populate.rs`'s own module comment), which means its *current* output is
already better-behaved than stock FFS's own writer would produce under
any multitasking load — worth stating plainly rather than presenting
"contiguous by policy" as merely restoring what FFS always intended.

### The trackdisk track buffer: real, primary-sourced, floppy-specific

RKRM Devices, trackdisk.device chapter (primary, Commodore, 1991),
quoted directly:

> "Each disk drive on the system has its own buffer which holds the
> track data going to and from the drive. Normally, a read of a sector
> will only have to copy the data from the track buffer... All track
> boundaries are transparent to the programmer... The performance of
> sequential reads will be up to an order of magnitude greater than
> reads scattered across the disk."

> "If the desired sector is already in the track buffer, no disk
> activity is initiated. If the desired sector is not in the buffer,
> the track containing that sector is automatically read in."

This confirms, from Commodore's own documentation, that **intra-track
block order is irrelevant to read cost once the track is buffered** —
reading sectors 1, 5, 2, 9 off an already-resident track costs the same
as reading 1, 2, 5, 9. The unit that matters is the *track* (one
side of one cylinder — 11 sectors on a standard DD floppy), not strict
LBA ordering within it. This buffering is entirely inside
trackdisk.device and happens whether FFS asks for one block or all
eleven, so it delivers its benefit even under the one-request-per-block
regime described above.

**No documented equivalent exists for `scsi.device`.** The RKRM's
scsi.device chapter maps `CMD_READ`/`CMD_WRITE` straight onto SCSI
READ/WRITE with no described AmigaOS-side track or block cache; any
buffering for hard disks is internal to the physical drive's own
controller, invisible to and unspecifiable from the host side. The
track-buffer argument for "contiguous-within-a-cylinder is nearly free"
is therefore **a floppy-specific fact**, not a general Amiga storage
fact — carrying it over to hard disks or CF/SD without qualification
would be exactly the kind of cargo-culting PLAN.md asked this survey to
watch for.

## 3. What the media actually cost

### Mechanical drives

Track-to-track seek on Amiga-era and adapter-connected small hard
drives runs roughly 4–11 ms (Conner CP-3044: 8 ms; Quantum ProDrive
LPS240AT: 4 ms — spec-sheet figures, secondary/TULARC-derived, high
confidence as figures but drive-model-specific); full-stroke seek is
much larger (16–29 ms average on the same drives; ~170 ms on the
earliest ST-506-class MFM drives, secondary). Rotational latency is the
standard `30000/RPM` ms average-half-rotation figure — 8.3 ms at 3600
RPM, 4.2 ms at 7200 RPM. A head switch between platters at the *same*
cylinder is electronic and sub-millisecond to ~1.5 ms — much cheaper
than a track-to-track step, though the margin narrows on later
high-density drives with longer settle times. **Confidence: secondary,
spec-sheet-derived, high for the numbers, moderate for how well they
generalize across the whole "mechanical drive behind an adapter"
population this crate actually targets.**

The finding that should change how this policy is built: **RDB's
`rdb_Cylinders`/`rdb_Heads`/`rdb_Sectors` fields (and the matching
`PartitionBlock` fields) do not reflect real physical geometry for
anything but the earliest native Amiga drives.** wiki.amigaos.net's own
RDB documentation states this plainly — partitioning tools "since a long
time" choose values that multiply out to the disk's reported size, not
values tied to the platters underneath. This is consistent with the
wider PC/SCSI history (drives self-reporting translated, convenience CHS
numbers once they had onboard controllers) and is unconditionally true
of CF/SD, which have no physical cylinders at all. **Confidence:
primary-adjacent (official AmigaOS documentation), high** — treat the
negative conclusion (RDB geometry is not a reliable proxy for real seek
boundaries on IDE/SCSI/CF media) as settled, not merely likely.

**Consequence:** cylinder-alignment logic keyed off RDB fields is
sound-sounding but almost certainly cargo-cult for every medium this
crate targets except real floppies (which don't carry an RDB at all —
their geometry, 11 sectors/track × 2 heads × 80 cylinders, is a known
constant, not something read off the partition table). For mechanical
drives and CF/SD, the only thing worth optimizing directly is **block
contiguity and locality in LBA space** — minimizing run breaks and the
distance between related blocks — not alignment to a "cylinder"
computed from fields that do not correspond to anything physical.

### CF/SD via IDE/PCMCIA-CF adapter

No seek or rotational cost — solid state, trivially true. The
granularity that matters instead is the **erase block**: general NAND
flash erase blocks run roughly 128 KB–2 MB (secondary, LWN.net,
technical); SD cards specifically commonly report a 4 MB allocation
unit via `AU_SIZE`, with some parts using 16 MB (secondary/community —
Raspberry Pi forum and SD-card formatting guidance, moderate
confidence; the SD Association's own Physical Layer spec, which defines
the `AU_SIZE` field, could not be fetched directly in this pass).
Small, old CF cards of the kind the retro-hobbyist community
specifically favours (for period authenticity or low cost) likely have
smaller erase blocks than modern SD media, following the general
generational trend in flash geometry, but **no vendor datasheet for a
specific period-appropriate CF part was found** — this is an inferred
generalization, not a confirmed figure, and should not be quoted as a
number in code or documentation.

Whether Amiga-side CF/IDE adapters (A1200 IDE, third-party buffered
CF-IDE interfaces) do any read-ahead or command queuing of their own
**could not be established either way** — nothing found describes such
behaviour, and nothing found rules it out. Treat as unknown.

**Consequence:** contiguity still has value on flash, but at
erase-block granularity for *writes* (avoiding read-modify-erase-write
amplification and uneven wear) rather than at the block-order-within-a-run
granularity that matters for reads on spinning media. For *reads*
specifically, flash has no seek/rotational penalty at all, so ordering
sub-erase-block sectors buys essentially nothing — a contiguous 100-block
run reads only marginally faster than the same 100 blocks scattered, once
per-request overhead is small relative to transfer time. This is the
clearest case in the whole survey of an optimization ("contiguous helps
reads") that transfers from mechanical media to flash in name only,
transformed into a different concern (write locality, not read locality)
that this crate's read-oriented layout policy does not currently need to
solve.

### Real floppies

RKRM Devices (primary, quoted in full in §2) confirms the track buffer
directly: reads within an already-buffered track are free regardless of
sector order; the unit that matters is the track. Step timing —
Commodore's Hardware Reference Manual reportedly specifies a 3 ms
worst-case step and a 15 ms settle time, with an 18 ms minimum
direction-reversal delay — but this could only be recovered via a
search-engine relay of the primary document (the mirror's TLS
certificate has expired), so **treat these three numbers as needing
independent re-verification** before being quoted as settled facts, even
though they are plausible and internally consistent with the drive
class.

**Consequence, and the one place PLAN.md's own framing needs a small
correction:** PLAN.md states "a file laid contiguously inside one
cylinder costs zero head steps to read." That is right about *steps* —
switching between the two tracks of one cylinder needs no actuator
movement — but it is not quite right about *cost*: trackdisk.device's
track buffer holds one track at a time, so crossing from one track of a
cylinder to its paired track still forces a fresh track read (a new
~200 ms-class rotation, at 300 RPM, to fill the buffer), even though no
head-stepping occurs. "Zero head steps" is accurate; "zero cost" would
overstate it. The real optimization unit for floppies, confirmed by
primary source, is smaller than "cylinder": it's the **track** (11
blocks on DD). A layout policy that keeps a file within one track
before spilling to the next gets the full benefit RKRM documents;
keeping it within one cylinder (22 blocks, two tracks) gets a smaller,
second-order benefit (no seek, but still a second buffer fill) on top
of that.

### Ranking: which optimizations transfer where

| optimization | floppy | mechanical HDD | CF/SD |
|---|---|---|---|
| block contiguity (minimize run breaks) | full — RKRM-confirmed, order-free within a track | real, physics-backed, but only reachable via raw LBA proximity, not RDB "cylinders" | reads: marginal; writes: matters at erase-block granularity, a different problem |
| cylinder/track alignment computed from RDB geometry | N/A — floppies carry no RDB; use the fixed 11×2×80 constant instead | **cargo-cult** — RDB fields are software-chosen since the embedded-controller era, not physical | **cargo-cult** — no physical cylinders exist at all |
| clustering metadata near root for cache locality (independent of seek) | applies | applies | applies — this is the one policy that transfers unconditionally, because it is a cache-hit-rate argument, not a seek argument |

## 4. What this means for PLAN.md's assumptions

PLAN.md's premises hold up better than a "just confirm what we already
believed" survey would suggest, but three refinements are warranted,
not zero:

1. **"Zero head steps" for a cylinder-contiguous floppy file is correct
   about steps, but the track buffer's granularity is the *track*
   (11 blocks), not the cylinder (22 blocks) — see §3.** A policy
   should target track-sized runs on floppies, treating "same cylinder"
   as a smaller secondary benefit, not the primary unit.
2. **RDB geometry cannot be used to find real cylinder boundaries on
   hard disks or CF/SD — it hasn't been physically meaningful "for a
   long time," per AmigaOS's own documentation.** Any cylinder-alignment
   reasoning this crate implements for those media would be decorative.
   The floppy case is different and safe, because its geometry is a
   known constant that never needs to be read from an RDB.
3. **FFS's own writer is not seek-aware at the block-allocation level** —
   only the *root's placement* is a deliberate, format-level
   seek-minimization choice; the day-to-day bitmap allocator (as
   observed via AROS, and consistent with why a commercial defrag-tool
   market existed at all) is a bare next-fit scan with no per-file
   locality preservation. This crate's `Populator` forward cursor
   already does better than that by accident. The creation-time policy
   this document recommends (§6) should be understood as *improving on*
   stock FFS behaviour, not merely reproducing an intent FFS itself
   never fully carried out at the allocator level.

Nothing surveyed contradicts the *root-at-midpoint* reasoning itself —
if anything, the seek/rotation figures in §3 make it a stronger claim
than PLAN.md states it as, since even a modest fixed seek cost
(4–11 ms track-to-track on mechanical media) compounds fast over
thousands of metadata touches during a session.

## 4a. Measured: what fragmentation actually costs a real ROM

Everything above about runtime behaviour is inference from AROS as a
proxy. This section is not: it is a measurement against a genuine
Kickstart 3.1 (A1200) reading a floppy, run under Copperline, whose
emulation is deterministic — so the numbers are exact and reproducible
rather than a benchmark with error bars.

`examples/frag-bench.rs` builds two `DOS\3` DD floppies that differ in
**nothing but block placement**. Both hold the same 200 000-byte file
(391 data blocks) with identical bytes. In one, `Populator`'s forward
cursor lays the data down as 6 ascending runs; in the other the same
file is threaded through holes left by creating 180 small files and
deleting every second one — wear, not sabotage, and the shape a volume
takes after months of use. Total in-order head travel: 395 blocks
against 790.

Then Workbench 3.1 boots from `df0`, and an AmigaShell script stamps
`date`, copies the file off `df1` to `ram:`, and stamps `date` again
(typed while the copy runs, so the console buffer makes the second
stamp the copy's own end time):

| layout | runs | start | end | elapsed |
|---|---|---|---|---|
| contiguous | 6 | 12:42:58 | 12:43:17 | **19 s** |
| fragmented | 83 | 12:42:58 | 12:43:26 | **28 s** |

**47% slower, for the identical file.** The start stamps agree to the
second across two separate emulator runs, which is the determinism
doing its job: the only variable was the layout.

What this establishes and what it does not:

- **Establishes** that layout has a large, measurable cost through the
  real ROM's own FFS on the most seek-sensitive medium. A policy that
  keeps files in a handful of runs is worth having; this is no longer
  reasoned from first principles.
- **Consistent with** §2's one-request-per-block conclusion — a
  filesystem that batched contiguous runs would show a wider gap than
  47%, since the contiguous case would also be saving requests.
- **Does not** transfer the *ratio* to hard disks or CF. Floppy is the
  extreme: a head step is milliseconds and the track buffer is only 11
  blocks. Mechanical drives will show less, CF/SD less again, and
  measuring those needs a different rig than an ADF in `df1`.
- **Does not** isolate seek from rotational latency, nor tell us
  anything about metadata scatter — which, per §1, ReOrg's author
  considered the *dominant* cost. That experiment is the obvious next
  one: two volumes differing only in how far directory and header
  blocks sit from the root, same files, same data layout.

Reproduce with `cargo run --example frag-bench -- outdir`, then boot
each image under Copperline with a real Kickstart and the script shape
above.

**Addendum, wave 1 implementation (2026-09-08).** The same rig was used
again, mid-implementation, to settle a placement question this section
does not answer on its own: where should a `T_LIST` extension block go
once directory metadata is clustered near the root? Two builds of the
identical 391-block file were compared, both otherwise using the
policy's own metadata/data cursor split — one with extension blocks
allocated near the root alongside the file's header, one with them left
at their natural position in the data stream (between the last data
block of the table they close and the first of the table they open).
Root-clustered: 21 s. Interleaved: 19 s, matching the original
contiguous number in the table above. The regression makes sense in
hindsight: unlike a header (read once, at open) or a dircache block
(read during a walk with no file read in flight), an extension block is
fetched *mid-stream* by a reader already reading the file, so pulling it
to the root inserts two long seeks into every extension-block boundary
instead of zero. Extension blocks stay interleaved with file data;
headers, dircache and comment-overflow blocks stay near the root.

**Addendum, wave 3 implementation (2026-09-08).** `examples/frag-bench.rs`
gained a fourth image, `reorged.adf`: `fragmented.adf`'s own 391-block
file, copied out to host memory, deleted and recreated through `Mutator`
with the wave-3 layout policy at its default (on) — nothing else done by
hand. This is the PFS2DefragTry pattern (§4a's own subject throughout,
credited in PLAN.md) run through this crate instead of through a real
ROM's FFS. Block-level result, one run, matching `defragmented.adf`
exactly (`defragment_file`'s own explicit tier-1 pass):

| image | runs | first | last | head travel |
|---|---|---|---|---|
| contiguous | 1 | 2 | 397 | 395 |
| fragmented | 80 | 883 | 1673 | 790 |
| defragmented | 1 | 24 | 419 | 395 |
| reorged | 1 | 24 | 419 | 395 |

`reorged.adf` lands on the *same* extent `defragmented.adf` does — not a
coincidence: both start from the identical fragmented image and both ask
the same allocator for one contiguous run near the file's own header, so
they find the same hole. This is the block-level half of the claim this
section's own §4a table already measured a real ROM's timing for; the
real-ROM half — booting `reorged.adf` next to `contiguous.adf` and
`fragmented.adf` under the same script — was not repeated this wave
(§4a's own contiguous/fragmented numbers already establish the timing
gap a run-count difference like this one implies, and `reorged.adf`'s
run count is identical to `defragmented.adf`'s, which *was* run-count-
verified against `defragment_file`'s own explicit report). Reproducing it
is the same recipe §4a already documents, against `reorged.adf` in place
of `fragmented.adf`.

## 5. What could not be established

- Whether the real Commodore ROM FFS batches contiguous block reads
  into fewer device requests, or matches AROS's historical
  one-request-per-block behaviour exactly. No source examined the
  binary FFS itself; the AROS evidence is inference by proxy.
- ~~The exact date and authenticity of AROS commit `fb2d26dc`~~
  **Resolved, 2026-09-08.** The commit is genuine: `fb2d26dc`,
  `afs-handler: read runs of blocks in one request`, authored
  2026-09-01, verified through the GitHub commit-search API against
  `aros-development-team/AROS`. The suspicion was reasonable and the
  answer is simply that it is *recent* — a week old at the time of
  writing. Two consequences, and they pull in opposite directions:
  historical AROS (and, by the proxy argument, plausibly the ROM FFS
  it reimplements) did issue one request per block, so the survey's
  §2 conclusion stands for every system in the field today; but
  current AROS batches contiguous runs, which means on that target
  contiguity now buys request-count reduction *as well as* the
  physical savings, and the effect will grow as the change propagates.
  That someone judged the batching worth implementing in 2026 is
  itself weak evidence that contiguity pays measurably.
- Whether ReOrg places a file's header block *immediately adjacent* to
  its first data block, or merely reduces the statistical distance
  between header blocks generally. Only the latter (header-to-header
  distance as the fragmentation metric) was directly quoted.
- ReOrg's exact data-block ordering rule (strictly ascending LBA, or
  something else).
- Whether ReOrg sorts directory entries alphabetically as a distinct
  mode, versus exposing arbitrary manual reordering only.
- The semantics and default of ReOrg's per-file "FileExt blocks"
  option — plausibly a growth-reservation knob, not confirmed.
- Quarterback Tools' optimizer algorithm, safety model, or performance
  at any technical level — only its existence as a product was
  confirmed. (Its source may exist in the same GPL release as
  Quarterback the backup tool; not confirmed either way, and the forum
  threads that might resolve it were unreachable — HTTP 503 — during
  this pass.)
- ABTools' actual placement algorithm and safety model — only a single
  secondhand methodological claim (fragmented-file-count metric) came
  through; the primary Usenet thread was unreachable directly.
- DiskOptimizer's (Strohmayer, 1999) block-level mechanism — marketing
  description only.
- Any vendor-published, hardware-specific performance benchmark for any
  tool surveyed.
- Exact erase-block size for period-appropriate, small, old CF cards
  specifically (as opposed to modern SD cards, where a 4 MB figure has
  moderate secondary support). Only a generational trend was inferred.
- Whether Amiga-side CF/IDE adapters do read-ahead or command queuing
  of their own, independent of the card.
- Precise Amiga floppy step/settle/direction-reversal timings (3 ms /
  15 ms / 18 ms) — plausible and internally consistent, but relayed
  through a search-engine cache of a now-unreachable primary mirror
  rather than fetched directly; worth re-verifying against RKRM
  Hardware Manual or a drive datasheet (e.g. Chinon FB-354) before
  treating as exact.
- Whether real ROM FFS's block allocator uses a single global rover
  (as AROS's does) or something with more per-file locality awareness —
  inferred from AROS plus community folklore about fragmentation, not
  confirmed against Commodore's own source.

## 6. Recommendations

Each item below is tagged **[evidence]** where a cited source drives it
directly, or **[reasoned]** where it follows from this crate's own
already-documented design (the M3 allocator's mark-then-use discipline,
`resize()`'s copy-then-flip ordering, `canonical_root_lba`) rather than
from an external source.

### 6a. Creation-time policy (`format` / `Populator`)

1. **[evidence]** Cluster directory metadata — headers, hash tables
   (inherent in the header block), dircache blocks, and comment
   overflow blocks — near the root, ahead of raw file-data contiguity.
   This is ReOrg's own stated priority (§1) and independently justified
   by the `AddBuffers` hold-cache argument (§2): the working set a
   directory walk touches repeatedly is small and revisited often, so
   keeping it close together (in LBA terms, so it stays resident in a
   bounded buffer pool and near the root physically) pays on every
   medium, including flash, where seek arguments don't apply at all.
2. **[reasoned]** Concretely: split `Populator`'s single forward cursor
   into two regions. A **metadata cursor** anchored near
   `canonical_root_lba` (the root's own position, already computed by
   `format`) allocates directory headers, hash-table-carrying blocks,
   and dircache blocks. A **data cursor** — the existing forward-cursor
   behaviour, unchanged — allocates file content. This mirrors ReOrg's
   own "directory area and file area stored consecutively" free-space
   mode (§1) rather than inventing a new scheme. The two-cursor split is
   a straightforward generalization of `Allocator::allocate_near`'s
   existing hint mechanism (`allocator.rs`), which already documents the
   hint as "the file's previous data block, or its header" — extending
   that to "a new directory's hint is its parent's header LBA, and the
   root's own children hint to the root" requires no new primitive, only
   `Populator` choosing hints instead of always using its bare forward
   cursor.
3. **[reasoned, already true]** File-data-adjacent-to-header is
   *already* a property of the current forward-cursor design and needs
   no new work: `populate.rs`'s own module documentation states data
   blocks (then extension blocks) are allocated and written before the
   header, in the same forward pass — so a file's header LBA already
   lands immediately after its own data, for exactly the same reason
   the whole file is already contiguous. Keep this; state it as policy
   rather than accident, per PLAN.md's own framing, but no code change
   is needed for this part specifically.
4. **[evidence]** Do not build RDB-geometry-derived cylinder-alignment
   logic for hard-disk or CF/SD targets. §3 establishes those fields are
   not physically meaningful for any medium this crate is likely to
   touch. If floppy-specific alignment is wanted, use the fixed,
   known-at-compile-time floppy geometry (11 sectors/track, 22
   blocks/cylinder on DD) directly — it is a format constant, not
   something to read from a partition table, and floppies do not carry
   an RDB to read it from regardless.
5. **[reasoned]** No attempt to reserve contiguous headroom for future
   file growth at creation time. `Populator` never mutates after
   `finish()` (its own module documentation is explicit about this — no
   append, no second session), so "growth space" has no addressee: any
   space reserved now for a hypothetical future write would just be
   wasted space in the volumes this crate actually produces. This is a
   deliberate divergence from ReOrg's "FileExt blocks" option (§1,
   §5) — appropriate for a write-once populator, not necessarily for a
   future milestone that supports append.

### 6b. Compaction pass — two tiers, in this order

1. **[evidence]** Sequence compaction with the ReOrg priority, not the
   naive one: **directory/header locality first, file-data contiguity
   second.** A compactor that only defragments file data and leaves
   directory headers scattered is optimizing the thing ReOrg's own
   author found didn't matter much and skipping the thing that did.
2. **Tier 1 — data blocks only.** For a file whose data blocks are
   non-contiguous or far from its header, relocate them with the
   existing `Allocator` discipline already built for this crate's other
   mutating operations: `allocate_near(hint)` with the hint set to the
   header's LBA (or the previous data block's LBA, matching the pattern
   `allocator.rs` already documents from Linux `affs`), write content at
   `Allocation::block()`, `flush()` the bitmap page, then use
   `Allocation` only once `reference()` confirms it is durable to
   re-point the file's own data-pointer table or extension-block chain,
   then free the old block. **[reasoned]** — this is exactly the
   crash-safety shape `resize.rs`'s own module documentation already
   states for root relocation ("copy-then-flip... every intermediate
   state is the old volume plus a leak"), applied to a file's own
   metadata instead of the root. Touches only that file's header and
   extension tables; no hash chain, no parent, no dircache — the tier
   PLAN.md correctly identifies as "most of the benefit" for the least
   risk.
3. **Tier 2 — header relocation.** Requires re-pointing the parent's
   hash-chain slot (or the previous link in a collision chain), every
   child's `parent` longword if the relocated header is itself a
   directory, `real_entry`/`next_link` chains for hard links, and the
   owning directory's dircache record. **[reasoned]** — `resize.rs`
   already implements the general shape of this for the one case that
   currently exists (moving the root: re-parenting children, relocating
   the dircache chain, patching the boot-block advisory pointer, all
   under the same "first write forces `bitmap_flag = 0`, last write sets
   it back to −1" ordering). Generalizing that machinery to move an
   arbitrary header, not just the root, is squarely `Mutator` territory
   (already named as such in `resize.rs`'s own documentation of what it
   deliberately does *not* reach into) — this tier is where the
   bug-prone surface area PLAN.md anticipated actually lives, matching
   PLAN.md's own risk assessment.
4. **[evidence]** Adopt a ReOrg-style two-phase run (read-only scan that
   is always safe to abort, then a move phase) as the operation's outer
   shape, but do **not** adopt ReOrg's admitted non-atomicity for the
   move phase — the mark-then-use discipline in (2) above already gives
   every step of the move phase itself the "old volume plus a leak"
   safety property. Document this explicitly as improving on the best
   documented prior art rather than merely matching it, since it is a
   real, citable difference (§1) and not a marketing claim.
5. **[reasoned]** Expose where the compactor leaves its reclaimed free
   space — trailing the metadata region, trailing the data region, or
   packed toward one end — the way ReOrg exposed it as a user choice
   (§1). This is what lets a caller run compaction specifically to set
   up a subsequent `resize()` shrink: PLAN.md's point 3 is that
   `RootTargetOccupied` and the doubled `minimum_size()` floor on a
   volume "packed from `reserved` upward" are exactly what relocation
   fixes, so the compactor's own layout choice should default to
   leaving free space contiguous *at the end*, which is precisely what
   makes a following `resize()` call cheap and likely to succeed against
   the floor `minimum_size()` reports as achievable-in-principle.
6. **[evidence]** Do not justify either tier by "fewer I/O requests" —
   §2 establishes FFS issues one request per block regardless of
   contiguity, so that's not a real saving on this filesystem's own
   read path. The only real savings are physical: seek/rotation cost on
   mechanical media (real, per §3), track-buffer hit rate on floppies
   (real and primary-sourced, per §2/§3), and cache-hit-rate for the
   bounded `AddBuffers`/dircache-scan working set (real, applies even to
   flash). State the justification in those terms so a future
   contributor doesn't "optimize" the compactor toward request-count
   reduction, which this filesystem's runtime doesn't reward.
