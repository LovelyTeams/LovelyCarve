# Code quality review — 2026-08-25

**Scope:** `src/`, 1,334 production lines across 10 files (test modules excluded from
every count below).
**Findings:** 0 high, 1 medium, 2 low. No fixes applied — this is a read of the code
as it stands.

This is the cleanest crate in the family, and by some distance. It has no duplication,
no unnamed offsets, no `#[allow]` suppressions, no function over 70 lines, and no
function taking more than four parameters. Three lines in the whole crate are indented
past 24 columns.

That is not luck. It is a small crate with one job — the `BlockRead` / `BlockDevice`
trait surface and a few implementations of it — and it has stayed that size while
every consumer of it has grown. Whatever review this crate gets should mostly be
concerned with keeping it that way.

---

## M1 — `ffi.rs` is 487 lines, 36% of the crate

**`src/ffi.rs`**

The C ABI is more than a third of a crate whose actual subject is a trait and four
implementations of it. That ratio is worth watching rather than fixing: it is what
happens when a small, stable core acquires a foreign-function surface, and the surface
does not shrink just because the core is small.

Nothing in the file is badly written. `fs_core_device_from_callbacks` (69 lines) is the
crate's longest function and reads cleanly — a null check, a `catch_unwind` guard, then
callback wrapping — with each failure setting a thread-local error before returning
null, consistently.

**Recommendation:** leave it. Splitting a flat list of ABI entry points adds navigation
cost without reducing what a reader has to hold in their head. Noted so that the ratio
is a deliberate state rather than an unnoticed one.

---

## L2 — Three lines indented 24 columns or deeper

**`src/ffi.rs`**

All three are in `fs_core_device_from_callbacks`, inside the `catch_unwind` closure
where a `match` on an optional callback nests one level further than the rest of the
crate ever does.

This is the least significant finding in the whole review. It is recorded only because
counting it and finding three is itself the useful result.

---

## L3 — `catch_unwind` is used, but the reason is not stated

**`src/ffi.rs:355`**

```rust
let res = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
```

Unwinding across an FFI boundary is undefined behaviour, which is why the guard is
here — but the code does not say so. A reader who does not already know that rule sees
a `catch_unwind` that appears to be defensive programming and may conclude it is
unnecessary.

`AssertUnwindSafe` compounds it: the assertion is that the captured state is safe to
observe after a panic, and that judgement is currently unwritten.

**Shape of the fix.** Two sentences above the call. This is the kind of comment that
stops a future reader removing something load-bearing.

---

## What is good, and is worth protecting

- **No duplication.** Zero repeated eight-line blocks.
- **No `#[allow(...)]` anywhere.** The only crate in the family that needs none.
- **No function over 70 lines**, and only one over 60.
- **No function with five or more parameters.**
- **One file per concept**, and the file names say what they hold:
  `block.rs`, `caching_device.rs`, `callback_device.rs`, `file_device.rs`,
  `readonly.rs`, `slice.rs`, `stream.rs`. A reader looking for the caching wrapper
  does not have to guess.
- **The composition model is genuinely simple.** `CachingDevice` holds an
  `Arc<dyn BlockDevice>` and is itself a `BlockDevice`, so layers stack without any
  layer knowing about the others. That property is why several sibling crates can wrap
  disk images and partitions without this crate changing.
- **`clippy -D warnings` and `rustfmt` are clean**, and CI enforces both.

## The finding that matters most is not in the list

This crate is a dependency of every filesystem driver in the family. Its API surface is
small, and its stability is the reason the drivers can be developed independently.

The main risk to it is not any of the above — it is accretion: a helper that "everyone
needs" landing here because there is nowhere else obvious, until `fs-core` becomes a
utility crate. The trait surface is the crate's whole value, and it is worth being
deliberately unwelcoming to anything that is not a block-device abstraction.

## Suggested order

L3, which takes two minutes and prevents a real mistake. Nothing else needs doing.
