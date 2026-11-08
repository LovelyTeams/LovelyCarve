# rust-fs-bcachefs

A pure-Rust bcachefs driver with a C ABI (`fs_bcachefs_*`) and command-line
tools (`fs.bcachefs`, `fsck.bcachefs`), written clean-room: from prose
documentation and black-box observation of images, never from the GPL
implementation. MIT-licensed.

Reading is the supported use. Writing is being built behind the `write`
cargo feature, off by default: every image it writes in the tests passes the
reference checker and reads back through the reference implementation's
mount, but it is new, partial, and has not been used on data anyone cares
about.

## Status

Reading is supported for single-device filesystems, clean or not, with
every checksum and compression the reference writes, and encrypted ones
whose master key is stored unsealed (their uncompressed data); `fsck.bcachefs`
checks without repairing; writing is experimental. **[docs/features.md](docs/features.md)
is the full list**: every feature, its state (supported, experimental,
partial, refused, upcoming, or unobservable clean-room), the release it
shipped in, its tracking issue and the test that checks it. Every pull
request that changes behaviour records its row in
[features.d/](features.d/README.md), which a release writes into it.

A create that would need a btree node with no room left is refused before
anything is written, until splitting full nodes lands. A kernel mount has not
been used as a judge: the harness has no kernel whose bcachefs module reads
what the reference tools write, so the reference implementation's own mount
stands in for it.

## Clean room

bcachefs's kernel code and userspace tools are GPL-2.0. Nothing from them is
in this repository: not copied, not translated, not linked, not a dependency.
The reference tools are used only as a test oracle, run inside a disposable
Linux VM, and are referred to by role. `docs/clean-room.md` records every
source used and what it told us, so the provenance of each fact is auditable;
what could not be learned that way is listed there as an open question.

## Test contract

- **unit** (`chore test:unit`): no tool, no fixture, no VM; debug profile with
  overflow checks on; and the fuzz targets' decoders with checksums switched
  off (`--features fuzzing`).
- **oracle** (`chore test:oracle`): every fixture the reference tools made in
  the Linux VM (`chore fixtures`), compared with what they recorded about it.
  A missing fixture fails the test; nothing skips.
- **vm** (`chore test:vm`): the same, compiled and run inside the guest, plus
  the tests that need the reference tools there: the write path and the
  checker against the reference checker.
- **cli** (`chore test:cli`): the installed tools, found on PATH.
- **coverage** (`chore coverage`) and **semver** (`chore check:semver`).

Every tier runs through rust-fs-core's `tier.sh`, quiet and output-budgeted,
with an executed-test floor from `test-floor.sh`.

Mutable image copies have scoped owners and live below one private scratch
root per test command. The unit, oracle, coverage and guest commands use
`scripts/test-scratch-run.sh` to remove that root after success, failure or
termination. A later run recovers marked roots whose supervisor and children
have died. Fixtures remain inputs, outside those roots. Set `CARGO_TARGET_DIR`
to a fast build disk to choose the scratch location too. Use the same wrapper
for a direct `cargo test` invocation so aborted tests also get run cleanup.

## Building

```sh
chore siblings     # ../rust-fs-core and ../fs-linux-test-harness at pinned refs
cargo build
chore staticlib    # dist/libfs_bcachefs.a and its headers
chore cli:install  # the tools, staged in tmp/cli
```

## Listing trees

`fs.bcachefs disk.img ls -R --max-depth 8 --limit 1000 /dir` walks in
byte-sorted depth-first order; `tree` is an identical alias of `ls -R`.
Each existing ls entry adds a volume-absolute `path` and relative `depth`
(direct children are depth 1, a starting file or symlink is depth 0).
Non-UTF-8 paths also carry `path_hex`. Plain `ls` still lists one directory.
Symlinks are never followed. Subvolume and snapshot roots are entered and
marked with their `subvolume` and `snapshot` IDs; repeated directory
identities (inode plus snapshot) carry `revisited: true` and are not entered.

Defaults are depth 64 and 100000 entries. Exact fits succeed; one further
entry beyond either bound, or an unreadable directory, exits 1 with the
collected JSON array (or `--text` lines) on stdout and the bound or path in
the stderr error. A missing starting path prints no report. Bounds accept
zero and require `--recursive` on `ls`; an empty directory fits zero bounds.

## One path's metadata

```sh
fs.bcachefs disk.img stat /etc/hostname
fs.bcachefs disk.img stat --text /var/log
```

`stat PATH` reports one path without following it when it is a symlink.
The shared keys come first, in this order: `path`, `inode`, `type`, `mode`,
`uid`, `gid`, `nlink`, `size`, `allocated_bytes`, then `atime`, `mtime`,
`ctime` and `btime` (the creation time) as signed seconds since the epoch,
each with `*_nsec` beside it. `type` is `file`, `dir`, `symlink`, `char`,
`block`, `fifo` or `socket`. A symlink adds `target`; a device node adds
`major` and `minor`, null because the device number's encoding has not been
observed. A non-UTF-8 path is resolved as bytes and adds `path_hex`; paths
inside subvolumes and snapshots resolve as `ls -R` walks them.

