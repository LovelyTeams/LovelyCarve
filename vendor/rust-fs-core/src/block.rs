//! Block-device traits.
//!
//! Two layers because not every consumer needs writes:
//!
//! - [`BlockRead`] is the minimum: positioned reads + size. Disk-image
//!   readers (qcow2 reading) and probes (partition table walk) only need
//!   this much.
//! - [`BlockDevice`] extends `BlockRead` with optional `write_at` / `flush`
//!   / `is_writable` / `set_len` / `can_grow`. Read-only devices that opt
//!   into the larger trait inherit the default `Err(ReadOnly)` write path
//!   automatically, and every device that cannot change its own length
//!   inherits the same refusal for `set_len`.
//!
//! Both traits are `Send + Sync` so callers can hold them behind `Arc<dyn _>`
//! across thread boundaries.

use crate::error::{Error, Result};

/// Read-only random-access block device.
pub trait BlockRead: Send + Sync {
    /// Read exactly `buf.len()` bytes starting at `offset` (bytes from the
    /// start of the device).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Total device size in bytes. Used for bounds checks.
    ///
    /// # It must not change for the life of the device — except through
    /// [`BlockDevice::set_len`]
    ///
    /// Callers are entitled to read it once, act on it, and read it again
    /// later expecting the same answer — [`crate::CachingDevice`] does
    /// exactly that, bounding a read against it and then clamping each
    /// block it fetches against it. A device whose size moves between the
    /// two makes the cache hold a block shorter than the read it was
    /// fetched for.
    ///
    /// An implementation whose length changes UNDERNEATH IT — a second
    /// handle appending to the same file, a volume grown by something
    /// else — must still not follow it. Reopen instead. That is
    /// rust-fs-core#70, and `tests/size_stability_contract.rs` measures
    /// it: a `FileDevice` whose backing file grows through another handle
    /// goes on reporting the length it was opened with.
    ///
    /// # THE ONE DOOR, AND WHY IT IS NOT A HOLE IN THE CONTRACT
    ///
    /// [`BlockDevice::set_len`] moves this number, and it is the only
    /// thing that may. The distinction is not "the size is stable except
    /// when it is not": it is that a caller ASKED, so the moment the
    /// number moves is a moment the caller chose, on a call it made, and
    /// every wrapper on the path is given the chance to keep its own view
    /// consistent as it goes past. `size_bytes` never surprises a reader;
    /// `set_len` is the reader.
    ///
    /// That is exactly what a device following its backing file could not
    /// offer. Nobody is told, nothing is invalidated, and the two halves
    /// of one device end up disagreeing about where it ends — which is
    /// the defect, not the movement.
    ///
    /// A device that answers `false` to [`BlockDevice::can_grow`] cannot
    /// be moved at all, and the stronger, older reading of this contract
    /// holds for it unchanged. Most devices in this crate are in that
    /// class.
    ///
    /// # What the implementations here actually do
    ///
    /// This used to say "every implementation in this crate takes its size
    /// once, at construction". That was not true of any of the three
    /// shapes below, and it is the sentence an implementer in a sibling
    /// crate reads before deciding their own device may report a live
    /// length. [`crate::caching_device`]'s resizing path and
    /// `tests/caching_size_change.rs` both used to describe this
    /// differently again, so the crate said three incompatible things
    /// about one contract.
    ///
    /// - [`crate::FileDevice`] and the slice devices in
    ///   [`crate::slice`] do take their size once, at construction. The
    ///   slices keep it forever; a `FileDevice` opened read-write on a
    ///   regular file also answers [`BlockDevice::can_grow`] with `true`,
    ///   so its number moves when — and only when — somebody calls
    ///   [`BlockDevice::set_len`] on it.
    /// - [`crate::ReadOnlyDevice`], [`crate::CountingDevice`] and
    ///   [`crate::CachingDevice`] FORWARD the question to the device they
    ///   wrap, on every call, and so are exactly as stable as it is. They
    ///   cannot be more: a wrapper has no way to hold a moving device
    ///   still. The `Arc<T>`, `Box<T>` and `&T` blanket impls below
    ///   forward the same way.
    /// - [`crate::CallbackDevice`] keeps its size in a PUBLIC field, so
    ///   nothing stops a caller moving it after construction. Stability
    ///   there is the caller's to keep, not the type's to enforce.
    ///
    /// So the contract binds the implementer; it is not something this
    /// crate's types guarantee on their behalf. `tests/
    /// size_stability_contract.rs` tests each of these behaviours rather
    /// than leaving this paragraph to be believed.
    fn size_bytes(&self) -> u64;
}

