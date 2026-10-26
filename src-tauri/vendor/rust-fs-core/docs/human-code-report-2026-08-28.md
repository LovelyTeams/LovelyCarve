# Human-code report — 2026-08-28

> **This is analysis only. No code was modified.** No files were changed, no
> branches created, no commits made. The working tree contains exactly one new
> file: this report. Every "shape of the fix" below is a proposal awaiting your
> confirmation, not a description of something already done.

**Scope:** the whole crate — `src/` (10 files, 1,329 production lines),
`tests/` (12 files, 1,812 lines), plus `README.md`, `include/fs_core.h`.

**Counts:** 19 findings — **4 High, 9 Medium, 6 Low**. 0 fixed, 19 awaiting
confirmation.

**Test baseline:** `cargo test` run at the start of this pass — **129 passed,
0 failed**, plus 1 ignored doc-test (41 unit in `src/`, 88 integration across
12 files in `tests/`). CI gates `cargo clippy --locked --all-targets -- -D
warnings`, `cargo fmt --check`, and `cargo llvm-cov --fail-under-lines 90` on
three platforms.

---

## Reading note: why this crate gets a stricter review than its size suggests

`am-fs-core` is a dependency of every filesystem driver and disk-image reader
in the family — `fs-ext4`, `fs-ntfs`, `fs-xfs`, `fs-btrfs`, `fs-erofs`,
`fs-squashfs`, the `img-*` readers, `am-partitions`. A confusing abstraction
here is not confusing once; it is confusing in seven places, and each of those
seven has to decide independently whether the confusion is intentional.

That changes what counts as "High". A magic number in a driver costs one
reader ten minutes. An inconsistent error vocabulary here costs seven driver
authors a design decision each, and the seven decisions will not agree.

The previous review (`docs/code-quality-review-2026-08-25.md`) found 0 High,
1 Medium, 2 Low and concluded this was "the cleanest crate in the family."
That remains true of the *local* qualities it measured — function length,
nesting depth, duplication within a function, `#[allow]` count. This pass
weights cross-crate consequences more heavily and looks at `README.md` and
`include/fs_core.h` as part of the crate's surface, which is where three of
the four High findings come from. The two reviews do not contradict each
other; they are measuring different things.

`src/ffi.rs` has also grown from 487 lines to 677 since that review, which is
where several new Medium findings live.

---

## Findings

### High

---

#### H1 — The same bounds condition produces two different error variants, and `OutOfBounds` is unreachable from every read path in the crate

**Category:** misleading names / comments that lie
**Severity:** High
**Files:**
- `src/slice.rs:55-65` — `SliceReader::read_at`
- `src/slice.rs:109-119` — `OwnedSlice::read_at`
- `src/slice.rs:162-172` — `OwnedRwSlice::read_at`
- `src/slice.rs:184-194` — `OwnedRwSlice::write_at`

All four sites evaluate the identical predicate:

```rust
offset.checked_add(want).map(|e| e > self.length).unwrap_or(true)
```

The three read sites then return:

```rust
Err(Error::ShortRead { offset, want: buf.len(), got: 0 })
```

and the one write site returns:

```rust
Err(Error::OutOfBounds { offset, len: want, size: self.length })
```

Two problems compound here.

**First, `got: 0` is not a measurement.** `Error::ShortRead` is documented at
`src/error.rs:11` as "Device returned fewer bytes than requested before EOF" —
a *partial* read. `FileDevice::read_at` (`src/file_device.rs:58-62`) uses it
correctly: `got: total` is the number of bytes actually transferred before the
device ran dry. The slice adapters never call the parent at all; they reject
the request up front. The `0` is a placeholder chosen to fill a field the
error shape demands, and a reader who trusts the variant's documentation will
read it as "the device had nothing left," which is a different fact.

**Second, `Error::OutOfBounds` is documented at `src/error.rs:19` as "Read or
write past the end of the device" — and no read path anywhere in the crate
ever constructs it.** Verified: the only construction site in `src/` is
`src/slice.rs:189`, inside `write_at`. So the read half of the abstraction
declines to use the variant that was designed for it, while the write half of
the *same struct*, guarded by the *same predicate*, uses it.

**Why this is High rather than cosmetic.** A driver author reads
`src/error.rs`, sees `OutOfBounds { offset, len, size }` described as covering
reads, and writes:

```rust
match dev.read_at(off, buf) {
    Err(Error::OutOfBounds { .. }) => /* clamp and retry */,
    ...
}
```

That arm is dead code. It will never match, on any device, through any
adapter. The request falls through to whatever the `ShortRead` arm does —
which for a driver is usually "the image is truncated, abort the mount" rather
than "I asked for too much, ask for less." Seven drivers each get to discover
this independently.

`src/stream.rs:119-133` then hides the discrepancy at the one boundary where
it would otherwise be visible: `fs_core_error_to_io` maps *both* `ShortRead`
and `OutOfBounds` to `io::ErrorKind::UnexpectedEof`, so anything reading
through `BlockReadStreamer` cannot tell the two apart even in principle.

