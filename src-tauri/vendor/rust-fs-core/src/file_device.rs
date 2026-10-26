//! File-backed `BlockDevice`. Used for disk images, raw `/dev/diskN` reads,
//! anything that std::fs::File can address.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// A file opened as a block device.
///
/// # Readers share the lock; writers take it alone
///
/// A read was once `seek` then `read` under a plain mutex, which made
/// the file's cursor shared state: two threads reading different offsets
/// had to take turns, not because the device could not serve them at
/// once but because one would have moved the other's cursor.
///
/// On Unix the cursor is not involved at all — `pread` takes the offset
/// as an argument — so readers hold the lock *shared* and genuinely
/// overlap. On Windows the equivalent (`seek_read`) *does* move the file
/// pointer, so readers there take it exclusively and only that platform
/// pays for the cursor.
///
/// Writers take it exclusively on both, because `write_at` is `seek`
/// plus `write_all` and neither another writer's seek nor a reader may
/// land in the middle of it.
///
/// # THE LOCK IS NOT AN OPTIMISATION, IT IS THE READ/WRITE CONTRACT
///
/// Reads briefly took no lock at all, which read as a natural
/// consequence of positioned reads needing no cursor. It was not: it
/// silently dropped the exclusion between readers and writers that the
/// single mutex had provided, so a read overlapping a `write_at` could
/// observe part of it. `write_all` is permitted to become several
/// `write` calls, and a read is a loop of positioned reads — either
/// split is a window, and the second one does not need the first.
///
/// So a reader holds the lock for the whole of [`FileDevice::read_at`],
/// not for each positioned read inside it. Per-read guards would leave
/// exactly the same hole one level down, and a rarer tear is worse than
/// a common one because nobody can reproduce it.
///
/// # A known limit, stated rather than fixed
///
/// `std::sync::RwLock` does not promise writer preference on every
/// platform, and the read path here is the hot one. A device under
/// sustained parallel reads can therefore make a writer wait longer than
/// a fair queue would. That is a throughput property, not a correctness
/// one, and it is left alone rather than solved with a hand-rolled queue
/// nobody would be able to audit.
pub struct FileDevice {
    file: File,
    /// Shared by readers, exclusive to writers — see the type's own
    /// note. `()` rather than the file, because it orders access rather
    /// than owning the handle: positioned reads need no cursor, so
    /// putting the `File` in here would reintroduce the serialisation
    /// the shared guard exists to avoid.
    io_lock: RwLock<()>,
    /// ATOMIC BECAUSE [`BlockDevice::set_len`] MOVES IT, AND
    /// `size_bytes` TAKES NO LOCK.
    ///
    /// It was a plain `u64`, which was right while nothing could change
    /// it. `set_len` takes `&self` -- every method on these traits does,
    /// because the devices are held behind `Arc<dyn _>` -- so the number
    /// needs interior mutability, and the cheapest correct one is an
    /// atomic rather than putting it under `io_lock`.
    ///
    /// Not under `io_lock` DELIBERATELY: `size_bytes` is called on the
    /// hot path of every wrapper in this crate ([`crate::CachingDevice`]
    /// asks it twice per read), and routing it through a lock a writer
    /// holds exclusively would make an ordinary read contend with an
    /// ordinary write to learn a number that fits in a register.
    ///
    /// `set_len` still takes `io_lock` exclusively -- it has real I/O to
    /// exclude -- and publishes this afterwards. See its own note for
    /// which of the two it moves first.
    size: AtomicU64,
    writable: bool,
    /// Whether `set_len` can do anything: a writable handle on a REGULAR
    /// FILE.
    ///
    /// Both halves are needed and neither implies the other. A read-only
    /// handle obviously cannot truncate. A BLOCK DEVICE NODE is the case
    /// that is easy to miss: `/dev/sdX` opened read-write is writable,
    /// `write_at` works on it, and its length is the kernel's rather than
    /// ours -- `ftruncate` on it is not a resize, and answering `true`
    /// here would promise an image writer room it can never get.
    ///
    /// Decided once, at open, from the same `metadata` call that
    /// `measure_size` is about to make: asking on every `can_grow` would
    /// turn a capability question into a syscall, and the file type of an
    /// already-open descriptor does not change.
    growable: bool,
    /// Test-only witness that a thread reached `io_lock` — see
    /// [`LockArrivals`]. Absent from every non-test build, and the
    /// `arriving` it feeds compiles to nothing there.
    #[cfg(test)]
    arrivals: LockArrivals,
}

/// How many threads are inside an `io_lock` acquisition and have not
/// yet been granted the guard.
///
/// # WHY A TEST NEEDS THIS AND CANNOT DO WITHOUT IT
///
/// The exclusion this type promises can only be asserted negatively —
/// an operation that must *not* proceed — and "did not finish within
/// 300ms" is not that assertion. It is satisfied just as well by a
/// worker the OS has not scheduled, so on a loaded runner a build with
/// no lock at all passes. That is rust-fs-core#104, and it applied to
/// all four of the tests that are the only evidence #77's fix works.
///
/// Signalling readiness from the top of the worker closure does not
/// fix it: the gap between the signal and the call under test has no
/// synchronisation in it, so the signal proves the thread ran once,
/// not that it reached the lock.
///
/// A counter incremented immediately before the acquisition and
/// decremented immediately after it is granted turns that into a
/// positive observation the test can wait for. Seeing it non-zero
/// while the test itself holds the lock means: this thread is in the
/// acquisition, and it cannot leave until we let go. Remove the lock
/// from the operation and the counter is never touched, so the wait
/// times out and names what failed rather than passing.
///
/// Measured on `a_read_cannot_proceed_while_a_write_holds_the_lock`
/// with `read_at`'s guard deleted, one commit, this machine:
///
/// | test shape                                | result   | time  |
/// |-------------------------------------------|----------|-------|
/// | ready signal, 400ms deschedule in the gap | **ok**   | 0.41s |
/// | ready signal, no deschedule               | FAILED   | 0.33s |
/// | this counter                              | FAILED   | 10.0s |
///
/// The first row is the defect: a build with no lock at all, passing.
/// The gap has no upper bound on it, so 400ms is an illustration of a
/// class rather than a threshold.
///
/// THE DECREMENT IS WHAT KEEPS THIS HONEST. If it went missing the
/// counter would stick above zero, every wait would return
/// immediately, and all four exclusion tests would pass without a
/// worker ever reaching the lock — the same unwitnessed shape one
/// level up. See
/// `an_uncontended_operation_leaves_no_thread_waiting_at_the_lock`.
#[cfg(test)]
#[derive(Default)]
struct LockArrivals {
    waiting: std::sync::atomic::AtomicUsize,
}

