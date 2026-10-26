# Features

What this driver does today, what it refuses, and what is coming. **Every
pull request that adds, fixes or removes behaviour adds its row to
[`features.d/`](../features.d/README.md), in the same pull request**
(AGENTS.md), never an edit to this page; a release writes those rows here.
The provenance of each on-disk fact is in [clean-room.md](clean-room.md); the
reasoning behind each change is in [CHANGELOG.md](../CHANGELOG.md).

**Since** is the release a row's current state shipped in, with the pull
request that landed it: `Unreleased (#N)` in its fragment until a release,
then the version. The first release is 0.4.0. **Tracking** names the issue
for anything not finished.

States:

- **Supported**: works, and is checked against the reference tools.
- **Experimental**: works in every test, but is new. The `write` feature is
  experimental as a whole.
- **Partial**: works for part of the case, and the row says which part.
- **Refused**: recognised and refused by name, rather than misread.
- **Not supported**: neither read nor refused by name.
- **Upcoming**: an open issue with a plan.
- **Unobservable**: blocked because the reference tools cannot make an image
  that shows it, so it cannot be learned clean-room.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Superblock: identity, geometry, layout, fields, members, btree roots | Supported | 0.4.0 (#1) | | `oracle_superblock.rs` |
| Superblock copies: the highest `seq` wins, the layout copy is the fallback | Supported | 0.4.0 (#66) | | `oracle_superblock_copies.rs` |
| Btree nodes: bsets, packed and unpacked keys, interior nodes, many bsets | Supported | 0.4.0 (#1, #40) | | `oracle_tree.rs`, `oracle_formats.rs` |
| Packed key formats of every width the reference writes (whole bytes) | Supported | 0.4.0 (#90) | | `oracle_formats.rs` |
| Node header flags: btree id (16 and above too) and level | Supported | 0.4.0 (#92) | | `oracle_node_flags.rs` |
| Lazy btree cursor: a lookup reads only the nodes on its path | Supported | 0.4.0 (#40, #71) | | `oracle_cursor.rs`, `oracle_cursor_end.rs` |
| Inodes, every field by name | Supported | 0.4.0 (#93) | | `oracle_inode_fields.rs` |
| Directory entries at their hash slot, SipHash and crc32c directories | Supported | 0.4.0 (#88) | | `oracle_fs.rs`, `oracle_strhash.rs`, `oracle_collisions.rs` |
| Colliding directory entries (hash run, `hash_whiteout`) | Supported | 0.4.0 (#88) | | `oracle_collisions.rs` |
| crc64-hashed directories | Partial: found by scanning the directory | 0.4.0 (#72) | open question 17 | |
| Extents, inline data | Supported | 0.4.0 (#1) | | `oracle_fs.rs`, `oracle_aged.rs` |
| Unwritten extents (preallocated in a `nocow` file), read as zeros; `fallocate` elsewhere makes reservations, also read as zeros | Supported | 0.4.2 | #158; cached pointers are refused (none seen) | `oracle_unwritten.rs` |
| Ranged reads (a window of a file) | Supported | 0.4.0 (#70) | | `oracle_read_range.rs` |
| Data checksums: crc32c, crc64, xxhash | Supported | 0.4.0 (#1) | | `oracle_fs.rs` |
| Compression: lz4, zstd, gzip | Supported | 0.4.0 (#1) | | `oracle_fs.rs` |
| Background compression (`reconcile` extent entry) | Supported | 0.4.0 (#75) | | `oracle_bgcompress.rs` |
| Extent pointers checked against the allocator's generations, a bucket reused by the reference kernel module among them | Supported | 0.4.0 (#69, #95) | | `oracle_pointers.rs` |
| A damaged alloc node does not fail every file: a pointer's generation is checked against the bucket_gens btree when the alloc btree cannot be read, and the extent's own checksum stands in when neither can; an extent with no checksum is then refused | Supported | 0.4.2 (#186) | | `oracle_alloc_damage.rs` |
| Error extents, extent whiteouts, older inode encodings | Supported | 0.4.0 (#68) | | `oracle_refused_local.rs` |
| Extended attributes, user and trusted namespaces | Supported | 0.4.0 (#18) | | `oracle_xattr.rs` |
| Unclean filesystems, through an in-memory journal replay | Supported | 0.4.0 (#17) | | `oracle_journal.rs` |
| A clean filesystem whose superblock records a root that will not read, through the journal's copy of the roots | Supported | 0.4.2 | #166 | `oracle_journal_roots.rs` |
| A damaged journal entry older than the replay window, or newer than its last flush, is passed over as recovery passes over it; one inside the window is refused | Supported | 0.4.2 (#183) | `Writer::open_journalled` still refuses a journal holding any damaged entry | `oracle_journal_window.rs` |
| Journal sequence blacklist | Supported | 0.4.0 (#67) | | `oracle_blacklist.rs` |
| crc128 extent entries (extents over 512 sectors), partly overwritten ones too | Supported | 0.4.0 (#97) | | `oracle_extent_entries.rs` |
| Poisoned extents (flags entry): reads fail with an I/O error, as the reference's do | Supported | 0.4.0 (#97) | | `oracle_extent_entries.rs` |
| Stripe pointers (erasure coding, several devices) | Refused by name | 0.4.0 (#97) | | `src/extent.rs` unit tests |
| Encrypted filesystems | Refused | 0.4.0 (#19) | | `oracle_refused.rs` |
| Multi-device filesystems | Refused | 0.4.0 (#19) | | `oracle_refused.rs` |
| A key at a snapshot the snapshots btree does not have, overlapping extents, unknown checksum types | Refused | 0.4.0 (#65) | | `oracle_refused_local.rs` |
| A whole-file read of a file that claims more than its device holds (read it with `read_range`), and a size memory cannot hold, refused with an error rather than a panic or an abort; a symlink target over 4096 bytes | Refused | 0.4.1 (#176) | #149 | `oracle_crafted_inode_size.rs` |
| A btree node whose header disagrees with its pointer (btree id, level, min_key, max_key), a key format whose fields unpack past their width, and keys out of order in a bset | Refused | 0.4.2 (#185) | | `oracle_node_header.rs` |
| A reservation or whiteout read through a window of zeros, not a buffer the size of its key; an extent whose stored sectors run past the device, refused as corrupt before anything is allocated for it | Refused | 0.4.2 (#177) | #151 | `oracle_crafted_extent_size.rs` |
| Snapshots and subvolumes: each subvolume and snapshot read as it is (`Filesystem::resolve` and the `_at` calls) | Supported (read) | 0.4.0 (#129) | #12; the writer writes the root subvolume only (#138) | `oracle_kernel.rs` |
| Reflinked extents (`reflink_p` into the reflink btree): read, checked, and freed by the writer one reference at a time; a write into a reflinked file copies its data out of the shared extents | Supported | 0.4.0 (#128, #144, #145); writes 0.4.2 (#169) | #7, #140; a pointer covering part of an indirect extent is refused (none seen) | `oracle_kernel.rs`, `oracle_check.rs`, `oracle_refused_local.rs`, `oracle_write_local.rs`, `write_oracle.rs` |
| Casefolded directories (`--casefold`): names listed as given, found in any case, non-ASCII included (Unicode full folding and NFD) | Supported | 0.4.0 (#98, #126) | folding tables are a newer Unicode than the reference's 12.1 (open question 18) | `oracle_casefold.rs` |
| Per-inode options: data_checksum, compression, background_compression and data_replicas read as the reference tool reports them, inherited ones included (`Inode::option`) | Supported (read) | 0.4.0 (#127) | #81; the other option fields are read raw, no image sets them | `oracle_kernel.rs`, `oracle_inode_fields.rs` |

## Checking

Fixture producer FUSE lifecycle: owned mount IDs are detached even when the
endpoint is disconnected; readiness requires a live daemon and a connected
mount. Native refusals retain their stderr and exit status without retrying or
rewriting the image (Unreleased #236, tracking #235). Lifecycle controls are in
`tests/scripts/guest-build-fixtures-cleans-up.sh`; independent native crc128
overwrite and full fixture/oracle CI validation remain required.

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| `fsck.bcachefs`, check-only: nodes, key order, dirents, link counts, extents, data checksums; fsck(8) exit status | Supported | 0.4.0 (#39) | | `check_oracle.rs`, `oracle_check.rs` |
| Damaged superblock copies reported | Supported | 0.4.0 (#66) | | `tests/cli/test-fsck.sh` |
| An extent or a size near the top of its range judged without overflowing: an extent far past the end of its file is reported | Supported | 0.4.0 (#175) | #150 | `oracle_check_overflow.rs` |
| `fsck.bcachefs` on snapshots, subvolumes and reflinks: each subvolume judged as its snapshot sees it, entries naming subvolumes, reflink ranges and refcounts | Supported | 0.4.0 (#144) | #139 | `oracle_check.rs`, `oracle_refused_local.rs` |
| `fsck.bcachefs` on xattrs and inode back-references: an xattr whose inode is gone or that its name's hash slot cannot reach, an inode whose `bi_dir`/`bi_dir_offset` names no entry or another inode's, an inode whose has-options flag disagrees with its options | Supported | 0.4.2 | #163 | `oracle_check_namespace.rs` |
| `fsck.bcachefs` on the allocation btrees: a bucket whose generation in `bucket_gens` is not its alloc key's; a data extent, or a piece of an uncompressed checksummed region, with no backpointer at its device sector plus its offset into the region; a bucket holding written data whose alloc key gives it another data type than user; on one device, a partly filled bucket with no fragmentation lru entry, an lru entry no bucket's fill gives, a bucket waiting for discard with no need_discard key, a need_discard key no such bucket gives, and the device's accounting of a data type out of step with its alloc keys | Supported | 0.4.2 | #163 | `oracle_check_alloc.rs` |
| One damaged inode node the reference refuses for a reason its report cuts off (`default.img` seed 44) | Not supported for one measured disagreement class | | #219 | `differential.yml` report 38579070302 |
| Repair | Not supported | | | |

## Writing (`--features write`)

Every image a write test makes passes the reference checker (`fsck -n`) and
reads back byte for byte through the reference implementation's mount
(`write_oracle.rs`, in the guest).

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Create a file in an existing directory | Experimental | 0.4.0 (#29) | | `write_oracle.rs` |
| A create while the inode cursor's next number is in use: the next free number is taken | Experimental | 0.4.0 (#117) | how the reference picks one is unobserved (#107) | `oracle_write_local.rs`, `write_oracle.rs` |
| mkdir, unlink, rmdir, rename; the last link's removal takes the file's extended attributes with it | Experimental | 0.4.0 (#30, #178) | #161 | `write_oracle.rs`, `oracle_unlink_xattrs.rs` |
| rename over an existing file, and a directory moved to another directory | Experimental | 0.4.0 (#118) | renaming over a directory is refused (#104) | `oracle_write_local.rs`, `write_oracle.rs` |
| File data inline or in extents, as the reference lays it out | Experimental | 0.4.0 (#86) | | `oracle_inline_limit.rs`, `write_oracle.rs` |
| Writing at an offset, appending, truncating to any size (the file is rewritten whole); truncating up leaves the new range a hole | Experimental | 0.4.0 (#120); holes 0.4.2 (#156) | | `oracle_write_local.rs`, `oracle_write_sparse.rs`, `write_oracle.rs` |
| A write or truncate whose new size is larger than the device, refused before anything is allocated (the file is rewritten whole, holes as allocated zeros) | Refused | 0.4.2 (#196) | #156 (holes kept as holes) | `oracle_write_large_offset.rs` |
| Data in allocated buckets, freed on unlink and rewrite, pieces of uncompressed checksummed regions included (a piece of a compressed region is refused) | Experimental | 0.4.0 (#35, #36); pieces 0.4.2 (#169) | | `write_oracle.rs`, `oracle_write_pieces.rs` |
| A write refused for space leaves the file as it was: data leaves 32 buckets for the btree nodes a commit rewrites | Experimental | 0.4.1 (#179) | #168 | `oracle_failed_write.rs` |
| Buckets freed in a session used again by its later writes | Experimental | 0.4.0 (#133) | buckets of generation 16 or more wait (#132) | `oracle_write_local.rs`, `write_oracle.rs` |
| Data on blocks larger than 512 bytes | Experimental | 0.4.0 (#89) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Commits through the journal | Experimental | 0.4.0 (#37) | | `write_oracle.rs` |
| Journal reclaim: a full journal is written into the nodes in place and the session goes on | Experimental | 0.4.0 (#125) | refused when a continued journal has no clean field or holds interior node keys (#113) | `oracle_write_local.rs`, `write_oracle.rs` |
| A journal left for replay is continued by the next writing session (`Writer::open_journalled`) | Experimental | 0.4.0 (#46) | | `oracle_write_local.rs`, `write_oracle.rs` |
| A session longer than its journal: the entry that does not fit is refused, so nothing a replay needs is overwritten | Refused | 0.4.0 (#112) | #113 (journal reclaim) | `oracle_write_local.rs` |
| Symlinks, hard links, mode and owner | Experimental | 0.4.0 (#43) | | `write_oracle.rs` |
| Extended attributes, any name length, at the slot the reference uses, on SipHash and crc32c inodes, colliding names in hash runs with whiteouts | Experimental | 0.4.0 (#43, #91, #119) | crc32c xattr slot observed (#106) | `oracle_xattr_slots.rs`, `oracle_write_local.rs`, `write_oracle.rs` |
| Colliding names (hash runs, whiteouts) | Experimental | 0.4.0 (#88) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Names in crc32c directories | Experimental | 0.4.0 (#88) | | `oracle_write_local.rs`, `write_oracle.rs` |
| A new btree root, any btree id | Experimental: no write needs an id of 16 and above yet | 0.4.0 (#92) | #80 | `oracle_node_flags.rs` |
| A full btree node, rewritten or split; a root grows a level | Experimental | 0.4.0 (#45) | | `oracle_write_local.rs`, `write_oracle.rs` |
| Writes on 4096-byte blocks with 32 KiB nodes, and on nodes smaller than their buckets (one node per bucket) | Experimental | 0.4.0 (#124) | how the reference shares a bucket between nodes is recorded, not copied (#109) | `oracle_write_local.rs`, `write_oracle.rs` |
| Data on every data checksum, and compressed with lz4, gzip or zstd as the filesystem asks (stored as incompressible when that saves no block) | Experimental | 0.4.0 (#122, #146) | gzip extents are judged by the reference kernel module (#141) | `oracle_write_local.rs`, `write_oracle.rs`, `oracle_kernel.rs` |
| Per-inode options: a new inode takes its directory's, and data is checksummed and compressed as the file's options say (the filesystem's where it has none) | Experimental | 0.4.0 (#147) | #81, #142; more than one replica is refused (one device) | `oracle_write_local.rs`, `oracle_kernel.rs` |
| Names in casefolded directories | Refused | 0.4.0 (#98) | | `oracle_casefold.rs` |
| Encrypted, multi-device filesystems | Refused | | | |
| A filesystem whose root subvolume is snapshotted | Refused | 0.4.0 (#143) | #138; writing at the root's own snapshot is not implemented | `oracle_write_local.rs` |
| Files inside another subvolume or a snapshot | Not written: the writer's keys are at the root subvolume's snapshot only | | #138 | |

## Oracles

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| The reference kernel module, in a VM of its own on the CI runner: an image it writes reads back here as its mount reported | Supported | 0.4.0 (#115) | | `oracle_kernel.rs` |

## Interfaces

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| C ABI: mount (path or fs_core device), volume info, stat, readdir, directory iterator, read, readlink, listxattr, getxattr | Supported | 0.4.0 (#34) | | `capi.rs`, `oracle_capi.rs` |
| `fs.bcachefs info`, `ls`, `cat`, `stat`, `tree` (`--features cli`) | Supported | 0.4.0 (#33) | | `tests/cli/` |
| `fs.bcachefs` write verbs: `put`, `mkdir`, `rm`, `rmdir`, `mv`, `ln`, `chmod`, `chown`, `setfattr`, `rmfattr` (experimental; the `cli` feature now includes `write`) | Experimental | 0.4.0 (#46) | | `tests/cli/test-write.sh`, `write_oracle.rs` |
| C ABI writes (`write` feature): read-write mount, create, mkdir, unlink, rmdir, rename, symlink, link, write_file, chmod, chown, setxattr, removexattr; negative errnos | Experimental | 0.4.0 (#123) | | `oracle_capi_write.rs`, `write_oracle.rs` |
| C ABI writes through an fs_core device (`fs_bcachefs_mount_rw_with_fs_core_device`): a file, a slice or host callbacks; a device that cannot be written refused at open with EROFS | Experimental | 0.4.0 (#188) | | `oracle_capi_write.rs`, `write_oracle.rs` |
| C ABI errnos: every failure, read or write, names its errno as the platform numbers it (`fs_bcachefs_last_errno`, and the writes' negative returns) | Supported | 0.4.0 (#188) | | `oracle_capi.rs`, `oracle_capi_write.rs` |
| Capacity and free space (`fs_bcachefs_statfs`, `Filesystem::usage`): the member's buckets and the freespace btree's runs | Supported | 0.4.0 (#188) | | `oracle_capi.rs` |
| Inode times through the C ABI (`fs_bcachefs_stat_times`): access, modification and change time since the epoch; the birth time is not read | Supported | 0.4.0 (#188) | | `oracle_capi.rs`, `write_oracle.rs` |