**Test coverage: complete, and that is the complication.** The current
behaviour is asserted in six places:
- reads → `ShortRead`: `src/slice.rs:252-261`, `tests/slice_rw.rs:117`,
  `tests/ffi_slice.rs:147`
- writes → `OutOfBounds`: `src/slice.rs:353-361`, `tests/slice_rw.rs:83`,
  `tests/slice_rw.rs:158`, `tests/ffi_slice.rs:111`

So this is not an untested corner that drifted. It is a deliberate-looking,
test-locked contract — which means changing it is a semver-visible behaviour
change for all seven consumers and for the C ABI (`FS_CORE_SHORT_READ = 2`
would become `FS_CORE_OUT_OF_BOUNDS = 4` for over-reads). **This is exactly
why it is reported rather than fixed.** It needs your decision, not a
refactor.

**Shape of the fix, if you want one.** Three options, in increasing order of
disruption:

1. *Document the split.* Add to `src/error.rs` that `ShortRead` means
   "rejected or truncated read" and `OutOfBounds` means "rejected write," and
   drop `got` from the slice sites' mental model by noting it is always `0`
   for a pre-flight rejection. Cheapest; leaves the asymmetry but stops it
   being a surprise. No behaviour change, no test change.
2. *Add a variant.* Introduce `Error::RangeRejected { offset, len, size }` for
   pre-flight rejection on both sides, leaving `ShortRead` to mean only what
   `FileDevice` uses it for. Additive to the Rust enum; needs a new C code
   (`FS_CORE_RANGE_REJECTED = 9`), which is additive to the header too.
3. *Unify on `OutOfBounds`.* Make the three read sites return `OutOfBounds`.
   Smallest diff, cleanest result, but it is a breaking change to a published
   ABI and six tests have to be rewritten.

---

#### H2 — Three slice types are the same type three times; the bounds-check-and-rebase block is copy-pasted four times

**Category:** duplicated code
**Severity:** High
**Files:** `src/slice.rs:26-208`

`SliceReader` (26), `OwnedSlice` (82) and `OwnedRwSlice` (135) differ in
exactly two respects: how they hold the parent (`&'a dyn BlockRead` /
`Arc<dyn BlockRead>` / `Arc<dyn BlockDevice>`), and whether writes propagate.
Everything else is triplicated verbatim:

- the `start: u64, length: u64` fields — 3x
- the `new(parent, start, length)` constructor — 3x
- `start()` and `length()` accessors — 3x each
- `size_bytes() -> self.length` — 3x
- the 12-line bounds-check-then-rebase body of `read_at` — 3x, plus a fourth
  near-copy in `write_at`

Roughly 90 of the file's 209 production lines are copies. Four instances of
the bounds check is well past the skill's three-instance extraction threshold.

The cost is not the line count. It is that a reader who has understood
`SliceReader` has no way to know whether `OwnedSlice` is the same logic or
subtly different logic, and must diff them by eye to find out. And a future
fix to the bounds check — including any resolution of H1 — has to land in
four places, with nothing to catch the one that gets missed.

There is already evidence of the drift this invites: `OwnedRwSlice::write_at`
checks `self.parent.is_writable()` *after* the bounds check
(`src/slice.rs:195-197`), so an out-of-range write to a read-only parent
reports `OutOfBounds` rather than `ReadOnly`. That may well be intended; it is
just not a decision any of the other three copies participated in.

**Test coverage: strong.** 9 unit tests in `src/slice.rs:236-369`, 8 in
`tests/slice_rw.rs`, 10 in `tests/ffi_slice.rs`, and 3 more in
`tests/composition_stacks.rs`. This is the best-covered code in the crate,
which makes it the safest thing here to refactor.

**Shape of the fix.** A private `struct SliceGeometry { start: u64, length: u64 }`
with one `fn rebase(&self, offset: u64, len: usize) -> Result<u64>` that
performs the check and returns the parent offset. Each of the three types
holds one and delegates. The three public types, their names, and their
signatures stay exactly as they are — this is internal only, no API change.
Note that doing this *first* would collapse H1 from four sites to one, making
H1 a one-line decision afterwards.

---

#### H3 — `README.md` omits four of the ten modules and lists two shipped types as unimplemented

**Category:** comments that lie
**Severity:** High
**Files:** `README.md` (Layout section; Roadmap section)

The `## Layout` block lists six files:

```
src/
  lib.rs  error.rs  block.rs
  file_device.rs  callback_device.rs  caching_device.rs
tests/
  cache.rs            CachingDevice + interop tests
```

`src/` actually contains ten. Missing: **`ffi.rs`** (677 lines — the largest
file in the crate and the entire C ABI, which is how Swift, Go and C consumers
reach this crate at all), **`readonly.rs`**, **`slice.rs`**, **`stream.rs`**.
`tests/` contains twelve files, not one.

Worse, the `## Roadmap` block is headed "Planned additions (not yet
implemented)" and its first two entries are:

- `SliceReader` — "currently lives in `am-partitions` … Will move here"
  → it is here, at `src/slice.rs:26`, and has been for at least two releases