/// Decrements the moment the guard is granted, whatever happens.
#[cfg(test)]
struct Arrival<'a>(&'a LockArrivals);

#[cfg(test)]
impl Drop for Arrival<'_> {
    fn drop(&mut self) {
        self.0
            .waiting
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl FileDevice {
    /// Open read-only.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let size = measure_size(&file)?;
        Ok(Self {
            file,
            io_lock: RwLock::new(()),
            size: AtomicU64::new(size),
            writable: false,
            // A read-only handle cannot change any length, whatever it
            // is open on.
            growable: false,
            #[cfg(test)]
            arrivals: LockArrivals::default(),
        })
    }

    /// Open read-write. Errors if the path is not writable.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let size = measure_size(&file)?;
        let growable = is_regular_file(&file);
        Ok(Self {
            file,
            io_lock: RwLock::new(()),
            size: AtomicU64::new(size),
            writable: true,
            growable,
            #[cfg(test)]
            arrivals: LockArrivals::default(),
        })
    }

    /// Open read-write if possible, fall back to read-only otherwise.
    pub fn open_best_effort<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        match Self::open_rw(p) {
            Ok(d) => Ok(d),
            Err(_) => Self::open(p),
        }
    }
}

/// How many bytes the opened handle addresses.
///
/// For a **regular file** this is the metadata length, and 0 is a
/// legitimate answer: an empty file is empty.
///
/// For a **device node** it is not an answer at all. `st_size` is 0 for
/// every block and character device on the platforms this crate ships
/// to, so the size has to be asked for directly. Measured against an
/// 8 MiB backing store:
///
/// | source                    | `metadata().len()` | `lseek(SEEK_END)` | ioctl     |
/// |---------------------------|--------------------|-------------------|-----------|
/// | macOS `/dev/disk9` (blk)  | 0                  | 0                 | 8388608   |
/// | macOS `/dev/rdisk9` (chr) | 0                  | 0                 | 8388608   |
/// | Linux `/dev/loop0` (blk)  | 0                  | 8388608           | 8388608   |
///
/// **`lseek(SEEK_END)` is not the portable fallback it looks like.** It
/// answers 0 on macOS for both node types, so a device opened there
/// would still report itself empty. The ioctl is the only mechanism that
/// answered on both platforms, and it happens to avoid the objection to
/// the seek as well: it does not move the file cursor. That matters
/// here, because `read_once` uses `pread` specifically so reads need no
/// lock -- see this type's own note.
///
/// When the size cannot be measured this returns an error rather than 0,
/// because **a device reporting 0 is not inert, it is invisible.**
/// `read_at` keeps serving real bytes, while
/// [`BlockReadStreamer::read`] returns `Ok(0)` on its first call,
/// [`CachingDevice`] treats every read as past-the-end and caches
/// nothing, and every slice cut from it inherits a parent claiming to be
/// empty. Each of those four failures is silent, which is the one
/// outcome worth refusing outright.
///
/// [`BlockReadStreamer::read`]: crate::BlockReadStreamer
/// [`CachingDevice`]: crate::CachingDevice
#[cfg(unix)]
fn measure_size(file: &File) -> Result<u64> {
    use std::os::unix::fs::FileTypeExt;
    let meta = file.metadata()?;
    let ft = meta.file_type();
    if !ft.is_block_device() && !ft.is_char_device() {
        return Ok(meta.len());
    }
    device_size_bytes(file)
}

/// Windows keeps the metadata length, because no equivalent measurement
/// has been made there. `\\.\PhysicalDriveN` is therefore still subject
/// to the defect this function exists to fix; saying so is better than
/// shipping an untested `DeviceIoControl` and implying otherwise.
#[cfg(not(unix))]
fn measure_size(file: &File) -> Result<u64> {
    Ok(file.metadata()?.len())
}

// The crate has no dependencies and this is not worth acquiring one for:
// `ioctl` is in libc, which is already linked into every std target.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux"))]
unsafe extern "C" {
    fn ioctl(fd: std::os::raw::c_int, request: std::os::raw::c_ulong, ...) -> std::os::raw::c_int;
}

/// macOS: `<sys/disk.h>` gives the block size and the block count
/// separately, and neither alone is the answer.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn device_size_bytes(file: &File) -> Result<u64> {
    use std::io;
    use std::os::fd::AsRawFd;
    // _IOR('d', 24, u32) and _IOR('d', 25, u64).
    const DKIOCGETBLOCKSIZE: std::os::raw::c_ulong = 0x4004_6418;
    const DKIOCGETBLOCKCOUNT: std::os::raw::c_ulong = 0x4008_6419;

    let fd = file.as_raw_fd();
    let mut block_size: u32 = 0;
    let mut block_count: u64 = 0;
    // SAFETY: `fd` is open for as long as `file` is borrowed, and each
    // request writes exactly the width its own encoding names into a
    // local of precisely that type.
    unsafe {
        if ioctl(fd, DKIOCGETBLOCKSIZE, &raw mut block_size) < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if ioctl(fd, DKIOCGETBLOCKCOUNT, &raw mut block_count) < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    block_count
        .checked_mul(u64::from(block_size))
        .ok_or_else(|| {
            Error::Io(io::Error::other(format!(
                "device reports {block_count} blocks of {block_size} bytes, \
                 whose product does not fit in u64"
            )))
        })
}

