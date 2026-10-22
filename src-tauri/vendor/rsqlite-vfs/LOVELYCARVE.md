# LovelyCarve integration

`rsqlite-vfs` 0.2.0 from crates.io, source commit
`3d8318430bacd7d6a1aafdf5b5e54fb0fee8d451`
(https://github.com/Spxg/sqlite-wasm-rs, MIT), vendored for the SQLite preview's
process-local in-memory VFS (`core/preview/sqlite/sqlite_vfs.rs`). Upstream targets
single-threaded WebAssembly and states that it is not thread-safe.

Changes made in LovelyCarve after vendoring:

- 2026-10-03: the memvfs file table is a `std::sync::RwLock` instead of a
  `RefCell`, and `VfsAppData`'s last-error slot a `Mutex`. The VFS is one
  global shared by every preview connection, and those run on several
  threads; the `RefCell` borrow flags raced (seen as "already mutably
  borrowed" panics under parallel tests). The crate stays `no_std` apart from
  `extern crate std` for the locks. `import_db_unchecked` copies the bytes
  before taking the write lock and checks for an existing name under the same
  lock.
- 2026-10-03: `install` / `uninstall` hold a process-wide mutex around
  `sqlite3_vfs_find` + register. Two threads using the VFS for the first time
  could otherwise both register a memvfs; a file imported through one was then
  missing when looked up through the other.

`core::sqlite_vfs::tests::concurrent_sessions_share_the_vfs` exercises
concurrent use. Whether a run hits the first-registration race depends on test
ordering, so a passing run does not prove that part.

- 2026-10-12: migrate to 0.2.0. Retain thread-safe registration, replace all
  shared Rc/RefCell state (including individual file data and error slots) with
  Arc/Mutex and require Send + Sync OS callbacks. Import now uses DbTransfer
  with atomic publication and keeps encrypted/WAL bytes intact. SQLite is
  explicitly registered with default_vfs=false.

Upstream audit: [../UPSTREAM.md](../UPSTREAM.md).