- `ReadOnlyDevice<T>` — "wrapper that takes any `BlockRead` … and rejects
  writes"
  → it is here, at `src/readonly.rs:18`, fully tested in
  `tests/readonly_wrapper.rs`

(The remaining two roadmap entries, `Logger` and `IoStats`, are genuinely
unimplemented and correct as written.)

**Why this is High.** This crate's entire value proposition is being a stable,
legible API that seven other crates can build against without reading its
source. The README is the first artifact a consumer author reads and often the
only one. It currently understates the crate by four modules — including the
FFI surface, which is the *only* way a Swift FSKit extension can use any of
this — and actively tells a reader that two shipped, exported, tested types
do not exist yet. A driver author who trusts it will go and reimplement
`SliceReader` in their own crate, which is precisely the outcome the roadmap
entry was written to prevent.

**Test coverage:** n/a — documentation. Nothing in CI checks that the README's
file list matches `src/`, which is why it drifted silently.

**Shape of the fix.** Update both blocks to match reality. Ten minutes, zero
risk, no code touched. Optionally add a CI step that diffs the Layout list
against `ls src/*.rs` so it cannot drift again.

---

#### H4 — An empty `impl BlockDevice for X {}` is load-bearing, and is indistinguishable from a forgotten one

**Category:** speculative/defensive code; misleading abstraction
**Severity:** High
**Files:**
- `src/block.rs:30-47` — the trait and its defaults
- `src/slice.rs:77`, `src/slice.rs:129`, `src/readonly.rs:51` — the three
  empty impls that depend on them

`BlockDevice` supplies defaults for all three of its methods
(`src/block.rs:33-46`):

```rust
fn write_at(&self, _offset: u64, _buf: &[u8]) -> Result<()> { Err(Error::ReadOnly) }
fn flush(&self) -> Result<()> { Ok(()) }
fn is_writable(&self) -> bool { false }
```

Three sites in this crate rely on that entirely, writing `impl BlockDevice for
X {}` with an empty body as a deliberate statement meaning "this device is
strictly read-only." `src/readonly.rs:48-51` even documents the intent well.

The problem is what the same design does in a *consumer* crate. A driver
author implementing a writable device writes `write_at`, tests that writes
land, and ships. If they omit `is_writable`, they inherit `false` — silently,
with no compiler warning, because the trait has a default. The device now
writes correctly and reports that it cannot write.

That is not a hypothetical failure mode, because `src/block.rs:42-43`
specifies what `is_writable` is *for*:

> Whether `write_at` is likely to succeed. Mount paths use this to decide
> whether to attempt journal replay or stay strict-read-only.

So the consequence of the omission is a volume that mounts read-only and skips
journal replay, on a device that was perfectly writable — a silent
capability downgrade, surfacing far from its cause, in whichever of the seven
drivers made the mistake. `OwnedRwSlice::write_at` (`src/slice.rs:195`) and
`CachingDevice` (`src/caching_device.rs:107`) both propagate `is_writable`
from their parent, so one wrong leaf poisons the whole stack.

The reverse ambiguity is just as costly for a reader: encountering `impl
BlockDevice for Foo {}` in a driver, there is no way to tell "deliberately
read-only" from "half-finished" without reading the rest of the file.

**Test coverage: the defaults are covered; the footgun is not, and cannot be
from inside this crate.** `src/readonly.rs:96-104`, `src/slice.rs:277-282` and
`tests/block_forwarding.rs:101-109` all assert the defaults behave as
documented. Nothing tests — nothing here *can* test — a consumer that
implements `write_at` and forgets `is_writable`.

**Shape of the fix.** Options, none of them free:

1. *Documentation only.* Add to the `BlockDevice` doc comment: "If you
   override `write_at`, you must also override `is_writable` — the default is
   `false` and nothing will warn you." Cheapest, and captures the invariant
   where a driver author will actually read it.
2. *A marker type.* Introduce `pub struct ReadOnly;` and have the three
   deliberate sites write `impl BlockDevice for X { /* read-only: see ReadOnly */ }`
   — or better, a `read_only_device!(X)` macro — so intent is stated rather
   than inferred from absence.
3. *Remove the `is_writable` default*, forcing every implementor to state it.
   Correct, and a breaking change for all seven consumers.

Given the memory note that architecture decisions on this project have been
lost across long gaps, option 1 has value beyond its cost: it writes down a
rule that currently exists only in whoever wrote the trait.

---

### Medium

---

#### M1 — The pointer-returning half of the C ABI hand-rolls `ffi_guard` four times

**Category:** duplicated code
**Severity:** Medium
**Files:** `src/ffi.rs:270-298`, `355-415`, `438-452`, `469-480`

`ffi_guard` (`src/ffi.rs:119-136`) exists precisely to wrap a body in
`catch_unwind`, map errors to codes, and stash the message. It serves the
functions that return `FsCoreErrorCode`.

The four functions that return `*mut FsCoreDevice` cannot use it — it returns
a code, not a pointer — so each one reimplements the same epilogue:

```rust
match res {
    Ok(p) => p,
    Err(panic) => {
        set_last_error(panic_message(&panic));
        ptr::null_mut()
    }
}
```