/// `BLKGETSIZE64` as `_IOR(0x12, 114, size_t)`, for a given pointer
/// width.
///
/// THE SIZE FIELD OF AN IOCTL REQUEST IS `sizeof(size_t)` -- the
/// USERSPACE POINTER WIDTH, not the width of the value the kernel
/// writes back. The payload is a `u64` on both, but the request NUMBER
/// differs: `0x8008_1272` where a pointer is 8 bytes, `0x8004_1272`
/// where it is 4. A literal for one width is rejected by the kernel on
/// the other.
///
/// That is a regression this change would have INTRODUCED rather than
/// inherited. Before it, an unmeasurable device node gave `Ok` with a
/// size of 0; now it is an error, so a 64-bit-only constant would make
/// [`FileDevice::open`] fail for EVERY block device on a 32-bit target.
///
/// Taking the width as a parameter is what makes it testable: nothing
/// available here runs 32-bit, and `cargo check --target` compiles a
/// wrong literal perfectly happily, so the only witness possible is to
/// compute both encodings and compare them with the two numbers the
/// kernel headers actually define.
///
/// Available in every TEST build rather than on Linux alone, because a
/// test that cannot run is not a witness: gated to Linux it would be
/// compiled and never executed on the machine the work is done on, and
/// the arithmetic is the same arithmetic everywhere.
#[cfg(any(target_os = "linux", test))]
const fn blkgetsize64_for(pointer_width: usize) -> std::os::raw::c_ulong {
    /// `_IOC_READ << _IOC_DIRSHIFT`.
    const READ: std::os::raw::c_ulong = 0x8000_0000;
    /// `_IOC_TYPESHIFT` is 8, `_IOC_SIZESHIFT` is 16.
    const TYPE: std::os::raw::c_ulong = 0x12;
    const NR: std::os::raw::c_ulong = 114;
    READ | ((pointer_width as std::os::raw::c_ulong) << 16) | (TYPE << 8) | NR
}

/// Linux: `<linux/fs.h>` answers in bytes in one call.
#[cfg(target_os = "linux")]
fn device_size_bytes(file: &File) -> Result<u64> {
    use std::io;
    use std::os::fd::AsRawFd;
    // _IOR(0x12, 114, size_t), encoded for THIS target -- see
    // `blkgetsize64_for`. Identical to the familiar 0x8008_1272 on a
    // 64-bit target and correct on a 32-bit one.
    const BLKGETSIZE64: std::os::raw::c_ulong = blkgetsize64_for(std::mem::size_of::<usize>());

    let mut size: u64 = 0;
    // SAFETY: as above -- one call, writing one u64 into a u64.
    let rc = unsafe { ioctl(file.as_raw_fd(), BLKGETSIZE64, &raw mut size) };
    if rc < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(size)
}

/// Every other Unix: refuse rather than guess. `lseek` is measured wrong
/// on one of the two platforms tested, so extending it here on the
/// strength of that would be picking the silent failure.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "linux"))
))]
fn device_size_bytes(_file: &File) -> Result<u64> {
    use std::io;
    Err(Error::Io(io::Error::other(
        "no measured way to read a device node's size on this platform; \
         open the backing image file rather than the device node",
    )))
}

impl FileDevice {
    /// Marks this thread as having reached `io_lock` and not yet been
    /// granted it. The returned value decrements on drop — which, at
    /// the two call sites below, is after the acquisition returns.
    ///
    /// Compiles to nothing outside a test build: the field it counts
    /// does not exist there. See [`LockArrivals`] for why a test cannot
    /// establish the same thing from outside.
    #[cfg(test)]
    fn arriving(&self) -> Arrival<'_> {
        self.arrivals
            .waiting
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Arrival(&self.arrivals)
    }

    /// THE ONLY PLACES `io_lock` IS ACQUIRED — two on Unix, one on
    /// Windows.
    ///
    /// Not a wrapper for its own sake: the arrival counter has to sit
    /// immediately before the acquisition to mean anything, and one
    /// pair of methods is what stops a third call site being added
    /// without it. `_arrival` outlives the tail expression and is
    /// dropped once the guard has been granted — which is what makes
    /// a non-zero count mean "waiting" rather than "has waited".
    ///
    /// The `cfg` is on the statement rather than on two bodies of
    /// `arriving`, so a non-test build contains the acquisition and
    /// nothing else.
    ///
    /// # `shared_guard` IS UNIX-ONLY, AND THAT IS THE DESIGN RATHER
    /// THAN TIDINESS
    ///
    /// Nothing on Windows takes `io_lock` shared. `read_guard` there is
    /// `exclusive_guard`, deliberately, because `seek_read` moves the
    /// file pointer and a reader must exclude other readers as well as
    /// writers — see `read_guard`. So on Windows this method is not
    /// merely unused, it MUST NOT BE CALLED: a future caller reaching
    /// for the cheaper guard would reintroduce the cursor race that the
    /// exclusive read guard exists to prevent.
    ///
    /// `cargo clippy --target x86_64-pc-windows-msvc --all-targets
    /// -- -D warnings` reported it as `method shared_guard is never
    /// used`, and `#[allow(dead_code)]` would have been the wrong
    /// answer: it silences the compiler on a platform where the right
    /// statement is that the method does not exist. Compiling it out
    /// makes a call site that should not exist fail to build.
    #[cfg(unix)]
    fn shared_guard(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        #[cfg(test)]
        let _arrival = self.arriving();
        self.io_lock.read().unwrap()
    }

    fn exclusive_guard(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        #[cfg(test)]
        let _arrival = self.arriving();
        self.io_lock.write().unwrap()
    }

    /// The guard a read holds for the whole of `read_at`.
    ///
    /// Unix takes it SHARED: `pread` carries its own offset, so readers
    /// do not disturb each other and only need to be kept apart from
    /// writers.
    ///
    /// Windows takes it EXCLUSIVE, because `seek_read` moves the file
    /// pointer — there, one reader really can spoil another's offset, so
    /// readers must exclude readers as well as writers. Same lock, same
    /// call site, and only that platform pays for the cursor.
    #[cfg(unix)]
    fn read_guard(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.shared_guard()
    }

    #[cfg(windows)]
    fn read_guard(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.exclusive_guard()
    }

    /// One positioned read, returning what it got.
    ///
    /// TAKES NO LOCK ON EITHER PLATFORM. The caller holds `read_guard`
    /// for the whole read; acquiring anything here would be a second,
    /// non-reentrant acquisition of the same lock — on Windows, where
    /// that guard is exclusive, an immediate self-deadlock.
    #[cfg(unix)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::unix::fs::FileExt;
        Ok(self.file.read_at(buf, offset)?)
    }

    /// Windows: `seek_read` DOES move the file pointer. The exclusion
    /// that needs is held by the caller's guard, not taken here — see
    /// `read_guard`.
    #[cfg(windows)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::windows::fs::FileExt;
        Ok(self.file.seek_read(buf, offset)?)
    }
}