/// Read-write random-access block device. Implementors that genuinely
/// support writes override `write_at` / `flush` / `is_writable`. The
/// defaults model a strict read-only device.
pub trait BlockDevice: BlockRead {
    /// Write exactly `buf.len()` bytes at `offset`. Default: returns
    /// [`Error::ReadOnly`].
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> Result<()> {
        Err(Error::ReadOnly)
    }

    /// Flush pending writes to stable storage. No-op by default.
    fn flush(&self) -> Result<()> {
        Ok(())
    }

    /// Whether `write_at` is likely to succeed. Mount paths use this to
    /// decide whether to attempt journal replay or stay strict-read-only.
    fn is_writable(&self) -> bool {
        false
    }

    /// Set the device's length to `new_len`. Default: returns
    /// [`Error::ReadOnly`].
    ///
    /// # WHY A DEVICE NEEDS THIS AT ALL
    ///
    /// Every sparse disk-image format in this family allocates the same
    /// way: append a block, cluster or grain to the end of the file, then
    /// record where it went. The only tool any of them had for the first
    /// half was a write past the end, and #75 refused that — correctly.
    /// `write_all` at a seeked offset EXTENDS a file, so the backing store
    /// grew while `size_bytes` went on reporting its construction-time
    /// length, and a caller bounding its reads by `size_bytes` (which is
    /// what [`crate::CachingDevice`] does, clamping every block it
    /// fetches) could never reach the bytes it had just written. The two
    /// halves of one device disagreed about where it ended. That is
    /// rust-fs-core#70.
    ///
    /// The bound is right and it stays. What was missing is the other
    /// half: a way to say *make the device longer*, out loud, so the
    /// number it reports and everything caching it move at the same
    /// moment. See rust-fs-core#147 and #129 for the four crates this
    /// blocked, and the measurements that named them.
    ///
    /// # THE CONTRACT IS THE ATOMICITY, NOT THE SIGNATURE
    ///
    /// An implementation must leave the backing store, the number
    /// [`BlockRead::size_bytes`] reports, and any cached view of the
    /// device agreeing with each other when this returns. An
    /// implementation that extends the store and leaves `size_bytes`
    /// stale, or leaves a cache holding a block clamped to the old
    /// length, has re-created #70 — and it will pass a naive test,
    /// because the write it enables succeeds and only a LATER cached read
    /// finds the hole.
    ///
    /// Where the two steps cannot be made one instruction, publish the
    /// NARROWER of the two lengths first: the declared size may lag
    /// behind a store that has already grown, but it must never exceed
    /// one that has already shrunk. A read inside a device that is
    /// shorter than it claims is a read off the end of the store.
    ///
    /// # IT SETS, IT DOES NOT ONLY GROW
    ///
    /// `new_len` below the current length TRUNCATES, discarding the bytes
    /// past it, exactly as [`std::fs::File::set_len`] does — the name is
    /// that method's and it means the same thing. A caller that only ever
    /// appends should compute `new_len` from `size_bytes()` and never
    /// hand this a smaller number; hiding the shrink behind a refusal
    /// would make a method named `set_len` do something else.
    ///
    /// # WHY THIS IS ON `BlockDevice` AND NOT A `GrowableDevice` TRAIT
    ///
    /// Because of what the consumers hold. All four image crates carry
    /// `Arc<dyn BlockDevice>` — `rust-img-vhdx` at 15 sites,
    /// `rust-img-qcow2` at 7, `rust-img-vhd` at 6, `rust-img-vmdk` at 5 —
    /// and a `dyn` type cannot be bounded onto a second trait without
    /// downcasting through [`std::any::Any`]. That turns "this device
    /// cannot grow" from a value into a failed downcast: a runtime error
    /// with a worse message than the one the default below gives, at
    /// every one of those 33 sites.
    ///
    /// It is also the idiom this trait already uses. `write_at` defaults
    /// to `Err(Error::ReadOnly)` and `is_writable` defaults to `false`
    /// for exactly the same reason — an optional capability, answered by
    /// the device rather than by the type system. [`can_grow`] is
    /// `is_writable` for length, and it is asked the same way.
    ///
    /// [`can_grow`]: BlockDevice::can_grow
    ///
    /// # THE DEFAULT REFUSES, AND IT IS `ReadOnly` DELIBERATELY
    ///
    /// Same variant as `write_at`'s default, because it is the same
    /// statement one field over: this device will not accept a mutation
    /// of this kind. [`crate::stream`] maps `ReadOnly` to
    /// `PermissionDenied` and `Custom` to an uncategorised
    /// `io::Error::other`, so a caller branching on the former to say
    /// "this cannot be modified" keeps working over a refused grow.
    ///
    /// A device that is writable but fixed-length — a block device node,
    /// a slice — is the one place that reads oddly, and it is why
    /// [`can_grow`] exists: branch on the capability, not on the error.
    fn set_len(&self, _new_len: u64) -> Result<()> {
        Err(Error::ReadOnly)
    }