Identical in all four, plus the matching `let res = std::panic::catch_unwind(
AssertUnwindSafe(|| ...))` prologue. Four instances, past the extraction
threshold.

A reader scanning `ffi.rs` sees `ffi_guard` used in three places and
open-coded in four others, and must check each open-coded copy to confirm it
is not doing something different. (They are all the same. Confirming that took
longer than it should.)

**Test coverage:** the NULL paths are covered — `tests/ffi_slice.rs:163,175`,
`src/ffi.rs:532-538,670-676`. The panic paths are not covered anywhere, which
is a coverage gap worth noting on its own: the `Err(panic)` arm is dead in
tests in all four copies plus `ffi_guard` itself.

**Shape of the fix.** `fn ffi_guard_ptr<T>(body: impl FnOnce() -> *mut T) ->
*mut T` alongside the existing guard. Purely internal — no exported symbol,
signature, or ABI changes.

---

#### M2 — A five-line explanation sits above the wrong function

**Category:** comments that lie
**Severity:** Medium
**Files:** `src/ffi.rs:331-335`

```rust
// `*mut c_void` is `!Send + !Sync` by default and `unsafe impl Send` on a
// NewType doesn't propagate cleanly through closure auto-traits. Round-trip
// the pointer through `usize` instead — that's `Copy + Send + Sync`, and
// the callback contract already puts the host on the hook for thread-safe
// `ctx` use.
fn cb_io_err(rc: c_int, op: &str) -> io::Error {
```

`cb_io_err` formats an error message from a return code. It does not touch
`ctx`, pointers, `Send`, `Sync`, or closures.

The code this paragraph explains is thirty lines further down: `let ctx_addr =
cfg.ctx as usize;` (`src/ffi.rs:366`) and its three re-materialisations at
`370`, `380` and `391`. Those four lines are the most surprising in the file —
casting a pointer to an integer and back looks like exactly the kind of thing
a later reader "cleans up" — and they currently carry no explanation at all,
while the explanation is attached to the one function nearby that needs none.

**Test coverage:** n/a — comment placement.

**Shape of the fix.** Move the five lines to sit immediately above `let
ctx_addr = cfg.ctx as usize;`. Zero risk. This is the cheapest genuinely
valuable change in the report.

---

#### M3 — The three callback adapters are the same eight-line shape three times

**Category:** duplicated code
**Severity:** Medium
**Files:** `src/ffi.rs:369-377` (read), `378-388` (write), `389-399` (flush)

Each wraps a raw `extern "C"` function pointer in a boxed Rust closure with
the identical body: rematerialise `ctx` from `ctx_addr`, invoke, and map the
return code:

```rust
if rc == 0 { Ok(()) } else { Err(cb_io_err(rc, "<name>")) }
```

Three instances of the rc-mapping, differing only in the string literal. These
four lines are also the crate's deepest indentation — all four production
lines at 20+ columns anywhere in `src/` are here (`src/ffi.rs:383,385,394,396`),
which is what the previous review's L2 was pointing at before the file grew.

`fs_core_device_from_callbacks` is at 69 lines the crate's longest function,
and this trio is most of its bulk.

**Test coverage: good.** `src/ffi.rs:594-668` exercises read, write and flush
through the real trampolines including the read-only-when-`write`-is-NULL
path; `tests/callback_device.rs` covers error propagation for read and write
callbacks.

**Shape of the fix.** A single `fn map_rc(rc: c_int, op: &'static str) ->
std::io::Result<()>` next to `cb_io_err`, called by all three closures. Cuts
the function to ~50 lines and removes the deepest nesting in the crate.

---

#### M4 — `FS_CORE_BAD_STRING` is a published error code that can never be returned

**Category:** speculative code for a scenario that can't happen
**Severity:** Medium
**Files:** `src/ffi.rs:64-65`, `include/fs_core.h:36`

```rust
/// Path string was not valid UTF-8 (or NUL-terminated).
BadString = 8,
```

```c
FS_CORE_BAD_STRING    = 8,
```

Verified: `FsCoreErrorCode::BadString` is never constructed anywhere in the
crate. The single situation it describes — a non-UTF-8 path — is handled at
`src/ffi.rs:273-277`, which cannot return it, because
`fs_core_file_open` returns `*mut FsCoreDevice`, not a code. It sets the
thread-local message and returns NULL instead.

So the C header publishes, as part of a block explicitly marked "Stable: do
not renumber," an outcome no call can produce. A consumer writing an
exhaustive `switch` over `FsCoreErrorCode` writes a `FS_CORE_BAD_STRING` arm
that is dead in every build.

`FS_CORE_PANIC` and `FS_CORE_NULL_ARG` are both genuinely reachable
(`src/ffi.rs:133` and `218/236/248`), so this is specific to `BadString`.

**Test coverage:** none — there is nothing to cover.

**Shape of the fix.** Either document it as reserved-and-currently-unreachable
in both the Rust doc comment and the header, or (better) give it a use: a
future `fs_core_file_open_ex` returning a code rather than a pointer would
want it. Do not renumber or remove it — the header's stability promise is
worth more than the tidiness.