impl BlockRead for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        // HELD ACROSS THE WHOLE LOOP, NOT AROUND EACH POSITIONED READ.
        //
        // The loop below can issue several reads for one call, and a
        // guard taken inside it would let a `write_at` land between two
        // of them: the caller would get some bytes from before the write
        // and some from after, which is the tear this exists to prevent
        // moved one level down and made rarer. Rarer is worse — nobody
        // can reproduce it.
        //
        // NO TEST PINS THIS PLACEMENT, and the reason is worth writing
        // down because the obvious one is wrong. A regular file does
        // return short reads — at EOF — and this loop retries rather
        // than failing on one, so a read spanning EOF really does run
        // twice and the guard really would be released in between. That
        // discriminator was built: a 4096-byte file, a reader asking
        // 4090..4102, a writer parked on the lock growing the file at
        // 4090, where interleaved old-and-new bytes are reachable no
        // other way. 200 trials with the guard moved inside the loop
        // produced 0 tears. The window between releasing at the end of
        // one iteration and retaking at the start of the next is a few
        // instructions, and a writer already waiting never won it.
        //
        // So this is unwitnessed, not inert: the placement is correct
        // and the race it prevents is simply too narrow to enter on
        // demand. Measured, not argued.
        let _guard = self.read_guard();

        // A SHORT READ IS AN ERROR NAMING WHAT WAS ASKED FOR AND WHAT
        // ARRIVED, not a smaller answer: a caller that asked for a block
        // and got half of one cannot tell the difference from bytes.
        let mut total = 0usize;
        while total < buf.len() {
            let n = self.read_once(offset + total as u64, &mut buf[total..])?;
            if n == 0 {
                return Err(Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: total,
                });
            }
            total += n;
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        // `Acquire` pairs with the `Release` store in `set_len`: a
        // thread that observes a grown size must also observe the
        // `ftruncate` that produced it.
        self.size.load(Ordering::Acquire)
    }
}

impl BlockDevice for FileDevice {
    /// A write past the end is refused, not an extension.
    ///
    /// `write_all` at a seeked offset EXTENDS a file, and this method
    /// had no bound of its own, so a write straddling the end grew the
    /// backing store while `size_bytes` went on reporting the length
    /// taken at construction -- measured on a 4096-byte file:
    /// `write_at(4094, 8 bytes)` returned `Ok`, the file became 4102
    /// bytes, `size_bytes()` stayed 4096, and `read_at(4096, 6)` then
    /// handed those bytes back. The two halves of one device disagreed
    /// about where it ended, and a caller bounding its reads by
    /// `size_bytes` -- which is what [`crate::CachingDevice`] does,
    /// clamping every block it fetches -- could never reach them.
    ///
    /// `RwBytes` in this crate's own test devices already refuses the
    /// same operation, commenting "a device is not a `Vec`", and the
    /// slice adapters in [`crate::slice`] clamp their window
    /// specifically because this method did not:
    /// `slice_rw_length_is_clamped_and_a_write_past_it_does_not_grow_the_image`
    /// names the file's length on disk as its oracle. This is the same
    /// rule one layer down, where it was missing.
    ///
    /// The alternative -- letting the size move and reopening -- is
    /// what [`BlockRead::size_bytes`]'s contract forbids. See
    /// rust-fs-core#70.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        // READ ONCE, COMPARED AND REPORTED FROM THE SAME VALUE. `size`
        // is atomic now that `set_len` moves it, and loading it twice
        // could bound the write against one length and name another in
        // the error -- an `OutOfBounds` whose `size` field does not
        // explain its own refusal.
        let size = self.size_bytes();
        // `checked_add` because a caller-supplied offset near `u64::MAX`
        // would otherwise wrap and land back inside the device.
        let end = offset.checked_add(buf.len() as u64);
        if end.is_none_or(|end| end > size) {
            return Err(Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size,
            });
        }
        // EXCLUSIVE: excludes other writers' seeks and every reader.
        let _guard = self.exclusive_guard();
        let mut f = &self.file;
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        // EXCLUSIVE for the same reason as `write_at`: this pushes
        // buffered bytes at the file and must not interleave with a
        // write or a read.
        let _guard = self.exclusive_guard();
        let mut f = &self.file;
        f.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    fn is_writable(&self) -> bool {
        self.writable
    }

    /// Set the file's length, and the length this device reports, as one
    /// operation.
    ///
    /// # THE POINT IS THAT THE TWO MOVE TOGETHER
    ///
    /// #75 refused a write past the end because `write_all` at a seeked
    /// offset grew the FILE while `size_bytes` went on reporting the
    /// length taken at construction, so the two halves of one device
    /// disagreed about where it ended and a caller bounding its reads by
    /// `size_bytes` could never reach what it had written
    /// (rust-fs-core#70). That bound stays. This method is the other
    /// half: growth that says so, and moves the number with it.
    ///
    /// An implementation that called `File::set_len` and left `self.size`
    /// alone would be #70 with a nicer name on it.
    ///
    /// # THE NARROWER LENGTH IS PUBLISHED FIRST, IN BOTH DIRECTIONS
    ///
    /// `ftruncate` and the store are two steps, and one of the two
    /// orderings has a window in it. Take a shrink done store-then-
    /// publish: between the truncate and the store, this device declares
    /// 8192 bytes over a 4096-byte file, so a concurrent read inside the
    /// declared device falls off the end of the real one and comes back
    /// `ShortRead`. The other direction is harmless — a device that
    /// briefly declares 4096 bytes over an 8192-byte file is only
    /// under-reporting, which is the state every `FileDevice` is in
    /// whenever something else appends to its file.
    ///
    /// So: a GROW truncates and then stores, and a SHRINK stores and then
    /// truncates. The invariant is one sentence — THE DECLARED SIZE NEVER
    /// EXCEEDS THE FILE'S REAL LENGTH — and it holds at every instant
    /// rather than only at the ends.
    ///
    /// # `io_lock` EXCLUSIVELY, LIKE A WRITE
    ///
    /// For the same reason `write_at` and `flush` take it: this changes
    /// the file underneath every reader, and a read must not observe
    /// half of it. `size_bytes` deliberately does NOT take the lock —
    /// see the field — so the ordering above is what keeps a reader that
    /// asked the size mid-call from being misled, not the lock.
    ///
    /// # WHAT IT REFUSES
    ///
    /// [`Error::ReadOnly`] on a handle opened with [`FileDevice::open`],
    /// before touching the file. [`Error::Custom`] on a handle that is
    /// writable but not a regular file — a block device node, whose
    /// length belongs to the kernel — naming that reason rather than
    /// letting `ftruncate`'s `EINVAL` stand in for it. See
    /// [`FileDevice::can_grow`], which is the question to ask instead of
    /// discovering either of these.
    fn set_len(&self, new_len: u64) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        if !self.growable {
            return Err(Error::Custom(
                "this FileDevice is open on something that is not a regular file \
                 -- a device node's length is the kernel's, not ours, and cannot \
                 be set through this handle"
                    .to_string(),
            ));
        }

        // EXCLUSIVE: excludes every reader and every other writer, the
        // same as `write_at`.
        let _guard = self.exclusive_guard();

        let old = self.size.load(Ordering::Acquire);
        if new_len < old {
            // Narrow the declared device BEFORE the bytes go. See above.
            self.size.store(new_len, Ordering::Release);
        }
        if let Err(e) = self.file.set_len(new_len) {
            // A FAILED SHRINK HAS ALREADY NARROWED THE DECLARATION, and
            // leaving it there would make the device deny bytes it still
            // holds -- #70's defect, reached by the error path. Re-measure
            // rather than restoring `old`: whether `ftruncate` did nothing
            // or something is not knowable from its error, and the file
            // itself is the only honest answer.
            if let Ok(actual) = measure_size(&self.file) {
                self.size.store(actual, Ordering::Release);
            }
            return Err(e.into());
        }
        self.size.store(new_len, Ordering::Release);
        Ok(())
    }

    fn can_grow(&self) -> bool {
        self.growable
    }
}