    /// Whether [`set_len`] is likely to succeed. Default: `false`.
    ///
    /// The question an image writer asks before it plans an allocation,
    /// and the reason the default above can be a refusal rather than a
    /// panic or a downcast. `is_writable` for length.
    ///
    /// `false` here is a promise that `set_len` will refuse, not a hint.
    /// A caller is entitled to read it once at mount time and build a
    /// writer around the answer.
    ///
    /// [`set_len`]: BlockDevice::set_len
    fn can_grow(&self) -> bool {
        false
    }
}

// Forwarding impls so `Arc<T>` and `Box<T>` work transparently as
// `&dyn BlockRead` / `&dyn BlockDevice`.
//
// EVERY DEFAULTED METHOD ON `BlockDevice` HAS TO BE FORWARDED HERE, AND
// A MISSING ONE IS SILENT.
//
// Method resolution on an `Arc<dyn BlockDevice>` finds the impl below
// BEFORE it derefs to the device inside, so a method these impls do not
// override is answered by the TRAIT DEFAULT over a device that would
// have answered differently. Nothing warns: the code compiles, the type
// is right, and the wrapper quietly says `false` / `Err(ReadOnly)` for a
// device that can do the thing.
//
// That is not hypothetical for `set_len`. The four image crates hold
// `Arc<dyn BlockDevice>` at 33 sites between them, so a `set_len` added
// to the trait and not added here would have shipped an API that changed
// nothing for any of them. `tests/device_growth.rs` pins both directions
// through both wrappers — forwarding the capability AND forwarding a
// refusal, since an impl that answered `true` unconditionally would
// satisfy the first alone.

impl<T: BlockRead + ?Sized> BlockRead for std::sync::Arc<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for std::sync::Arc<T> {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        (**self).write_at(offset, buf)
    }
    fn flush(&self) -> Result<()> {
        (**self).flush()
    }
    fn is_writable(&self) -> bool {
        (**self).is_writable()
    }
    fn set_len(&self, new_len: u64) -> Result<()> {
        (**self).set_len(new_len)
    }
    fn can_grow(&self) -> bool {
        (**self).can_grow()
    }
}

impl<T: BlockRead + ?Sized> BlockRead for Box<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }
}

impl<T: BlockRead + ?Sized> BlockRead for &T {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for Box<T> {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        (**self).write_at(offset, buf)
    }
    fn flush(&self) -> Result<()> {
        (**self).flush()
    }
    fn is_writable(&self) -> bool {
        (**self).is_writable()
    }
    fn set_len(&self, new_len: u64) -> Result<()> {
        (**self).set_len(new_len)
    }
    fn can_grow(&self) -> bool {
        (**self).can_grow()
    }
}