---

#### M5 — `stats()` returns a bare `(u64, u64)`, discarding names the struct already has

**Category:** misleading/opaque names
**Severity:** Medium
**Files:** `src/caching_device.rs:39-42`

```rust
pub fn stats(&self) -> (u64, u64) {
    let s = self.state.lock().unwrap();
    (s.hits, s.misses)
}
```

`CacheState` names these fields properly at `src/caching_device.rs:21-22`. The
names are then thrown away at the one place they matter — the public API — so
every caller has to either open this file or guess. Guessing wrong is silent:
both are `u64`, both are plausibly first, and a hit/miss ratio computed
backwards looks like a plausible number rather than an error.

`tests/composition_stacks.rs:43` (`re_streaming_through_cache_records_hits`)
and several tests in `tests/caching_lru.rs` destructure this tuple positionally
and are correct — but only because their authors checked.

**Test coverage:** the values are covered thoroughly (`tests/caching_lru.rs`,
`tests/cache.rs:106-122`); the ordering is only ever asserted positionally.

**Shape of the fix.** `pub struct CacheStats { pub hits: u64, pub misses: u64 }`,
or two methods. **This is a public API change** and therefore semver-visible —
listed as report-only for that reason. The skill's own rule is not to change
exported signatures without explicit agreement.

---

#### M6 — `invalidate_range` takes four parameters to work around a borrow that is not a conflict

**Category:** too many parameters; dense expression
**Severity:** Medium
**Files:** `src/caching_device.rs:49-54`, call site `95-99`

```rust
{
    let mut s = self.state.lock().unwrap();
    let bs = self.block_size;
    Self::invalidate_range(&mut s, offset, end, bs);
}
```

`invalidate_range` is an associated function rather than a method, taking the
locked state *and* `block_size` explicitly, so the call site needs the `let bs
=` temporary. But `self.block_size` is a `u64` (`Copy`) living in a different
field from `self.state`, so reading it while `state` is locked was never a
borrow problem. The temporary and the associated-fn form are both working
around a constraint that does not exist.

A reader hits `let bs = self.block_size;` and reasonably assumes there is a
borrow-checker reason for it, then spends time looking for one.

**Test coverage: excellent.** `tests/caching_cross_block.rs` has five tests
covering single-block, two-block, three-block, boundary-aligned, and
end-byte-only invalidation.

**Shape of the fix.** `fn invalidate_range(&self, state: &mut CacheState,
start: u64, end: u64)` reading `self.block_size` directly. Private, no API
change, and the well-covered behaviour makes it low-risk.

---

#### M7 — `BlockRead for &T` exists; `BlockDevice for &T` does not, and the impls are interleaved so the gap reads as an oversight

**Category:** misleading abstraction
**Severity:** Medium
**Files:** `src/block.rs:49-101`

The forwarding impls appear in this order:

| line | impl |
|---|---|
| 52 | `BlockRead for Arc<T>` |
| 61 | `BlockDevice for Arc<T>` |
| 73 | `BlockRead for Box<T>` |
| 82 | `BlockRead for &T` |
| 91 | `BlockDevice for Box<T>` |

`Arc` gets both halves. `Box` gets both halves, split apart by an unrelated
impl. `&T` gets only the read half.

Two separate readability costs. The interleaving means the `Box` pair reads as
incomplete until you scroll past `&T`. And the genuinely-absent `BlockDevice
for &T` has no stated rationale — it may be deliberate (a shared reference
should not carry write authority) or it may simply never have been needed. The
section comment at `src/block.rs:49-50` says "so `Arc<T>` and `Box<T>` work
transparently" and does not mention `&T` at all, so the one impl that *is*
there is undocumented too.

A driver author who writes `&dev` where a `BlockDevice` is expected gets a
trait-bound error with no hint whether to add the impl upstream or restructure
their code.

**Test coverage:** `tests/block_forwarding.rs` covers all five existing impls
(`ref_blockread_forwards` at line 111 covers the `&T` read half). Nothing
covers or documents the absence — correctly, since there is nothing to test.

**Shape of the fix.** Reorder to keep each pair adjacent, and add one line to
the section comment stating why `&T` is read-only. If the omission turns out
to be accidental, adding `impl<T: BlockDevice + ?Sized> BlockDevice for &T` is
additive and non-breaking.

---

#### M8 — Eleven hand-rolled in-memory device fakes, each subtly different

**Category:** duplicated code
**Severity:** Medium
**Files:**

| file:line | fake |
|---|---|
| `src/slice.rs:215` | `Bytes(Mutex<Vec<u8>>)` |
| `src/slice.rs:294` | `RwBytes(Mutex<Vec<u8>>)` |
| `src/stream.rs:142` | `Bytes(Mutex<Vec<u8>>)` |
| `src/stream.rs:165` | `AlwaysFails` |
| `src/readonly.rs:60` | `WritableBytes(Mutex<Vec<u8>>)` |
| `tests/block_forwarding.rs:6` | `Tracker` |
| `tests/cache.rs:65` | `CountingDev` |
| `tests/caching_lru.rs:6` | `CountingDev` |
| `tests/caching_cross_block.rs:11` | `Mem` |
| `tests/slice_rw.rs:7` | `Bytes` |
| `tests/readonly_wrapper.rs:7` | `WritableBytes` |