/// Is this handle open on a regular file?
///
/// The other half of `can_grow`, and the half a reader is most likely to
/// assume. A `FileDevice` is opened just as often on `/dev/sdX` as on an
/// image: `measure_size` has a whole `ioctl` path for exactly that case.
/// Such a handle is writable, `write_at` works on it, and its length is
/// fixed by the kernel -- `ftruncate` on a block device is not a resize.
///
/// A failed `metadata` call answers `false`. The question is "may this
/// device promise it can grow", and a promise nobody could verify is not
/// one to make.
fn is_regular_file(file: &File) -> bool {
    file.metadata().is_ok_and(|m| m.file_type().is_file())
}

#[cfg(test)]
mod tests {
    /// THE TWO NUMBERS THE KERNEL HEADERS DEFINE, and the pointer
    /// widths they belong to. External knowledge, not a restatement of
    /// the formula -- a test that recomputed the encoding on both sides
    /// would agree with itself whatever the encoding was.
    ///
    /// This cannot prove the ioctl works on 32-bit; nothing available
    /// here runs 32-bit, and `cargo check --target` compiles a wrong
    /// literal happily. What it does is stop the encoding quietly
    /// reverting to a single hardcoded number, which is the way this
    /// defect arrived.
    #[test]
    fn blkgetsize64_is_encoded_for_the_pointer_width() {
        const KNOWN: &[(usize, u64)] = &[(4, 0x8004_1272), (8, 0x8008_1272)];
        for (width, want) in KNOWN {
            assert_eq!(
                super::blkgetsize64_for(*width) as u64,
                *want,
                "_IOR(0x12, 114, size_t) with a {width}-byte size_t is {want:#010x}"
            );
        }
    }

    /// And the one this build will actually issue is the one for THIS
    /// target, rather than whichever happened to be written down.
    #[test]
    fn this_target_issues_its_own_encoding() {
        let width = std::mem::size_of::<usize>();
        let expected = if width == 8 {
            0x8008_1272u64
        } else {
            0x8004_1272u64
        };
        assert_eq!(super::blkgetsize64_for(width) as u64, expected);
    }

    use super::*;