The `bcachefs` object holds `snapshot`, `sectors`, `flags` (by name, with
any unnamed bits as one hex value), `generation`, and the inode's
`data_checksum`, `compression` and `background_compression` options (null
when it carries none). A missing path is exit 1 with the shared stderr
error. Compatibility change: the earlier `ino`, `sectors` and
`atime_ns`/`mtime_ns`/`ctime_ns` keys are gone; `inode`, `bcachefs.sectors`
and the seconds plus nanoseconds replace them.

## Reading files

`fs.bcachefs` reads an image or device directly, without mounting it:

```sh
fs.bcachefs disk.img read /hello.txt
fs.bcachefs disk.img read /big/random.bin -o random.bin
```

`read` streams regular-file bytes to stdout. With `-o FILE`, it writes
`FILE.partial` and renames that file to `FILE` after a successful read;
an existing partial is refused. `cat` is an identical alias. Both refuse
directories, special files and symlinks; a symlink error names its target
so it can be read explicitly. Errors use JSON on stderr by default, or
text with `--text`.

## Filesystems inside whole-disk images

`--offset BYTES` is the unsigned byte offset of the filesystem within the
target file or device, as reported by a partition probe. It defaults to zero
and applies to every `fs.bcachefs` verb, including experimental writes and
their preflight checks. It never means a position within a file's contents.
The global option can appear before the target, before the verb, or after
the verb's arguments:

```sh
fs.bcachefs --offset 1048576 whole-disk.img info
fs.bcachefs whole-disk.img --offset 1048576 ls /
fs.bcachefs whole-disk.img read /hello.txt --offset 1048576 -o hello.txt
fs.bcachefs whole-disk.img write /hello.txt --from hello.txt --offset 1048576
```

Malformed or overflowing offsets return the shared exit-2 argument error.
An offset at or beyond the target's end fails before any filesystem operation.
Unimplemented verbs still refuse before opening a target.

## Writing files (experimental)

The `cli` feature includes the library's experimental `write` feature:

```sh
printf 'hello\n' | fs.bcachefs disk.img write /hello.txt
fs.bcachefs disk.img write /hello.txt --from hello.txt --mode 640
fs.bcachefs disk.img write /hello.txt --from hello.txt --journal
```

`write PATH` creates or replaces a regular file from stdin, or from
`--from FILE`. Empty input truncates the file. `put` is an identical alias.
`--mode OCTAL` sets permissions on creation (default `644`); replacement
preserves the existing mode. `--journal` commits through the library's
journal path and can continue an earlier journalled session. In-place
writes require a clean image and are not crash-safe.

Capability, path and input refusals are checked before opening the image
for writing. Symlinks are refused. Errors use the shared stderr contract:
status 1 for failure, 2 for invalid arguments and 3 for unsupported operations.

## Filesystem properties

`get [KEY]` and `info [KEY]` are identical:

```sh
fs.bcachefs disk.img get
fs.bcachefs disk.img info label --text
fs.bcachefs disk.img get bcachefs.uuid
fs.bcachefs disk.img set label BACKUP     # unsupported: exit 3
fs.bcachefs disk.img resize 20G --force   # unsupported: exit 3
```

JSON is the default. The ordered shared keys are `fs`, `label` (null when
empty), `total_bytes`, `free_bytes`, `block_size`, `dirty`, and `bcachefs`.
Capacity counts the member's usable buckets; free space counts the free
bucket runs, both in bytes, as in the C ABI's filesystem statistics.
`dirty` means the superblock is not marked clean, even after in-memory
journal replay. The `bcachefs` object holds `uuid`, `version`,
`btree_node_size`, `devices`, `encrypted` and `clean`.

Selection returns an object such as `{"bcachefs.uuid": "..."}`; `--text`
prints that value alone. Unknown keys return exit 2. `set KEY VALUE` and
`resize SIZE [--force]` return exit 3 before opening the target, without
stdout or image changes; errors use the shared stderr format.

Compatibility change: `info` previously returned filesystem-specific fields
at the top level and an empty label as `""`. Those fields now live under
`bcachefs`, and an empty label is null. Full reports require readable
filesystem trees for the free-space count.

## Changelog

The latest releases; every release, with the reasoning behind each change, is in [CHANGELOG.md](CHANGELOG.md).

### v0.4.2 — 2026-10-14

- Writes: a file with preallocated space keeps it reading as zeros, a reflinked file and one the reference split into pieces of checksummed regions can be rewritten, and truncating up leaves the new range a hole.
- The checker reports what the reference checker reports on the allocation btrees (bucket generations, data types, backpointers, the fragmentation lru, need_discard and accounting, on a filesystem shut down cleanly) and on xattrs and inode back-references.
- Unwritten extents read as zeros; a damaged alloc node, root or journal entry no longer makes the filesystem unreadable.

### v0.4.1 — 2026-10-12

- A file whose inode claims an impossible size is refused with an error instead of aborting the reading process, and a symlink target over 4096 bytes is refused as damage.
- A write refused for space leaves the file as it was: data leaves buckets for the btree nodes a commit rewrites.
- The first release published through crates.io trusted publishing.

### v0.4.0 — 2026-10-12

- The first release.
- Reads single-device, unencrypted filesystems, clean or not, with their subvolumes, snapshots and reflinks.
- `fsck.bcachefs` checks without repairing.
- Writing, through the Rust API, the tools and the C ABI over a path or an fs_core device, is experimental behind the `write` feature.

## Licence

MIT. See `LICENSE`.