Every one is a `Vec<u8>` behind a `Mutex` implementing `BlockRead` (+
sometimes `BlockDevice`). They differ in ways that are easy to miss and that
matter: some return `ShortRead` on over-read (`src/slice.rs:221`), some
`copy_from_slice` and panic instead (`src/readonly.rs:65`), some count
operations, some do not.

The risk is not the line count — it is that a test can pass because *its*
fake happens to be lenient where a real device is not. A driver author reading
`tests/` to learn the expected device contract gets eleven partially
contradictory answers.

The leverage argument is what pushes this above cosmetic: the seven consumer
crates each need the same fixture and almost certainly each carry their own
copy. One shared `MemoryDevice` behind `#[cfg(feature = "testing")]`, exported
from this crate, would serve all of them and would make the device contract
executable rather than folkloric.

**Test coverage:** n/a — this *is* the test code.

**Shape of the fix.** Add `pub mod testing` gated behind an off-by-default
`testing` feature, containing one `MemoryDevice` with explicit read/write
bounds semantics matching `FileDevice`, plus `AlwaysFails`. Migrate this
crate's own fakes to it. Additive; nothing existing breaks. Note this is a new
public surface on a crate whose main risk (per the previous review) is
accretion — but a test double for the crate's own trait is squarely within
"block-device abstraction," not a general utility.

---

#### M9 — `CachingDevice::new` decides the caller's ownership; every other constructor does not

**Category:** misleading abstraction
**Severity:** Medium
**Files:** `src/caching_device.rs:26`

```rust
pub fn new(inner: Arc<dyn BlockDevice>, block_size: u64, capacity: usize) -> Arc<Self>
```

Compare the rest of the crate:

| constructor | returns |
|---|---|
| `FileDevice::open` (`src/file_device.rs:19`) | `Result<Self>` |
| `ReadOnlyDevice::new` (`src/readonly.rs:23`) | `Self` |
| `OwnedSlice::new` (`src/slice.rs:89`) | `Self` |
| `OwnedRwSlice::new` (`src/slice.rs:142`) | `Self` |
| `BlockReadStreamer::new` (`src/stream.rs:42`) | `Self` |
| `CachingDevice::new` | **`Arc<Self>`** |

One of six pre-allocates for the caller. A caller who wants a `CachingDevice`
by value, or inside their own `Arc<dyn BlockDevice>` allocation, cannot have
one without an extra indirection. More to the point for readability: a reader
who has learned the crate's constructor convention from five types gets it
wrong on the sixth, and the error message (`expected CachingDevice, found
Arc<CachingDevice>`) does not explain that this one is special.

**Test coverage:** every caching test constructs it this way and would need
updating — `tests/cache.rs`, `tests/caching_lru.rs`,
`tests/caching_cross_block.rs`, `tests/composition_stacks.rs`.

**Shape of the fix.** Return `Self` and let callers wrap, or keep `new() ->
Self` and add `new_arc() -> Arc<Self>` for the common case. **Public API
change** — report-only for the same reason as M5.

---

### Low

---

#### L1 — `open_best_effort` discards why the read-write open failed

**Category:** speculative/defensive code
**Severity:** Low
**Files:** `src/file_device.rs:41-47`

```rust
match Self::open_rw(p) {
    Ok(d) => Ok(d),
    Err(_) => Self::open(p),
}
```

`Err(_)` treats every failure identically: a permissions denial (the intended
case), a missing file, a busy device, a bad path. For the intended case the
fallback is right. For the others it converts one clear error into a second,
less relevant one — a missing file reports the read-only open's `ENOENT`,
which is fine, but a device-busy failure silently becomes a read-only mount.

