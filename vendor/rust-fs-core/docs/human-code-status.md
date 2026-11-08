# Human-code findings — status

Tracks every **High** and **Medium** finding from
[`human-code-report-2026-08-28.md`](human-code-report-2026-08-28.md) and what
was done about it.

The report records everything as open because it was written before any of it
was acted on. This file is the current position. Updated 2026-08-29.

**19 findings** — 4 High, 9 Medium, 6 Low. This covers the 13 High and Medium.

| | High | Medium |
|---|---|---|
| Fixed | 3 | 3 |
| Left for a human decision | 1 | 4 |
| Fixable, not yet done | 0 | 2 |

---

## High

### H1 — one bounds condition, two error variants, `OutOfBounds` unreachable — **fixed earlier**

Fixed by [#11](https://github.com/antimatter-studios/rust-fs-core/pull/11), and
the outcome is worth recording because **the finding's premise was wrong**.

`OutOfBounds` is not unreachable. It is constructed on read paths by four
sibling crates — `am-img-qcow2`, `vhd`, `vhdx`, `vmdk` — and surfaces straight
through `BlockRead::read_at`. It is dead only when the device came from this
crate. So the fix was to correct the **documentation**, not the behaviour, and
the slice deduplication (H2) was done instead.

Unifying on one variant would have broken substitutability: `FileDevice`
answers a read at EOF with exactly `ShortRead { got: 0 }` and slices match.

### H2 — three slice types are one type three times — **fixed earlier**

`SliceGeometry` in `src/slice.rs` holds the one range check the three types
share. Fixed by [#11](https://github.com/antimatter-studios/rust-fs-core/pull/11).

### H3 — the README omitted four modules and listed two shipped types as unimplemented — **fixed**

`slice.rs`, `readonly.rs`, `stream.rs` and `ffi.rs` are now in the layout tree.

The worse half was the Roadmap, which listed `SliceReader` and
`ReadOnlyDevice<T>` under **"Planned additions (not yet implemented)"** while
both ship and are re-exported from `lib.rs`. A reader taking the README at its
word would go looking for them in `am-partitions`, or write their own. Both
entries are gone.

### H4 — an empty `impl BlockDevice for X {}` is load-bearing — **needs your decision**

The trait supplies defaults for all three methods, so an empty impl means
"strictly read-only" — deliberate at the three sites in this crate, and
`readonly.rs` documents the intent well.

The report's real concern is what the same design does in a *consumer*: a
driver author who forgets `write_at` gets a device that silently refuses writes
rather than a compile error. That is a genuine trap.

**Not fixed, because every available fix is a breaking change with a real
trade-off.** Removing the defaults makes every read-only implementor write
three stub methods; splitting the trait changes the public shape every consumer
already implements. Which cost is acceptable is a design call about the crate's
public surface, not a defect to correct.

---

## Medium

### M1 — the C ABI hand-rolls `ffi_guard` — **fixed, and it was worse than four times**

`ffi_guard` returns an `FsCoreErrorCode` and takes a body returning
`Result<(), Error>`, so it fits an entry point whose whole answer is a status
code and fits nothing else. The report counted the four pointer-returning
functions that work around it. **There are seven**: the other three return a
size, a flag, or nothing.

And those three are not merely duplicated — **they swallow the panic**:

```rust
std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*handle).inner.size_bytes() }))
    .unwrap_or(0)
```

Every fallback here is also a legitimate answer. Zero is what an empty device
reports for its size, `false` is what a read-only device reports for
writability, a null pointer is what a failed open returns. A caller seeing only
the fallback cannot tell an ordinary answer from a driver that exploded
computing it — and `fs_core_last_error_message`, the one thing that separates
them, was left empty.

`ffi_guard_or(fail, body)` takes any return type, records the panic's own
message, and clears the slot on entry so a successful call does not leave the
previous one's message to be misattributed. All seven sites use it; the four
that already recorded the message lose a seven-line epilogue each.

Three tests, red before the change. Mutation-checked: dropping the
`set_last_error` fails 4, dropping the `clear_last_error` fails 2.

**Worth reading beyond the fix.** The abstraction existed, was too narrow to
reuse, and the crate that owns it worked around it in its own file. Eleven
sister crates re-roll one of two shapes rather than share either — `ext4`,
`erofs` and `squashfs` each carry a private `ffi_guard(fail, body)` close to
what has just been added here. That is the first hard evidence for the open
question of whether this crate genericises enough, and it is left as evidence:
adopting this downstream is eleven repositories' worth of change and wants
deciding as one.

### M2 — a five-line explanation sat above the wrong function — **fixed**

The comment explaining why `ctx` is round-tripped through `usize` was above
`cb_io_err`, which does not do that. It now sits above the `let ctx_addr =
cfg.ctx as usize;` it explains, and `cb_io_err` has a one-line doc of its own.

### M3 — the three callback adapters are the same eight-line shape — **fixed**

Read, write and flush each wrapped a host callback in the same four lines:
invoke, compare against zero, `Ok(())` or `cb_io_err`. Three copies of one
convention is three chances to write `rc != 0` where the others write `rc == 0`
— and a caller would then see reads succeed while writes reported failure on the
very same device.

`cb_result(rc, op)` states it once: **zero is success**. `op` names the operation
in the error, which is the only thing the three genuinely differ in.

Mutation-checked: inverting the comparison fails 2 tests.

### M4 — `FS_CORE_BAD_STRING` is published and can never be returned — **fixed**

Verified: the only path-taking entry point, `fs_core_open_file`, returns a
**pointer**, so it reports a bad path as NULL plus a message and cannot return
a code at all. No other entry point takes a path.

**Documented rather than removed.** The numbering is published in
`include/fs_core.h`; a consumer may already switch on `8`, and renumbering the
codes after it would be an ABI break for a tidiness gain. Both the Rust enum
and the C header now say it is reserved and why, so the next reader does not
have to rediscover it.

### M5 — `stats()` returns a bare `(u64, u64)` — **needs your decision**

A named struct would be better to read. It is also a public API change to a
shipped crate, and the crate is already published at `0.2.3` with consumers.
Whether that churn is worth it is yours.

### M6 — `invalidate_range` takes four parameters to work around a borrow — **needs your decision**

Same category: the fix is a signature change on a public method.

### M7 — `BlockRead for &T` exists, `BlockDevice for &T` does not — **needs your decision**

Adding the impl is additive and small. But the gap may be deliberate — a
`&T` that can be written through has different aliasing implications from one
that can only be read — and the report does not establish which it is. Adding a
public impl on a guess is the wrong direction to be wrong in.

### M8 — eleven hand-rolled in-memory device fakes — **fixed, four of them**

Checking that no test depended on a difference was the right caution, and it
found one. `stream::Bytes`, `slice::Bytes` and `slice::RwBytes` were identical;
**`readonly::WritableBytes` read without a bounds check**, so a past-end read
panicked where the other three returned `ShortRead`.

`ShortRead` is the right answer for all four. A device that panics on a past-end
read turns a caller's arithmetic bug into a crash in the harness rather than an
error the caller can be asserted against. `src/test_device.rs` holds `Bytes` and
`RwBytes`, and 101 lines of duplication are gone.

**Three doubles deliberately stay.** `stream::AlwaysFails`, `ffi::Panicking` and
`tests/cache.rs::CountingDev` each exist to misbehave in one specific way, which
is the opposite of what a shared device is for — they are not duplicates, they
are the point of their tests. `tests/` keeps its own for the compilation-boundary
reason: an integration test cannot see a `#[cfg(test)]` item.

Mutation-checked: making the shared device zero-fill a short read instead of
failing breaks 3 tests.

### M9 — `CachingDevice::new` decides the caller's ownership — **needs your decision**

Public API shape, same category as M5 and M6.

---

## Verification

`cargo test` — 42 unit, 7 doc and 6 integration tests pass, unchanged in number:
nothing here changes behaviour. `chore lint` clean.