    /// Concurrent readers do not serialise, and none of them sees
    /// another's offset.
    ///
    /// THE BUG THIS REPLACES: reads were `seek` then `read` under one
    /// mutex, so the file cursor was shared state. Two threads reading
    /// different parts of the same image took turns for no reason the
    /// device imposed. Worse, the shape was one edit away from being
    /// wrong rather than merely slow -- drop the lock without moving to
    /// positioned reads and every reader corrupts every other reader's
    /// offset.
    ///
    /// The assertion is on the BYTES rather than on timing: a test that
    /// measured overlap would be a flake on a loaded machine, while a
    /// reader that got another's offset returns the wrong bytes every
    /// time.
    #[test]
    fn many_threads_reading_different_offsets_each_get_their_own_bytes() {
        let path = temp_path("parallel_reads");
        let _c = Cleanup(path.clone());
        // Each 256-byte page filled with its own page number, so a read
        // that landed at the wrong offset is obvious from one byte.
        let mut bytes = Vec::with_capacity(64 * 256);
        for page in 0..64u8 {
            bytes.extend(std::iter::repeat_n(page, 256));
        }
        std::fs::write(&path, &bytes).expect("write the image");

        let dev = std::sync::Arc::new(FileDevice::open(&path).expect("open"));
        let mut handles = Vec::new();
        for page in 0..64u8 {
            let dev = dev.clone();
            handles.push(std::thread::spawn(move || {
                // Several times each, so a thread that raced would have
                // many chances to read somebody else's page.
                for _ in 0..50 {
                    let mut buf = [0u8; 256];
                    dev.read_at(u64::from(page) * 256, &mut buf).expect("read");
                    assert!(
                        buf.iter().all(|b| *b == page),
                        "page {page} came back holding another page's bytes"
                    );
                }
            }));
        }
        for h in handles {
            h.join().expect("a reader panicked");
        }
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique temp path under the system temp dir (no extra dev-deps).
    fn temp_path(tag: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("fs_core_{tag}_{pid}_{n}.bin"))
    }

    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn open_rw_round_trips_write_then_read() {
        let path = temp_path("rw");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0u8; 32]).unwrap();

        let dev = FileDevice::open_rw(&path).unwrap();
        assert!(dev.is_writable());
        assert_eq!(dev.size_bytes(), 32);

        dev.write_at(8, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
        dev.flush().unwrap();

        let mut buf = [0u8; 4];
        dev.read_at(8, &mut buf).unwrap();
        assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn open_rw_errors_on_missing_path() {
        let path = temp_path("missing");
        assert!(FileDevice::open_rw(&path).is_err());
    }

    #[test]
    fn open_best_effort_uses_rw_when_writable() {
        let path = temp_path("best_rw");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0u8; 16]).unwrap();

        let dev = FileDevice::open_best_effort(&path).unwrap();
        assert!(dev.is_writable());
        dev.write_at(0, &[0x11; 4]).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn open_best_effort_falls_back_to_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("best_ro");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0xEFu8; 16]).unwrap();
        // Read-only permissions force `open_rw` to fail; fall back to `open`.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let dev = FileDevice::open_best_effort(&path).unwrap();
        assert!(!dev.is_writable());
        // Writes are rejected at the read-only layer.
        assert!(matches!(dev.write_at(0, &[0u8; 4]), Err(Error::ReadOnly)));
        // Read still works.
        let mut buf = [0u8; 4];
        dev.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xEF; 4]);
        // Flush on a read-only device is a no-op success.
        dev.flush().unwrap();
    }

    use std::sync::mpsc;
    use std::time::Duration;

    /// How long an operation that must finish is given. Generous on
    /// purpose: a loaded machine makes it slower, not flakier.
    ///
    /// EVERY DEADLINE IN THE EXCLUSION TESTS IS THIS ONE. There used to
    /// be a second, short one — 300ms, after which a worker that had
    /// not finished was taken to have been blocked. That is the defect
    /// rust-fs-core#104 describes: an unscheduled worker is
    /// indistinguishable from a blocked one, so the assertion passed
    /// for a reason unrelated to the lock and would have kept passing
    /// with the lock removed. Blocking is now established by
    /// [`await_arrival`] instead, which fails when nothing arrives
    /// rather than passing when nothing happens.
    const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

    /// A 4 KiB image of one repeated byte, opened read-write.
    fn rw_image(tag: &str, fill: u8) -> (std::sync::Arc<FileDevice>, Cleanup) {
        let path = temp_path(tag);
        let cleanup = Cleanup(path.clone());
        std::fs::write(&path, vec![fill; 4096]).expect("write the image");
        let dev = std::sync::Arc::new(FileDevice::open_rw(&path).expect("open rw"));
        (dev, cleanup)
    }

    fn waiting_at_the_lock(dev: &FileDevice) -> usize {
        dev.arrivals.waiting.load(Ordering::SeqCst)
    }

    /// Blocks until a thread is parked inside an `io_lock` acquisition,
    /// and PANICS IF NONE EVER IS.
    ///
    /// This is the half that carries the evidence. Once it returns, the
    /// worker is between the counter's increment and the guard being
    /// granted — and since the caller holds that guard and has not let
    /// go, the worker cannot leave. "The operation is blocked on the
    /// lock" is then a fact about the program's state rather than an
    /// inference from a stopwatch.
    ///
    /// An operation that stopped taking the lock never increments, so
    /// this times out and says which operation never arrived. The
    /// deadline can only produce a false FAILURE, which is the safe
    /// direction and the opposite of what it replaced.
    fn await_arrival(dev: &FileDevice, operation: &str) {
        let deadline = std::time::Instant::now() + UNBLOCKED_WITHIN;
        while waiting_at_the_lock(dev) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "{operation} never reached io_lock. It either never ran, or it \
                 does not take the lock at all — which is the exclusion this \
                 test exists to assert"
            );
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    /// Releases the holder's guard and THEN joins the worker, on every
    /// path out of a test including a panicking assertion.
    ///
    /// Both halves are rust-fs-core#105. Discarding the `JoinHandle`
    /// let a failing assertion unwind the test thread while the worker
    /// was still inside `read_at`/`write_at`/`flush` with the file
    /// open, so `Cleanup` removed the temp file underneath it — a race
    /// on exactly the run where a real regression is being diagnosed,
    /// and a leaked file per failure on a platform that will not unlink
    /// an open file.
    ///
    /// THE ORDER IS NOT INCIDENTAL. Joining first would wait for a
    /// worker this very thread is blocking, and the test would hang
    /// instead of failing. Dropping the guard is what lets the worker
    /// finish so the join can return.
    ///
    /// Declared after the `Cleanup` it protects, so it drops first and
    /// the file still exists when the worker touches it.
    struct ReleaseThenJoin<G> {
        held: Option<G>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl<G> ReleaseThenJoin<G> {
        /// Let the blocked operation through, keeping the join.
        fn release(&mut self) {
            self.held = None;
        }
    }

    impl<G> Drop for ReleaseThenJoin<G> {
        fn drop(&mut self) {
            self.held = None;
            if let Some(worker) = self.worker.take() {
                // Ignored on purpose: this runs while unwinding a
                // failed assertion, and a panic in a drop during
                // unwinding aborts the process, which would replace the
                // test's own message with nothing.
                let _ = worker.join();
            }
        }
    }

    /// THE WORKER IS JOINED BEFORE THE TEMP FILE GOES, ON THE PANIC
    /// PATH SPECIFICALLY.
    ///
    /// rust-fs-core#105 is a failure-path defect, so the only way to
    /// witness it is to fail on purpose. The four exclusion tests
    /// discarded their `JoinHandle`, so a failing assertion unwound the
    /// test thread while the worker was still inside the operation with
    /// the file open, and `Cleanup` removed the file underneath it —
    /// worst on the one run that matters, the run where a real
    /// regression tripped one of them.
    ///
    /// This reproduces that shape with the file replaced by a recorder,
    /// because the ordering is what is being asserted and a removed
    /// file cannot say when it went. With the join dropped the recorder
    /// sees `cleanup` first; with the release and the join in the order
    /// [`ReleaseThenJoin`] fixes, it sees `worker` first.
    ///
    /// **Two panics are printed while this test runs, and both are the
    /// test working.** The first is the deliberate one. The second is
    /// the worker's: unwinding past a held `RwLock` guard poisons it,
    /// and every acquisition in this module `unwrap`s, so the read the
    /// worker was blocked on comes back `PoisonError` instead of
    /// bytes. That is why the worker's arrival is recorded from a
    /// `Drop` rather than after the read — the ordering claim is about
    /// when the thread *ends*, and it must hold whether the read
    /// returns or unwinds.
    #[test]
    fn a_panicking_assertion_joins_the_worker_before_the_cleanup_runs() {
        use std::panic::AssertUnwindSafe;
        use std::sync::{Arc, Mutex};

        /// Stands in for `Cleanup`: same position, same drop timing,
        /// but it says when it ran.
        struct Recorder(Arc<Mutex<Vec<&'static str>>>);
        impl Drop for Recorder {
            fn drop(&mut self) {
                self.0.lock().unwrap().push("cleanup");
            }
        }

        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let (dev, _c) = rw_image("panic_joins", 0x99);

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // Declared first, so it drops last — exactly where
            // `Cleanup` sits in the four exclusion tests.
            let _recorder = Recorder(Arc::clone(&order));

            let held = dev.io_lock.write().unwrap();
            let worker = {
                let dev = Arc::clone(&dev);
                let order = Arc::clone(&order);
                std::thread::spawn(move || {
                    /// Records the worker ENDING, return or unwind.
                    struct Ended(Arc<Mutex<Vec<&'static str>>>);
                    impl Drop for Ended {
                        fn drop(&mut self) {
                            self.0.lock().unwrap().push("worker");
                        }
                    }
                    let _ended = Ended(order);
                    let mut buf = [0u8; 4096];
                    let _ = dev.read_at(0, &mut buf);
                })
            };
            let _lock = ReleaseThenJoin {
                held: Some(held),
                worker: Some(worker),
            };

            await_arrival(&dev, "read_at");
            panic!("the assertion an exclusion test exists to make, failing");
        }));

        // NAMED, NOT MERELY PRESENT. `await_arrival` panics too, and
        // an `is_err()` satisfied by that one would report a join this
        // test never exercised.
        let payload = outcome.expect_err("the deliberate panic must have unwound");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<panic payload was not a string>");
        assert!(
            message.contains("an exclusion test exists to make"),
            "the test unwound for the wrong reason, so the ordering below is \
             about some other failure: {message}"
        );

        assert_eq!(
            *order.lock().unwrap(),
            vec!["worker", "cleanup"],
            "the worker was still inside read_at when cleanup ran: an unwinding \
             test must release the guard, join the worker, and only then let the \
             temp file be removed"
        );
    }

    /// THE COUNTER COMES BACK DOWN.
    ///
    /// [`await_arrival`] is only evidence while a non-zero `waiting`
    /// means a thread is parked *now*. A missing decrement would leave
    /// it stuck above zero, every wait would return immediately, and
    /// all four exclusion tests would pass without a worker ever
    /// reaching the lock — the same unwitnessed shape they were fixed
    /// for, one level up. So the uncontended path is asserted too:
    /// every acquisition site, no contention, nothing left behind.
    #[test]
    fn an_uncontended_operation_leaves_no_thread_waiting_at_the_lock() {
        let (dev, _c) = rw_image("arrivals_settle", 0x0F);
        assert_eq!(waiting_at_the_lock(&dev), 0, "nothing has run yet");

        let mut buf = [0u8; 16];
        dev.read_at(0, &mut buf).expect("read");
        assert_eq!(
            waiting_at_the_lock(&dev),
            0,
            "read_at left an arrival behind"
        );

        dev.write_at(0, &[0x10u8; 16]).expect("write");
        assert_eq!(
            waiting_at_the_lock(&dev),
            0,
            "write_at left an arrival behind"
        );

        dev.flush().expect("flush");
        assert_eq!(waiting_at_the_lock(&dev), 0, "flush left an arrival behind");
    }

    /// A READ CONCURRENT WITH A WRITE MUST NOT PROCEED.
    ///
    /// This is the regression. Reads were briefly taken with no lock at
    /// all, which quietly removed the exclusion the original single
    /// mutex gave and left a read free to run through the middle of a
    /// `write_at` — `write_all` may become several `write` calls, and
    /// `read_at` is itself a loop, so either side can split.
    ///
    /// # Why the lock rather than the tear is the assertion
    ///
    /// The obvious test races a writer against readers and looks for a
    /// region holding bytes from both sides of the write. On a regular
    /// file that test cannot fail: a single `write` call is atomic
    /// against `pread` on both Linux and macOS, and `write_all` only
    /// splits above roughly 2 GiB, so the tear it looks for is
    /// unreachable at any size a test would use. It would pass with the
    /// fix reverted — an assertion whose outcome does not depend on the
    /// defect, which is worse than no assertion.
    ///
    /// So the exclusion itself is asserted, by holding the very guard
    /// `write_at` takes and requiring that a read cannot get past it.
    /// That is deterministic, needs no tear to be reproducible, and
    /// fails the moment `read_at` stops taking the lock.
    ///
    /// # And "cannot get past it" is observed, not timed
    ///
    /// See [`await_arrival`] and rust-fs-core#104: the reader is
    /// required to *arrive* at the lock, which a build that does not
    /// take the lock cannot do, rather than merely to not finish within
    /// a short window, which a build that does not take the lock
    /// manages easily on a loaded machine.
    #[test]
    fn a_read_cannot_proceed_while_a_write_holds_the_lock() {
        let (dev, _c) = rw_image("read_excluded_by_write", 0x5A);

        // Stands in for a write in progress: the same exclusive guard
        // `write_at` holds across its seek and write. Taken directly
        // rather than through `exclusive_guard`, so the holder is not
        // itself counted as an arrival.
        let held = dev.io_lock.write().unwrap();
        assert_eq!(
            waiting_at_the_lock(&dev),
            0,
            "the holder must not count as a waiter, or the wait below proves nothing"
        );

        let (done_tx, done_rx) = mpsc::channel();
        let worker = {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let outcome = dev.read_at(0, &mut buf);
                let _ = done_tx.send(outcome.map(|()| buf[0]));
            })
        };
        let mut lock = ReleaseThenJoin {
            held: Some(held),
            worker: Some(worker),
        };

        // PROVE THE READER IS PARKED IN THE LOCK. Not that it started,
        // and not that it failed to finish in time: that it is inside
        // the acquisition this thread is holding shut.
        await_arrival(&dev, "read_at");
        assert!(
            matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "a read completed while the exclusive write guard was held. Reads and \
             writes are not mutually excluded, so a read overlapping a write_at \
             can observe a partially written region"
        );

        // And it is blocked rather than broken: it completes once the
        // writer lets go. Without this half the test would pass against
        // a read_at that simply never returned.
        lock.release();
        let first = done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the read must proceed once the write guard is released")
            .expect("and must succeed");
        assert_eq!(first, 0x5A, "the read returned the wrong bytes");
    }

    /// AND THE EXCLUSION HOLDS THE OTHER WAY ROUND.
    ///
    /// A write must not start while a read is in progress, or the read
    /// it interleaves with is the one that tears. Asserted with a shared
    /// guard, which is what a Unix reader holds.
    #[test]
    fn a_write_cannot_proceed_while_a_read_holds_the_lock() {
        let (dev, _c) = rw_image("write_excluded_by_read", 0x11);

        // Stands in for a read in progress.
        let held = dev.io_lock.read().unwrap();
        assert_eq!(waiting_at_the_lock(&dev), 0, "the holder is not a waiter");

        let (done_tx, done_rx) = mpsc::channel();
        let worker = {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let _ = done_tx.send(dev.write_at(0, &[0x22u8; 4096]));
            })
        };
        let mut lock = ReleaseThenJoin {
            held: Some(held),
            worker: Some(worker),
        };

        await_arrival(&dev, "write_at");
        assert!(
            matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "a write completed while a read guard was held; a write_at may not \
             run through a read that is already in progress"
        );

        lock.release();
        done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the write must proceed once the read releases")
            .expect("and must succeed");

        let mut buf = [0u8; 4];
        dev.read_at(0, &mut buf).expect("read back");
        assert_eq!(buf, [0x22; 4], "the write did not land");
    }

    /// AND A FLUSH IS A WRITE FOR THIS PURPOSE.
    ///
    /// `flush` pushes buffered bytes at the file and calls `sync_data`,
    /// so it must not interleave with a read or a write any more than
    /// `write_at` may. The exclusive guard was here before this test
    /// was, and stating an invariant in a comment is not testing it:
    /// with the guard removed the whole suite stayed green.
    #[test]
    fn a_flush_cannot_proceed_while_a_read_holds_the_lock() {
        let (dev, _c) = rw_image("flush_excluded_by_read", 0x33);

        // Stands in for a read in progress.
        let held = dev.io_lock.read().unwrap();
        assert_eq!(waiting_at_the_lock(&dev), 0, "the holder is not a waiter");

        let (done_tx, done_rx) = mpsc::channel();
        let worker = {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let _ = done_tx.send(dev.flush());
            })
        };
        let mut lock = ReleaseThenJoin {
            held: Some(held),
            worker: Some(worker),
        };

        await_arrival(&dev, "flush");
        assert!(
            matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "a flush completed while a read guard was held; flush takes the lock \
             exclusively for the same reason write_at does"
        );

        lock.release();
        done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the flush must proceed once the read releases")
            .expect("and must succeed");
    }

    /// READERS STILL OVERLAP, WHICH IS THE POINT OF THE SHARED GUARD.
    ///
    /// THE OVER-CORRECTION THIS CATCHES: restoring read/write exclusion
    /// with a plain mutex, or by taking the write half of this lock on
    /// the read path, would pass both tests above and quietly undo the
    /// reader parallelism the positioned-read work existed for. Nothing
    /// else in the suite would notice, because every other assertion is
    /// about bytes and serialised readers return the right bytes.
    ///
    /// Unix only: on Windows `seek_read` moves the file pointer, so
    /// readers there take the guard exclusively on purpose and this
    /// would correctly block.
    #[test]
    #[cfg(unix)]
    fn a_read_does_not_exclude_another_read() {
        let (dev, _c) = rw_image("reads_overlap", 0x77);

        // Stands in for another reader already inside `read_at`.
        let held = dev.io_lock.read().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let worker = {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let outcome = dev.read_at(0, &mut buf);
                let _ = done_tx.send(outcome.map(|()| buf[0]));
            })
        };
        // Holds the read guard for the whole assertion, so the read
        // below completes WHILE another reader holds the lock — which
        // is the claim. No arrival wait here and no ready signal: this
        // assertion is a positive one, and a worker that has not been
        // scheduled makes it slower, never falsely green.
        let lock = ReleaseThenJoin {
            held: Some(held),
            worker: Some(worker),
        };

        let first = done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect(
                "a read blocked behind another read. On Unix the guard must be \
                 shared -- positioned reads need no cursor, and serialising them \
                 undoes the parallelism the read path was rewritten for",
            )
            .expect("and the read must succeed");
        assert_eq!(first, 0x77);
        drop(lock);
    }
}