The doc comment ("Open read-write if possible, fall back to read-only
otherwise") describes the behaviour accurately; what is missing is the note
that "otherwise" is deliberately unconditional.

**Test coverage:** `src/file_device.rs:146-177` covers both branches;
`tests/file_device_edge_cases.rs:147` covers the fallback. The
non-permission failure modes are not covered.

**Shape of the fix.** One comment line stating the fallback is intentionally
unconditional. Changing the behaviour is not recommended — the current shape
is probably right for the mount path it serves.

---

#### L2 — `capacity: 0` yields a one-entry cache rather than no cache

**Category:** dense logic / undocumented edge
**Severity:** Low
**Files:** `src/caching_device.rs:80-83`

```rust
if s.entries.len() >= s.capacity {
    s.entries.pop_back();
}
s.entries.push_front((offset, data));
```

With `capacity == 0`: `len()` is `0`, `0 >= 0` holds, `pop_back()` on an empty
deque is a no-op, and `push_front` runs anyway. The cache settles at exactly
one entry forever. A caller passing `0` to mean "disable caching" gets a
one-block cache instead — and if the intent was to bypass the cache, they now
have stale-read exposure they thought they had opted out of.

`capacity` is neither validated nor documented (`src/caching_device.rs:26`,
no doc comment). The type is `usize`, so `0` is reachable and looks reasonable.

**Test coverage:** `tests/caching_lru.rs:49` covers `capacity_one_evicts_on_every_new_block`.
Nothing covers `capacity == 0`.

**Shape of the fix.** Document what `0` does, add a test pinning it, or
`debug_assert!(capacity > 0)`. Whichever — currently the behaviour is
accidental rather than chosen.

---

#### L3 — A test import kept alive by a statement whose only purpose is to use that import

**Category:** speculative code for a scenario that can't happen
**Severity:** Low
**Files:** `src/ffi.rs:553`, `src/ffi.rs:666-667`

```rust
use std::sync::{Arc as StdArc, Mutex as StdMutex};   // line 553
...
    // suppress unused warning
    let _ = StdArc::new(StdMutex::new(0u8));         // lines 666-667
```

Circular: the import is unused, so a statement was added to use it, so the
import is no longer unused. Neither line does anything. Deleting both is
behaviour-neutral and removes a small puzzle from the middle of an otherwise
clear test.

**Test coverage:** n/a — dead code inside a test.

**Shape of the fix.** Delete both. (Per the collision-guard rule, this deletes
two lines inside a test, not a test — `callback_device_readonly_when_write_null`
keeps all its assertions.)

---

#### L4 — A magic `5` in the test trampolines, commented as an error it does not name

**Category:** magic numbers
**Severity:** Low
**Files:** `src/ffi.rs:565`, `src/ffi.rs:580`

```rust
if off + len > st.data.len() {
    return 5; // out of bounds
}
```

The callback contract is "0 on success, non-zero (errno-like) on failure"
(`src/ffi.rs:308-309`), so `5` is legal — it is an errno-space value, not an
`FsCoreErrorCode`. But it sits fifteen lines from an enum where
`FS_CORE_OUT_OF_BOUNDS = 4` and `FS_CORE_CUSTOM = 5`, and it is commented
"out of bounds." A reader who connects the literal to the nearby enum reads
the comment as wrong, then has to work out that the two numbering schemes are
unrelated.

**Test coverage:** the trampolines are exercised by
`src/ffi.rs:594-640`; this specific branch (over-range callback read) is not
hit by any test.

**Shape of the fix.** `const CB_ERR_RANGE: c_int = 5;` with a one-line note
that callback codes are errno-space, not `FsCoreErrorCode`.

---

#### L5 — `panic_message` borrows a `Box` where a trait reference would do

**Category:** dense/awkward signature
**Severity:** Low
**Files:** `src/ffi.rs:138`

```rust
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
```

`&Box<dyn Trait>` is a double indirection; `&(dyn Any + Send)` conveys the
same thing with one. This is the shape clippy's `borrowed_box` normally
flags — CI runs `-D warnings` and passes, so the `dyn` form is evidently
exempt, but the readability point stands independently of the lint.

All five call sites (`src/ffi.rs:132, 295, 412, 449, 477`) pass `&panic` where
`panic` is already a `Box`, so the change is `&panic` → `panic.as_ref()` at
five sites. Private function; no API impact.

**Test coverage:** none — every call site is on a panic path, and no test in
the crate triggers a panic across the FFI boundary. (See M1: the panic arms
are collectively the largest untested region in `ffi.rs`.)

---

#### L6 — The crate's only doc example is ```` ```ignore ````, so it is never compiled

**Category:** comments that lie
**Severity:** Low
**Files:** `src/stream.rs:12-20`

```rust
//! ```ignore
//! use fs_core::{BlockReadStreamer, FileDevice};
//! use std::io::Read;
//!
//! let dev = FileDevice::open("disk.img")?;
//! let mut stream = BlockReadStreamer::new(dev);
//! let mut hasher = sha2::Sha256::new();
//! std::io::copy(&mut stream, &mut hasher)?;
//! ```
```

Confirmed by the test run: `Doc-tests fs_core … 1 ignored`. This is the only
doc example in the crate, and `ignore` means rustdoc never compiles it — so it
cannot fail, and it will not notice if the API it demonstrates changes
underneath it.

The reason for `ignore` is legitimate: the example uses `sha2`, which is not a
dependency (the crate has none, deliberately), and `?` outside a function body
would not compile either. But the effect is an example that is documentation
in appearance and unverified prose in fact — on the one type whose whole
purpose is to be composed with third-party `Read` consumers, where a wrong
example is most likely to be copied verbatim.

**Test coverage:** none, by construction — that is the finding.

**Shape of the fix.** Rewrite as a compiling `no_run` example using only std
(`std::io::copy` into a `Vec<u8>`, or `read_to_end`), wrapped in
`fn main() -> std::io::Result<()>`. That gets it compiled by `cargo test` on
every CI run at the cost of dropping the (illustrative but non-essential)
hashing flavour. Keep the `sha2` variant as plain non-code prose beneath it if
the flavour is worth keeping.

---

## What I would fix first

The order below is deliberately not the severity order. It front-loads changes
that are cheap, un-reversible-risk-free, and reduce the cost of the expensive
decisions that follow.

**1. H3 — fix the README.** Ten minutes, no code touched, and it is the
finding with the widest audience: every consumer author reads this file, and
right now it hides the entire FFI surface and denies the existence of two
shipped types. Nothing else in this report is read by as many people.

**2. M2 — move the orphaned comment.** Two minutes. It relocates an
explanation from a function that needs none onto the four lines in the crate
most likely to be "cleaned up" by someone who does not know why they are
there. Same category of value as the previous review's L3, which was also its
top recommendation and remains unaddressed — the `catch_unwind` at
`src/ffi.rs:355` still has no stated rationale, and adding those two sentences
belongs in the same edit.

**3. H2 — collapse the three slice types onto shared geometry.** This is the
best-covered code in the crate (30-plus tests across four files), so the
refactor is guarded from the first line. Do it before H1, not after: it
collapses H1's four divergent sites into one, which turns H1 from "change four
things consistently and hope" into a one-line decision.

**4. H1 — then decide the error vocabulary.** With H2 done this is a single
edit, and the decision is yours to make rather than mine to guess: option 1
(document the split) is free and non-breaking; option 3 (unify on
`OutOfBounds`) is correct and breaks a published ABI. My recommendation is
option 1 now and option 3 at the next major version, because the crate's value
is its stability and the current behaviour, while inconsistent, is at least
consistently inconsistent and locked by tests.

**5. H4 — write down the `is_writable` rule.** One doc-comment paragraph on
`BlockDevice`. This is the finding whose cost is paid entirely by other crates,
and the cheapest option (documentation) captures the invariant at the exact
point a driver author will read it.

**Then, if there is appetite:** L6 (make the one doc example compile — it is
the only executable documentation the crate has, and right now it is not
executed), M1 and M3 (both internal to `ffi.rs`, both
pure deduplication, both zero-ABI-impact), M6 (well covered, small), M4 and
M7 (documentation), L3 and L4 (trivial).

**Not recommended without a separate conversation:** M5 and M9 change public
signatures on a published crate with seven consumers; M8 adds public surface to
a crate whose stated main risk is accretion. All three are real, none is
urgent, and each is a semver decision rather than a readability fix.

---

## Deliberately not flagged

Recording these so a future pass does not re-litigate them:

- **`ffi.rs` is 677 lines, half the crate.** The previous review's M1 called
  this a ratio to watch rather than fix, and that judgement still holds. It is
  a flat list of ABI entry points; splitting it would add navigation cost
  without reducing what a reader holds in their head. M1/M3 above shrink it by
  deduplication instead, which is the right lever.
- **`CachingDevice`'s O(capacity) linear LRU scan** (`src/caching_device.rs:67`).
  Correct and readable for the small capacities the module doc describes; a
  `HashMap` + intrusive list would be faster and harder to read. Not a
  readability finding.
- **`set_last_error`'s `.expect("contains no NUL after replace")`**
  (`src/ffi.rs:92`). A panic in FFI-adjacent code, but unreachable by
  construction — the `replace('\0', "?")` on the previous expression
  guarantees it — and the expect message says exactly that. Idiomatic.
- **`impl BlockDevice for OwnedSlice {}` / `SliceReader`** as a *local*
  pattern. Documented at `src/slice.rs:74-76` and `128`. The pattern itself is
  fine here; H4 is about what it does in consumer crates, not this one.
- **Zero `#[allow(...)]` outside `src/ffi.rs:25`** (`missing_safety_doc`, which
  is defensible for a module whose header carries the safety contract). Worth
  protecting.

---

## Test results

Nothing was changed, so before and after are the same run.

| | before | after |
|---|---|---|
| tests passing | 129 | 129 (unchanged — no code modified) |
| tests failing | 0 | 0 |
| tests ignored | 1 (the `stream.rs` doc example — see L6) | 1 |
| test count | 41 unit + 88 integration | same |
| clippy (`--locked --all-targets -- -D warnings`) | clean, exit 0 (verified this pass) | unchanged |
| rustfmt | clean (CI-enforced) | unchanged |
| line coverage | CI gate `--fail-under-lines 90` | unchanged |

Both `cargo test` and the full CI clippy gate were run against the unmodified
tree during this pass; neither was inferred from CI history.

Coverage gaps noted incidentally during triage, all in error/panic paths:

- every `Err(panic)` arm in `src/ffi.rs` (5 sites: `ffi_guard` at 131, plus
  270-298, 355-415, 438-452, 469-480) — no test panics across the FFI boundary
- `FsCoreErrorCode::BadString` — unreachable, so untestable (M4)
- `CachingDevice` with `capacity == 0` (L2)
- `FileDevice::open_best_effort` fallback on non-permission errors (L1)
- the over-range branch of the FFI test trampolines (`src/ffi.rs:565,580`)

These are observations, not requests. The 90% line gate is met.

---

## Provenance

Analysis performed against the working tree at commit `bd20ed6`, clean, on
branch `main`. No files other than this report were created or modified. No
branches, commits, or pushes were made.
