# Clean-room record

This crate reads bcachefs. bcachefs's implementation (the kernel's
`fs/bcachefs` and the userspace tools) is GPL-2.0; this crate is MIT. To keep
it that way, the on-disk format is learned only from:

1. **prose documentation** of the format, and
2. **black-box observation**: hexdumps of images the reference tools made, and
   the reference tools' own printed output about those images.

No GPL source was read, in any form (repository, crate, search snippet), by
anyone writing this crate. The reference tools are built and run only inside
the disposable Linux test VM (`scripts/vm-setup.sh`), are never linked or
copied here, and are named by role: the **reference formatter**
(`format`), the **reference superblock printer** (`show-super`), the
**reference lister** (`list`), the **reference checker** (`fsck`), the
**reference key editor** (`kvdb`, which reads and sets btree keys by field
name), and the **reference implementation**: the tools' userspace copy of
the filesystem mounted through FUSE (`fusemount`), which ages the `aged`
fixture inside the VM by ordinary file operations.

Every source used is listed below with what it told us. A fact that could not
be learned this way is an **open question**, not a guess.

## Sources

| # | Source | Kind | What it told us |
|---|---|---|---|
| S1 | "bcachefs: Principles of Operation", https://bcachefs.org/bcachefs-principles-of-operation.pdf (first read 2026-10-06 at title page "Revision 1.39.7+915c4e1"; re-read 2026-10-12 at "Revision 1.39.7+efec39c", the PDF with SHA-256 `50e95f298a3f877c39814fe71e14a7412c6a72d85f6e75cd78cf538767ad699e` -- see "The specification's own licence" and "S1's revisions" below) | prose documentation | Superblock at sector 8 (4 KiB), layout copy at 3584 bytes; superblock carries UUIDs, label (32 bytes), block size, btree node size, device count, version and minimum version, seq, typed variable-length fields (journal, members_v2, clean, ...); 28 btrees by name; `struct bpos {u64 inode; u64 offset; u32 snapshot}` and `struct bkey {u8 u64s; u8 format; u8 type; u8 pad; bversion; u32 size; bpos p}` as documented C declarations; packed keys use a per-node `bkey_format` with a base offset and bit width for six fields (inode, offset, snapshot, size, version_hi, version_lo) and keep a 3-byte header; extent key positions are the END of the extent; btree node = header (checksum, magic, seq, flags with btree id and level, min/max key, format, first bset) followed by `btree_node_entry` bsets; extent values are a list of entries whose type is the position of the first set bit of the first word; crc32/crc64/crc128 entry sizes (8/16/24 bytes); pointer has a device, a sector offset and a generation (the offset's 44 bits are S4's: efec39c no longer gives a width); list of key types and metadata versions in order. |
| S2 | bcachefs-tools `INSTALL.md` at v1.39.7 (build instructions only) | prose documentation | The reference tools' build dependencies, minimum Rust and the `BCACHEFS_FUSE=1` switch for their FUSE mount, used only to build the oracle inside the VM. Contains no format information. |
| S3 | Reference tools v1.39.7, run in the test VM | black-box oracle | `show-super`, `list`, `fsck` output for each fixture; recorded in `.vm-share/fixtures/*.json` and `*.txt` beside the images. |
| S4 | Hexdumps of fixture images, and candidate computations over their bytes (checksum conventions) | black-box observation | See the per-structure notes below. |
| S5 | The LZ4 block format description, https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md (BSD-2-Clause documentation of a public format) | prose documentation | Token, literal-length and match-length encoding, little-endian 16-bit offsets, overlapping matches: the decoder in `src/compress.rs` is written from it. |
| S6 | Published check values: CRC-32C of "123456789" (0xe3069283), CRC-64/WE of "123456789" (0x62ec59e3f1a4f00a), XXH64 of the empty input with seed 0 (0xef46db3751d8e999) | public reference values | Unit-test anchors for the checksum implementations. |
| S8 | The reference implementation mounted through FUSE in the test VM (tools v1.39.7 built with `BCACHEFS_FUSE=1`, per S2), driven by ordinary file operations (`scripts/guest-age.py`) | black-box oracle | The `aged` and `aged-unclean` fixtures: what a running filesystem writes (inline data, narrow key formats, nodes of many bsets, link counts, an unclean shutdown), with the inode numbers and link counts the mount reported. |
| S9 | J.-P. Aumasson and D. J. Bernstein, "SipHash: a fast short-input PRF" (2012), https://www.aumasson.jp/siphash/siphash.pdf, and its published test vector | prose documentation | The SipHash-2-4 algorithm `src/siphash.rs` is written from. |
| S10 | The package repository index at https://apt.bcachefs.org/ and its `Packages` metadata (package names, versions, dependencies), fetched 2026-10-07 | metadata | That the reference kernel module is packaged for DKMS and needs kernel headers 6.16 or newer, and (fetched again 2026-10-09) that the `trixie` suite carries `bcachefs-kernel-dkms` 1:1.39.7, the tools' pinned version. Since #110 a Debian 13 container on the CI runner installs that package, as an oracle at arm's length: DKMS builds the module in the container, which is discarded with its source, and the module runs in a VM QEMU boots there (`scripts/kernel-oracle.sh`). No package was opened on a workstation. |
| S7 | `bcachefs-tools` GitHub API metadata (tag list, `Cargo.toml` `rust-version` field only) | metadata | Which release to pin (v1.39.7) and the minimum Rust to build it with in the VM. No source file was opened. |
| S11 | Y. Nir and A. Langley, "ChaCha20 and Poly1305 for IETF Protocols", RFC 8439 (2018), https://www.rfc-editor.org/rfc/rfc8439 | public specification | The ChaCha20 state (constants, key, 32-bit block counter in word 12, 96-bit nonce in words 13..15), the Poly1305 MAC, and its one-time key as the first 32 bytes of a ChaCha20 block; test vectors 2.4.2 and 2.6.2 anchor `src/crypt.rs`. See "Encryption: btree nodes". |
| S12 | OpenSSL 3.6.5 (2026-09-29), `enc -chacha20`, run 2026-10-17 | independent cipher oracle | The final two ChaCha20 blocks for a zero key and IV `feffffff4433221188776655ccbbaa99`, encrypting 128 zero bytes, anchor the counter-boundary regression in `src/crypt.rs` (#318). Only cipher output was used; no filesystem format fact was inferred. |

## The specification's own licence

S1 is produced from the bcachefs-tools repository: its title page carries
the tools' revision ("1.39.7+915c4e1"), and it reproduces a few C
declarations from the implementation's headers (`struct bpos`, `struct
bkey`). That repository's code is GPL-2.0.

Reading it is not a GPL violation. The GPL is a copyright licence: it sets
conditions on copying, modifying and distributing the covered work, and
none on reading it. What copyright protects is *expression* -- the text of
the code and the prose -- not the facts a specification states: an offset,
a bit width, a field order, the name of a type or a feature bit. A reader
written from a specification is the ordinary clean-room route, and this
repository follows it. The exposure to manage is transcription, not
reading, so the rule for S1 is:

- **Facts, names and numbers only.** A field's position, width, order and
  name may be taken from S1 and are recorded in the per-structure notes.
- **Never transcribe.** No declaration, comment, table or passage of prose
  from S1 is copied into this crate's code, comments or docs. The Rust types
  here are field-level restatements of documented facts -- a `bpos` has
  three members in a documented order -- and nothing more.
- **Identifiers are the format's, not the implementation's.** Where this
  crate uses a name S1 uses (`bkey_format`, `btree_ptr_v2`, `inode_v3`,
  `KEY_FORMAT_CURRENT`), it is because that name is how the reference tools
  and their documentation refer to the on-disk thing, and matching the
  oracle's vocabulary is what makes the oracle tests legible.
- **Everything else is observation** (S3, S4, S8), as the notes record.

This is the authors' understanding of the position, not legal advice.

The *dependency graph* is held to the same standard mechanically:
`chore lint` runs `cargo deny check licenses` against `deny.toml`, which
allows MIT, Apache-2.0, BSD, ISC, Zlib and 0BSD and fails on anything else.

## S1's revisions

S1 is regenerated from the tools repository, so it changes under the URL.
This record was written against revision 1.39.7+915c4e1 (2026-10-06) and
re-checked against 1.39.7+efec39c (2026-10-12, #157):

- Every section cited here exists in efec39c under the same number (1.3,
  2.7, 5.5.5, 6.12, 7.1, 7.7, 9.1.2.1, 9.1.3, 9.1.3.1, 9.1.3.3, 9.1.6,
  9.1.6.2, 9.1.7, 9.1.12, 9.5.1, 9.7, 9.7.2, 9.7.5, 9.8.5, 11.2 to 11.6).
- Each fact taken from S1 still reads the same, with one exception: efec39c
  says a pointer gives a device, an offset and a generation, but no longer
  the offset's width. The 44 bits (`src/extent.rs`, bits 4..47) rest on S4,
  checked against the lister for every pointer of every fixture.
- New in efec39c, and bearing on open questions: the journal's blacklist
  entries (question 4), the snapshot tree's skiplist and ancestor bitmap
  (question 5), and encryption's key hierarchy and nonces (question 6, #159).

`scripts/s1-overlap.py` pins efec39c by its SHA-256 (#154), and
`.github/workflows/s1-watch.yml` fetches the PDF every week: when its
SHA-256 differs from the pin, it opens an issue (or comments on the open
one) to re-read the new revision against this record and move the pin.

## Per-structure notes

Each entry: the fact, and which source it came from. "Checked" means an
oracle test in `tests/oracle_*.rs` compares it against the reference tools for
all eight fixture sets.

### Superblock (`src/superblock.rs`) -- documented location, inferred layout

- At byte 4096; the layout copy at 3584 (S1). Little-endian throughout (S4).
- Offsets (S4, checked against the superblock printer): csum[16] @0x00;
  version u16 @0x10 and oldest-version u16 @0x12, each `major << 10 | minor`
  (1.39 = 0x427; the version names are S1's version history); magic[16]
  @0x18; internal UUID @0x28; external UUID @0x38; label[32] @0x48; offset
  u64 @0x68; seq u64 @0x70; block_size u16 (sectors) @0x78; dev_idx u8
  @0x7a; nr_devices u8 @0x7b; u64s u32 @0x7c (length of the fields in
  u64s); time base @0x80..0x90; flags[8] u64 @0x90; features[2] @0xd0;
  compat[2] @0xe0; embedded layout @0xf0; fields from 0x2f0.
- Layout: magic[16], type u8, sb_max_size_bits u8 (sectors, log2), nr u8,
  5 pad, then 61 u64 sector offsets (S4; "up to 61 backup locations" is S1).
- Fields: u32 u64s, u32 type, body. Type numbers follow S1's list order,
  confirmed by the printer's "Sections" line.
- Checksum: covers byte 16 to the end of the fields. Type from flags[0] bits
  2..5 (inferred): 1 = standard crc32c, 2 = CRC-64/WE (ECMA-182 polynomial,
  MSB first, init and xorout all ones), 7 = XXH64 seed 0 -- each found by
  computing candidates over a fixture (S4, S6) and checked.
- Options (inferred, checked): btree_node_size = flags[0] bits 12..27
  (sectors); metadata/data checksum option = flags[0] bits 40..43 / 44..47
  (none 0, crc32c 1, crc64 2, xxhash 3); compression option = flags[1] bits
  4..7 (none 0, lz4 1, gzip 2, zstd 3).
- Feature names: the printer lists the names of set bits in ascending order;
  pairing them gave lz4 0, gzip 1, zstd 2, new_siphash 7, ... (checked).
- members_v2: u16 member size, 6 pad, then per member uuid, nbuckets u64,
  first_bucket u16, bucket_size u16, 4 unread bytes, last_mount u64 (S4,
  checked).
- clean: flags u32, two u16 clocks, journal_seq u64, then journal entries
  (u16 u64s, u8 btree_id, u8 level, u8 type, 3 pad); type 1 = btree_root,
  matching S1's journal entry list order (S4, checked by walking the roots).

### Keys (`src/bkey.rs`) -- documented members, inferred byte order

- `bpos` and `bkey` members are S1's documented declarations. On disk the
  bpos is snapshot u32, offset u64, inode u64 (reverse of the documented
  order); an unpacked key is u64s, format (1 = unpacked), type, pad,
  version (12 bytes), size u32, bpos (S4, checked).
- Packed keys (format 0): the node's bkey_format is key_u64s u8, nr_fields
  u8, bits[6] u8, field_offset[6] u64 (S1 names the fields; S4 the layout,
  confirmed by the lister's `formats` mode). Fields are taken from the most
  significant bit of the key's words downward (S4; checked on byte-aligned
  formats only -- see open questions).
- Key type numbers follow S1's list order: extent 6, dirent 10,
  btree_ptr_v2 18, inode_v3 29 (checked); also named from that order,
  not yet seen in a fixture: error 2 (reads are I/O errors, S1 9.1.2.1),
  inode 8 and inode_v2 23 (older encodings, refused by name),
  extent_whiteout 36 (reads as a hole on a filesystem without snapshots),
  reflink_p 15 and reflink_v 16 (checked, see "Reflinks" below). S1's
  order also puts indirect_inline_data at 19, which no image has shown.

### Btree nodes (`src/btree.rs`) -- documented structure, inferred layout

- Header: csum[16], magic u64 @16, flags u64 @24, min_key @32, max_key @52,
  8 unread bytes @72, bkey_format @80, first bset @136. Bset: seq u64,
  journal_seq u64, flags u32 (low 4 bits: checksum type), version u16, u64s
  u16, keys. Later bsets are `btree_node_entry`s: csum[16] then a bset,
  starting on block boundaries, with the node's seq (S1 structure; S4
  layout, checked by key-for-key agreement with the lister).
- Node magic = 0x90135c78b99e07f5 XOR the first 8 bytes of the internal UUID
  (S4: the XOR is constant across every fixture).
- Checksum: from byte 16 of the record to the end of its keys; type 1
  standard crc32c, 2 CRC-64/WE, 7 XXH64 (S4, checked).
- btree_ptr_v2 value: mem_ptr u64 (unread), seq u64, sectors_written u16,
  flags u16, min_key, then extent pointers (S1 names; S4 layout).
- Btree ids follow S1's list order: extents 0, inodes 1, dirents 2
  (checked).
- The root key of every btree (in the clean field, and in the journal's
  root entries) is at SPOS_MAX, the all-ones position, on every fixture
  (S4, 14 to 16 roots each): the rightmost node covers everything up to
  the end of the key space. The cursor records the root key's position as
  the btree's end rather than assuming it (`tests/oracle_cursor_end.rs`).
- On every clean fixture the clean field and the journal's newest flush
  entry record the same root for every btree (S4,
  `tests/oracle_journal_roots.rs`, #166). The reference opened, and checked
  clean, images whose clean-field root had been damaged (S8, the
  differential run, #164), so the reader takes the journal's roots when a
  clean-field root does not read: its key does not decode, its node does
  not read, or the node is another btree's or level's. Which copy the
  reference prefers when both read and disagree is not observed, and the
  reader keeps the clean field's then.
- A btree can have no root recorded at all: every formatter-made fixture
  records none for xattrs (3), and `aged`, which has xattrs, does (S4). The
  reference lister lists such a btree as empty, with no error
  (`write-study/base.xattrs.txt`, S3), so the reader takes a missing root
  as a btree holding nothing. For the journal's root entries the same is
  INFERRED by analogy; no fixture has shown it.

### Inodes and dirents (`src/inode.rs`) -- documented fields, inferred encoding

- Inodes are keyed by number in the bpos OFFSET field (S3: the lister
  prints `0:4096:U32_MAX`). Root is 4096 (S3).
- inode_v3: journal_seq u64, hash_seed u64, flags u64 (bits 24..31 number of
  varint fields, bits 20..23 hash type, bits 36..51 mode), then sectors u64,
  size u64, version u64, then varints in the lister's field order with each
  time taking two varints (S3 + S4, checked for mode, size, sectors, uid,
  gid, nlink, atime, ctime, mtime on every inode).
- Varint: length = trailing one bits of the first byte + 1; 9 bytes = next 8
  verbatim; else little-endian bytes >> length (S4, checked).
- Dirent value: inode u64, DT_* type u8, NUL-padded name (S4, checked).
- Casefolded directories (#54; the `casefold` set, formatted with
  `--casefold`, S3 + S4). The superblock printer shows `casefold: 1`; every
  inode carries flag bit 10, which the lister prints as
  `has_case_insensitive` (no inode of any other image has it); `bi_casefold`
  stays 0. Each dirent's type byte has bit 7 set (0x88 for a file), then
  come two zero bytes, the name's length and the folded name's length
  (u16 each), the name as given, the folded name, and zero padding; the
  lister prints both, `Hello.TXT (casefold hello.txt)`. The folded form
  is full case folding after canonical decomposition, as far as the set
  shows: `Straße.txt` folds to `strasse.txt`, `ÉCOLE` to `e`, U+0301,
  `cole`. A dirent's slot is SipHash-2-4 of the folded name, keyed and
  shifted as for any name (all 51 entries), and the reference mount
  finds `HELLO.txt` as `Hello.TXT` (S8). The reader finds an ASCII name
  by its lowercase, which is its folded form in every case seen, and
  matches any other name as stored, by a scan; the writer refuses a
  casefolded directory (checked: tests/oracle_casefold.rs).
- One path's metadata, as `fs.bcachefs stat` reports it (#251). The four
  times are the first varint of each pair, in the order the lister prints
  them, `bi_atime`, `bi_ctime`, `bi_mtime`, `bi_otime` (S3: every inode of
  every `<set>.inodes.txt`). `otime` is reported as the creation time
  (`btime`), as its name says; the mount's lstat carries no creation time,
  so it is judged against the lister only: `btime - mtime` must equal
  `bi_otime - bi_mtime` for every stat'd path of four fixture sets, the
  time precision being 1. Generation and sectors are judged against the
  lister's `bi_generation` and `bi_sectors` the same way. The allocated
  bytes are `bi_sectors` times 512, judged against the reference mount's
  `st_blocks` (512-byte units) for the aged set and every path of the
  kernel tree, with the owner, link count and the other three times
  (tests/cli/test-stat-tree.sh, tests/cli/stat-oracle.py). bi_flags is the
  flags word's low 20 bits; only bit 10 has a name the lister has printed
  (`has_case_insensitive`), so every other set bit is reported as hex. A
  device node's numbers are open question 21. No reference source was read.
- A stored time is a SIGNED count from the time base, two's complement
  (S3, #251). The formatter copies each file's times from its source tree,
  made moments before the base: the `default` set's `/link` stores
  `bi_atime=18446744073627977305`, which is 2^64 - 81574311, beside
  `bi_otime=210425689`. Read unsigned it is a time in the year 2611; read
  signed it is 81.6 ms before the base and 0.29 s before the inode's
  creation. The CLI tier requires every time of the four stat'd sets to
  fall within a day of the superblock's "Time of last write"
  (tests/cli/test-stat-tree.sh). The C ABI's `fs_bcachefs_stat_times`
  still reads the count unsigned (#307); it is judged on the aged set only,
  whose times all follow the base.

### Extents and data (`src/extent.rs`, `src/fs.rs`, `src/compress.rs`)

- Entry types by lowest set bit (S1); bit layouts in `src/extent.rs` found by
  hexdump against the lister's printed fields (S3, S4), checked by reading
  every file of every fixture byte for byte.
- crc128 is kind 3, three words (S3, S4: the `crc128` fixture, formatted
  with `--encoded_extent_max=1M` on 512k buckets, gave six incompressible
  extents of 832 and 1024 sectors and one lz4 extent of 2048, each a 9-u64
  key). Bits 4..16 and 17..29 are the compressed and uncompressed sizes
  less one (13 bits, S1's 8192 sectors); bits 56..59 the checksum type and
  60..63 the compression type; the second word holds the checksum the
  lister prints after the colon (the low half), the third the high half,
  0 for every checksum seen. Bits 30..42 are the offset (S8, S4: the
  `crc128-overwrite` image, a 1 MiB file written through the reference
  mount with the same options, then 4K of it overwritten: the extent's
  part after the overwrite, `offset 16`, differs from the part before
  only in 16 << 30, and the file reads back to the mount's SHA-256). Bits
  43..55 are then the 13-bit nonce S1 gives encryption alone; 0 in every
  entry seen, and a non-zero value is refused.
- The flags entry is kind 6, one word, and bit 7 is `poisoned` (S8, S4: the
  `poison` image). A data sector of a settled image was corrupted and the
  file read through the reference mount: the read failed, and the
  extent's value became `0xc0`, then its crc32 and pointer, which the
  lister prints as `flags: poisoned` (its reconcile entry was dropped). A
  later mount with reconcile on moved the file's other extents and left
  that one. The reference checker passes the image.
- Data checksums: type 5 = crc32c from zero, not inverted; 6 = CRC-64/WE
  from zero, not inverted; 7 = XXH64 seed 0 (S4, checked).
- Compression numbering in crc entries: gzip 2, lz4 3, zstd 4,
  incompressible 5 (S3 + S4, checked). lz4 = bare LZ4 block; zstd = u32
  length + standard frame; gzip = raw deflate (S4, checked).
- Small files and symlink targets were stored as ordinary extents, not
  inline data, by this formatter (S3). A mounted filesystem stores small
  files as `inline_data` keys (type 17, S1's list order) in the extents
  btree: the value is the file's bytes zero-padded to a whole u64 (the
  lister's `datalen` is the value length), and the key covers `size`
  sectors ending at its position like any extent (S3, S4, S8; checked by
  reading every file of `aged`).
- How much is inline (#79): S1 (9.1.7) says the end of a file being
  written is stored inline when smaller than min(block_size / 2, 1024)
  bytes. The write study's `inline-*` images (one file of every size from
  1 to 2100 bytes, written through the reference mount) put the bound
  inclusive: on 512-byte blocks files of 1 to 256 bytes are inline only;
  a longer file has its full blocks in extents and its final partial block
  inline when that holds 1 to 256 bytes (1025 bytes: an extent of 2
  sectors, then inline data of 1), else nothing inline. On 4096-byte
  blocks 1 to 1024 bytes are inline and 1025 to 2100 are extents only.
  The inline key covers one block (8 sectors on 4096-byte blocks), and
  the inode's `bi_sectors` counts it with the extents' sectors (S8,
  checked for every size by tests/oracle_inline_limit.rs). A file grown
  from 100 to 3000 bytes holds no inline data afterwards (S8).

### What an aged filesystem added (`aged`, S8)

- Feature bits 5 = `journal_seq_blacklist_v3` and 8 = `inline_data`, by the
  same pairing as the other names (S3, checked).
- Clean shutdown: superblock flags[0] bit 1. The printer's `Clean: 1` and
  `Clean: 0` images (aged, aged-unclean) differ in flags[0] in that bit
  alone, and it is set on every formatter-made image (S3, S4, inferred).
  The reader refuses an image without it: its roots are stale until the
  journal is replayed (open question 4).
- Link counts: a file stores one less than its link count (three names,
  stored 2); a directory stores its number of subdirectories, and the mount
  reports that plus 2 (S3 against S8's `st_nlink` for every inode, checked).
- Leaf nodes with packed formats narrower than 64/64/32 and non-zero field
  offsets (`fields 16:2147485236, 8:1, 32:0, 8:0`; the lister's `formats`
  mode, recorded as `aged.<btree>.formats.txt`), and nodes of up to 78
  bsets with deletions among them: every key the lister prints is read,
  in order, with the same type, position and size (checked).

### Journal and replay (`src/journal.rs`) -- documented behaviour, inferred layout

- Documented (S1 9.7, 11.2): a ring of buckets of `jset`s with increasing
  sequence numbers; sub-entries typed in the listed order (btree_keys 0,
  btree_root 1, ..., log 9, overwrite 10, ...); btree roots recorded on
  every write; recovery replays from the newest flush entry's `last_seq`
  to that entry; entries after the last flush are not replayed; btree
  data from never-committed sequence numbers is ignored.
- `journal_v2` field (type 9): `(first bucket u64, count u64)` pairs; the
  journal's buckets are at `bucket * bucket_size` sectors (S4: every entry
  the reference lists is found at the sector it names, checked).
- jset: csum[16], magic u64 @16 (= 0x245235c1a3625032 XOR the internal
  UUID's first 8 bytes; constant across every entry, inferred), seq u64
  @24, version u32 @32, flags u32 @36 (low 4 bits checksum type; bit 5 set
  on the entries the reference calls `flush 0`), u64s u32 @40, last_seq
  u64 @48, sub-entries @56; the checksum covers bytes 16 to the end of the
  sub-entries; each entry is padded to the block size (S4, every header
  field checked against `list_journal -H` for all 201 entries).
- Sub-entry header: the same 8 bytes as the clean field's entries; keys
  are unpacked bkeys (S4; every `btree_keys` key of the replay window
  checked against `list_journal -d` by btree, position and size).
- Btree ids beyond dirents follow S1's list order; those the journals
  touch (alloc 4, lru 10, freespace 11, need_discard 12, backpointers 13,
  deleted_inodes 16, logged_ops 17, accounting 20) are checked.
- Replay in memory: the roots are the newest flush entry's `btree_root`
  entries, each with its level; bsets whose journal sequence (bset bytes
  8..16) is above that entry are ignored; the replayed keys, leaf and
  interior (the journal carries level-1 pointer updates when nodes split),
  replace the keys at their positions in the nodes they fall within, a
  deleted key or whiteout removing one. Checked: the uncleanly unmounted
  `aged-unclean` reads exactly as the reference sees it after its own
  replay, every path, listing and file byte.
### Refused filesystems

- Encryption: the formatter's `--encrypted --no_passphrase` adds a `crypt`
  field (type 2, S1's field order; the printer's `Sections` line) and the
  btree nodes no longer parse (S3, S4). The reader reads the field (see
  "Encryption: the crypt field" below) and refuses to open an image whose
  master key a passphrase seals, saying so (#270); passphrase unsealing
  remains open question 6 (#273). With an unsealed key its btree nodes are
  decrypted and read (#271, "Encryption: btree nodes"), and so are its
  uncompressed data extents (#272, "Encryption: data extents").
- Multiple devices: each member's superblock carries `nr_devices` 2 (the
  printer's `Devices`, checked); the reader refuses it.

### Encryption: the crypt field (`src/crypt.rs`) -- documented purpose, observed layout

- S1 (9.1.4.1) says what the field is for: a random 256-bit master key
  kept in the superblock, sealed by a 256-bit key derived from the
  passphrase through scrypt with parameters stored in the crypt field,
  and a magic value stored with the master key that verifies the
  passphrase. It gives no byte layout.
- The reference formatter's help (S3, `reference-help.txt`) names three
  options: `--encrypted`, `--passphrase_file` (a file holding the
  passphrase) and `--no_passphrase` (the master key is stored unencrypted
  in the superblock). The fixture build makes one image each way from the
  same tree: `encrypted` with `--no_passphrase`, and
  `encrypted-passphrase` with `--passphrase_file`, the passphrase's exact
  bytes (no trailing newline) recorded beside it as
  `encrypted-passphrase.passphrase`.
- The superblock printer shows nothing of the field but its name in the
  `Sections` line (S3, `encrypted`).
- `encrypted` (S4, hexdump of the image in the fixtures artifact of CI run
  39018127117, 2026-10-17): the field is 8 u64s, so its body is 56 bytes.
  Bytes 0..16 are zero; bytes 16..24 are the ASCII `bch**key`; bytes
  24..56 are 32 bytes of no evident structure. Taken together with the
  help's "stored unencrypted", bytes 16..24 are the magic S1 describes and
  24..56 the master key, both in the clear. That those 32 bytes are the
  key is established by decrypting every btree node with them (#271, see
  "Encryption: btree nodes" below).
- `encrypted-passphrase` (S3, S4, the fixtures artifact of CI run
  39021762764, 2026-10-17): the formatter exited 0, but wrote only its
  initial superblocks: the three copies carry seq 0 and four fields
  (ext, members_v2, crypt and members_v1, by S1's type numbers), there is
  no clean field or journal,
  and only 4 of the image's 16384 4K blocks are not zero. So the image
  holds a reference-made crypt field and nothing else; reading files from
  a passphrase image needs a complete one (#273). Its crypt field is also
  8 u64s, a 56-byte body: bytes 0..8 zero, bytes 8..16 `0e 00 03 00 04 00
  00 00`, bytes 16..56 of no evident structure, with no magic in the clear.
  S1 puts scrypt's parameters in this field, so bytes 8..16 are the
  candidates for them; their encoding is #273's to establish by
  computation, not read off these bytes.
- Since #327 the fixture build makes `encrypted-passphrase` in two steps
  instead (S3): formatted `--encrypted --no_passphrase` from the tree, as
  `encrypted` is, and then sealed by `set-passphrase`, given the recorded
  passphrase on standard input. The formatter run with `--passphrase_file`
  wrote no tree (it reports no files written, where with `--no_passphrase`
  it reports 313), and a guard in the build now fails when the image holds
  fewer than half of `encrypted`'s non-zero 4K blocks. The crypt field
  bytes above were read from the superblocks-only image; that
  `set-passphrase` writes the same layout is #273's to confirm on the new
  image. `tests/oracle_encrypted_passphrase.rs` requires the image to be
  marked clean and the reference mount, unlocked from the passphrase file,
  to read the source tree from it.
- The superblock printer will not print this image without its
  passphrase: it reports `Error reading passphrase` and exits 1 (S3), where
  it prints `encrypted` in full. That refusal is the reference's own view
  that the key is sealed.
- The rule the reader applies (`CryptField::is_sealed`): a body of 56
  bytes whose bytes 16..24 read the magic holds an unsealed key; any other
  56-byte body holds a sealed one. A shorter body is corrupt, a longer one
  has not been observed and is refused. Checked by
  `tests/oracle_refused.rs` on both images, against the printer: it reads
  the unsealed one and refuses the sealed one for want of a passphrase.

### Encryption: btree nodes (`src/crypt.rs`, `src/btree.rs`) -- documented scheme, observed layout

- S1 (2.1.2, 9.1.4) gives the scheme in prose: ChaCha20 under the master
  key with a 128-bit nonce, separate domain separators for data and for
  the Poly1305 key (`BCH_NONCE_EXTENT`, `BCH_NONCE_POLY`; no values), a
  Poly1305 MAC stored where a checksum would be, 128 bits for metadata,
  and all metadata but the superblock encrypted and authenticated. It
  says nothing of how a btree node's nonce is made or which of its bytes
  are encrypted. The ciphers are RFC 8439's (S11): ChaCha20 (2.3, 2.4), the
  Poly1305 MAC (2.5) and its one-time key, the first 32 bytes of a
  ChaCha20 block (2.6). The implementations are RustCrypto's `chacha20`
  and `poly1305` crates (MIT OR Apache-2.0), checked against RFC 8439's
  vectors 2.4.2 and 2.6.2 in `src/crypt.rs`.
- The counter boundary (#318) is checked against OpenSSL (S12): with a
  zero key, IV `feffffff4433221188776655ccbbaa99`, and 128 zero input
  bytes, the first block starts at counter `0xffff_fffe` and the second
  at `0xffff_ffff`; all three nonce words are nonzero. The regression
  checks full and partial blocks, XOR with nonzero input, and refusal
  without changing input when a request exceeds that range. RustCrypto
  0.9.1's slice wrapper reserves the final block; the reader generates
  that block once through its core after checking the complete range,
  then discards the core. This uses S11's cipher without establishing
  anything about filesystem counter wrap or carry.
- Everything below is S4: candidate computations over `encrypted.img` (the
  fixtures artifact of CI run 39021762764, 2026-10-17), with the master key
  read from its crypt field, by a scratch program outside this repository
  that walked every node from the clean field's roots: 33 nodes, 192
  records (14 btrees have a root; 9 nodes hold no keys in their first
  record). A candidate was kept only where it reproduced the stored
  128-bit MAC of every record, or decrypted a header to the bounds its
  parent's pointer names. A wrong candidate matches a 128-bit MAC by
  chance with probability 2^-128.
- **In the clear**: the magic (bytes 16..24) of every node, which matches
  the unencrypted formula above; and every bset header, whose seq equals
  the parent pointer's, whose journal sequence numbers run 0..0x2b, and
  whose checksum type is 4 on all 192. S1's metadata MAC is 128 bits, so
  type 4 is read as ChaCha20/Poly1305 with a 128-bit MAC; no other type
  has been seen in a node and none is read as a MAC.
- **The nonce** is four little-endian u32 words loaded into ChaCha20's
  state words 12..15, word 0 being RFC 8439's 32-bit block counter: word 0
  the record's byte offset in the node (0 for the first record; 512 to
  28160 for the 159 later ones), words 1..2 the node's seq (low half
  first; the seqs are random 64-bit values), word 3 `0x2000_0000` XOR the
  bset's journal sequence number. Established by the MAC (next item): with
  word 0 any other candidate (the sector, the block) the later records'
  MACs do not reproduce, and without the journal sequence number only the
  33 first records' do (their journal sequence numbers are 0).
- **The MAC** is the record's first 16 bytes. It is Poly1305 over the
  record from byte 16 to the end of its keys, as stored (ciphertext, and
  the magic or the bset header in the clear), keyed with the first 32
  keystream bytes of the record's nonce with `0x8000_0000` XORed into word
  3. Of the 16 x 16 candidates `k << 28` for the data nonce's top nibble
  and `m << 28` for the Poly1305 key's, every pair with `k ^ m = 0xa`
  reproduces all 192 MACs (only the Poly1305 nonce enters the MAC) and no
  other pair reproduces any; `k` is then fixed by decryption.
- **The header**: bytes 24..136 (flags, min and max key, the 8 unread
  bytes, the key format) are encrypted with the first record's keystream
  from its start. Known plaintext fixed it: XORing bytes 32..72 with the
  min and max keys the parent's pointer names (the root key's position,
  for a root) gives keystream bytes 8..48 of the nonce with `k = 2` on all
  33 nodes, and with no other of the 16 values of `k`, at no offset in its
  first 4096 bytes. The decrypted flags name the btree id and level the
  parent says, on all 33.
- **The keys**: the first record's keys are encrypted with the same
  keystream from byte 128 (the 64-byte block after the header's 112
  bytes); a later record's keys from byte 0 of its own nonce's keystream.
  Every one of the 24 first records with keys, and the 159 later ones,
  decodes at that offset to keys that are well-formed, in order and
  inside the node's bounds (as the reader decodes them). The interior
  nodes' keys are the child pointers the walk followed.
- What is not observed, and refused rather than guessed: a journal
  sequence number of 2^28 or more (how its high bits share word 3 with the
  domain separator), a keystream running past the 32-bit block counter
  (whether it wraps or carries), and a node's MAC of any other width.
- The reader verifies each record's MAC before decrypting it, and refuses
  a record whose MAC does not reproduce as a bad checksum naming the MAC.
  With a master key it also refuses a node or bset that carries a checksum
  instead of a MAC: S1 (2.1.2) says all metadata is authenticated.
- Checked: `tests/oracle_encrypted.rs` lists every btree of `encrypted.img`
  key for key (type, position, size, in order) against the reference
  lister, which decrypts it with the same key, and refuses the image with
  one byte of a node changed. `src/btree.rs`'s unit tests read one real
  node of it (`tests/data/encrypted-backpointers-root-39021762764.bin`,
  two records, both kinds) and refuse it with a byte changed, without its
  key, and with another key. On the same image the scratch program's walk
  of inodes and dirents gave the same positions and sizes as the reference
  lister's listing of `default`, formatted from the same tree; its extents
  differ only where three extents were split at a bucket boundary.

### Encryption: data extents (`src/crypt.rs`, `src/fs.rs`) -- documented scheme, observed layout

- S1 (9.1.4.1, 9.1.4.3) gives the scheme in prose: an extent is encrypted
  under the master key with a 128-bit nonce built from the key's 96-bit
  version, the extent's compression type and its uncompressed size,
  together with a nonce offset kept in each crc entry; data and the
  Poly1305 key have separate domain separators (`BCH_NONCE_EXTENT`,
  `BCH_NONCE_POLY`; no values); and the MAC is kept in the crc entry, cut
  to 80 bits by default (`chacha20_poly1305_80`; the `wide_macs` option
  keeps 128). It gives no byte layout.
- Everything below is S4: computations over `encrypted.img` (the fixtures
  artifact of CI run 39021762764, 2026-10-17) by a scratch program outside
  this repository. It walked the extents btree with this crate's btree
  reader (#271), found each extent's file through the dirents, and XORed
  the extent's stored sectors with the same bytes of the tree the formatter
  was given, regenerated by `scripts/fixture-tree.py` (its 307 files match
  the SHA-256 in `default.json`, formatted from the same tree). The image
  has 441 data extents, all `extent` keys, each a crc64 entry and one
  pointer: 1 to 64 sectors, compressed size equal to uncompressed,
  compression type 0, nonce field 0 and offset 0 in every one. Their keys'
  versions run from 1 to 0x1b9, one per extent in the order the files were
  written, with the top 64 of the 96 bits zero.
- **The checksum type** of every crc entry is 3, and its 80 bits are the
  crc64 entry's checksum: the 16 high bits of its first word and its
  second word. S1 calls the default data MAC `chacha20_poly1305_80`;
  tests/oracle_encrypted.rs checks that the reference lister names type 3
  so on every extent.
- **The nonce** is `[0, version, 0, 0x1000_0000]`, word 0 being ChaCha20's
  block counter, so the keystream runs from the extent's first byte. The
  search: each of the four words drawn from zero, the version's three
  32-bit parts, the uncompressed and the compressed size, the size less
  one, the size times 512, and the size and compression type packed
  together at shifts of 4 to 28 bits, with each of the 16 top-nibble values
  XORed into any one word, compared with the first 32 keystream bytes of an
  extent of each of the six sizes the image has (64, 9, 55, 23, 1 and 8
  sectors). Every hit is the same 128-bit value, whose extent domain's top
  nibble is 1. With it, all 441 extents decrypt to the source tree byte for
  byte, the zeros after each file's end in its last sector included.
- What S1 names and the fixture does not show: the uncompressed size runs
  from 1 to 64 sectors with the nonce unchanged, so it does not enter an
  uncompressed extent's nonce; how it and the compression type enter a
  compressed extent's is #275's. Nor are a version of 2^32 or more (whether
  its high bits are word 2 or share word 3 with the domain), or a data MAC
  of another width observed. A non-zero nonce offset is, below (#331).
- **Incompressible extents (#356, part of #330)** take the uncompressed
  extent's nonce. S4: candidate computations over `encrypted-compressed.img`
  (formatted `--compression=lz4`; its random file's extents carry
  compression type 5, incompressible), with each key's version as the
  reference lister prints it. All 34 incompressible extents reproduce their
  stored 80-bit MAC with `[0, version, 0, 0x1000_0000]`, so neither the
  incompressible type nor their uncompressed sizes (13 to 64 sectors) enter
  it. Its 10 LZ4 extents reproduce theirs with no nonce tried: the version
  word, and each other word, XORed with every value below 2^22 (and those
  values bit-reversed), and any two words XORed with the compression type
  (0 to 15) and one of the uncompressed or compressed size, less one or
  times 512, each shifted 0 to 31 bits. An LZ4 extent stays refused until
  its nonce is found.
- **A nonce offset (#331)** advances the block counter. S4: the
  `encrypted-compressed.img` of merge-queue run 39806426452 (2026-10-22)
  held the partly overwritten extent of #329 narrowed by the reference: key
  `2147483650:179:U32_MAX`, version 10, its crc entry 43 sectors
  compressed and uncompressed, offset 0, nonce field 21, compression type
  5, its pointer 21 sectors past the original extent's. (Other builds of
  the fixture keep the whole 64-sector entry at offset 21 and nonce 0, so
  the narrowing happens in some runs only.) Of seven placements of 21 --
  as word 0, times 8 and times 512 in word 0, as word 2, added to the
  version in word 1, and XORed into word 3, besides none -- only
  `[21 * 8, version, 0, 0x1000_0000]` reproduces its stored 80-bit MAC:
  the offset counts sectors, each eight 64-byte ChaCha20 blocks, so the
  narrowed bytes keep the keystream they had in the original extent.
- **The MAC** is Poly1305 over the extent's stored bytes (the ciphertext,
  all of its compressed size), keyed with the first 32 keystream bytes of
  its nonce with `0x8000_0000` XORed into word 3, as for a btree node. Its
  first 8 bytes, little-endian, are the crc entry's second word and the
  next 2 its high 16 bits. Of the 16 values of the Poly1305 domain's top
  nibble, the MAC over the ciphertext or over the plaintext, and six ways
  of placing 80 of its 128 bits, only this one reproduces any stored MAC,
  and it reproduces all 441. A wrong candidate matches an 80-bit MAC by
  chance with probability 2^-80.
- The reader verifies an extent's MAC before decrypting it, and refuses a
  mismatch as a bad checksum naming the MAC. In an encrypted filesystem it
  refuses an extent with no MAC (S1 9.1.4: every data pointer carries
  one), and outside one an encrypted extent, for want of a key. It refuses
  what has not been observed rather than give it a guessed nonce: a
  compressed extent, a nonce offset, a version of 2^32 or more, a checksum
  type other than 3 or 4, and an extent of which only part is live (one partly
  overwritten, #275).
- Checked: `tests/oracle_encrypted.rs` reads every file of `encrypted.img`
  against the SHA-256 in `encrypted.json`, which the fixture build writes
  from the tree it formats the image from. In the test VM the reference
  implementation, mounted over a copy of the image, reads the same tree
  (`encrypted.mounted.json`), and the reference checker passes the image.
  A byte changed in the first or last sector of an extent gets that file
  refused by the MAC while every other file reads. `src/crypt.rs`'s unit
  tests decrypt `hello.txt`'s real extent
  (`tests/data/encrypted-hello-extent-39021762764.bin`) and refuse it with
  a bit changed in its data, in either part of the stored MAC, in the
  version, or with another key.
- **The 128-bit MAC** (`wide_macs`, #335). S1 9.1.4.3 says the option
  keeps all 128 bits of the data MAC. `encrypted-wide-macs.img`, formatted
  `--wide_macs`, carries checksum type 4 (`chacha20_poly1305_128`) in every
  crc entry, by this crate's decoder and by the reference lister (#334).
  The reader takes the stored tag as the 80-bit layout carried on: the
  MAC's first 8 bytes, little-endian, in the crc entry's low checksum word
  and its next 8 in the high one. That placement is a hypothesis held to
  the reference's image, not yet a separate observation: it is kept only
  if `tests/oracle_encrypted_wide_macs.rs` reads every file of that image
  against the SHA-256 the reference mount recorded, which a wrong placement
  fails with probability 1 - 2^-128 per extent. `src/crypt.rs`'s unit test
  refuses a bit changed in either word, including the 48 bits the 80-bit
  MAC drops.

### What each write changes (the write study, `fixtures/write-study`, S8)

Each pair is the same settled base image before and after one operation
through the reference implementation's mount; every btree of both is dumped
by the reference lister (scripts/guest-write-study.sh). Keys that changed,
by operation (positions `inode:offset:snapshot`):

- **create a small file** (6 bytes): extents + `inline_data` at
  `ino:1`; inodes + the new `inode_v3` (`bi_dir` = parent, `bi_dir_offset`
  = the dirent's offset, a fresh `hash_seed`) and the parent's
  `bi_ctime`/`bi_mtime`/`journal_seq` updated; dirents + `dirent` at
  `parent:hash(name)`; accounting `nr_inodes` +1 and the per-btree
  counters of extents, inodes and dirents; logged_ops' `inode_alloc_cursor`
  `consumed` +1 and `idx` = the next inode number.
- **create an empty file**: as above without the extents key.
- **mkdir**: as create-empty, the inode 19 u64s (a directory carries one
  more field).
- **unlink**: the dirent, the inode and its inline data are deleted;
  accounting down; here the filesystem also allocated a node for the
  deleted_inodes btree (alloc, freespace, backpointers and the btree
  accounting changed with it).
- **rename** within a directory: one dirent deleted, one added at the new
  name's hash; the dirents counter's bytes change.
- **truncate to 0 / overwrite** of an inline file: the inline_data key is
  removed / replaced; the extents counter changes.
- **create a 300000-byte file**: ten `extent` keys of up to 64 sectors,
  alloc_v4 keys for the buckets used, freespace ranges shrunk, an lru
  entry, backpointers, accounting.
- The per-btree accounting key (`snapshot id=... btree=NAME a b 0`) holds
  the number of leaf keys and their total size in bytes (u64s * 8): every
  pair adds or removes exactly the keys it changed (inferred, every pair).

### Extended attributes (`src/xattr.rs`, S8)

- The xattrs btree (id 3, S1's order) holds one `xattr` key (type 11) per
  attribute at `inode:hash:snapshot`. Value: namespace u8, name length u8,
  value length le16, the name without its namespace prefix, the value,
  zero padding (S3 + S4: hexdumps against the lister's `name:value` and
  the mount's listing; checked for every path of aged and aged-unclean).
- Namespaces seen: 0 = `user.`, 3 = `trusted.`. Others are refused, not
  guessed (open question 6).
### The write path (`src/write.rs`, feature `write`) -- observed, then judged

- A change to a node is a new bset appended after its written part, at a
  block boundary: `btree_node_entry` csum[16], then the bset with the
  node's seq, a journal sequence, flags = checksum type | (start sector
  within the node << 16), the node's metadata version, u64s, keys (S8:
  the write study's create-small appended exactly one such bset to the
  dirents leaf, flags 0x30001 at sector 3; the aged image's nodes carry
  0x8c, 0x85, ... at sectors 140, 133, ...: bits 16.. are the start
  sector).
- The parent's `btree_ptr_v2` records `sectors_written`; the reference
  raised the root key's in the clean field from 3 to 4 (S8). The writer
  re-states each parent's pointer key in a bset appended to the parent and
  rewrites the root key in the clean field and every superblock copy.
- Keys are written unpacked (format 1): the lister's `formats` mode counts
  unpacked keys in a node, so they are allowed (S3), and the reference
  checker passes nodes holding them (checked).
- Judged: re-stating the newest key of the dirents, inodes and extents
  btrees of the write-study base and of the aged image (level-1 roots)
  leaves images the reference checker passes with nothing to fix.

- Selected-copy regression (#250, S3, S4, S8): on a copy of the
  reference-made write-study base, give either backup a sequence one above
  the primary and the distinct label `writer-selected-copy`, then recompute
  its checksum. A second case zeros the primary's header and fields,
  leaving the standalone layout intact. Both cases select the backup before
  creating `/d/selected-copy.txt`; every published copy must retain its
  sequence and label and record its own location. The guest regression
  requires the reference checker to find nothing to fix, the reference
  lister to name the file, and the read-only reference mount to list and
  read both the new file and the existing file. These are black-box
  observations using the header offsets above; no reference source is read.

### Creating a file (`Writer::create_file`) -- learned from the write study, judged

- Dirent offset = SipHash-2-4 keyed `(directory's hash_seed, 0)` over the
  name, shifted right by one. SipHash is a published algorithm (Aumasson
  and Bernstein, 2012; S9); which key and which shift were found by
  computing candidates against the write study's dirents (S4) and then
  checked against every dirent of every fixture, thousands in `aged`
  (tests/oracle_encode.rs). Collisions (open question 10) are refused.
- inode_v3 varints in order: atime, ctime, mtime, otime (two varints
  each), uid, gid, nlink, generation, dev, data_checksum, compression,
  project, background_compression, data_replicas, promote_target,
  foreground_target, background_target, erasure_code, fields_set, dir,
  dir_offset (a file stores 21 fields), then subvol, parent_subvol, nocow,
  depth (a directory stores 25). Varint encoding is the inverse of the
  decoding above; decoding then encoding every inode of every fixture gives
  back its bytes (checked).
- Times are nanoseconds since the superblock's time base (`time_base_lo`,
  nanoseconds since the epoch; precision 1) -- the reference's create was
  5.1e9 units after the base's, five seconds later (S8); the reference
  mount reports the mtime this writer sets (checked).
  `fs_bcachefs_stat_times` turns them into seconds since the epoch the
  same way (`time_base_lo` plus the stored count times the precision);
  the reference mount's own stat of a file this crate wrote agrees
  (tests/write_oracle.rs), and every time of a fixture falls before the
  superblock's "Time of last write" (S3, tests/oracle_capi.rs).
- The next inode number is the `inode_alloc_cursor` key (type 35) in
  logged_ops at 1:1:0, value `(u64 0, u64 next)`; the reference advanced it
  by one per create (S8).
- Accounting: `nr_inodes` at POS_MIN (value: count), and per btree the key
  at inode `0x05ffffffff000000 | id << 16` (value: keys, bytes, 0), bytes
  counted at the unpacked size (S8, every pair).
- File data is laid out as the reference lays it out (#79, above): inline
  up to min(block_size / 2, 1024) bytes, else full blocks in extents with
  a short final block inline. Data extents are cut and sized in whole
  blocks, the last zero-padded, with the checksum over the padded block
  (#87, S8: a 1500-byte file is one 8-sector extent on 4096-byte blocks,
  its crc32c that of the data and 2596 zero bytes).
  Symlink targets stay at most 248 bytes, the longest seen.
- mkdir: a 25-field directory inode with `depth` one more than its
  parent's, and the parent's stored subdirectory count raised; unlink and
  rmdir: deleted keys over the dirent, the inode and its inline data (or
  one link fewer when another name remains); rename: the old dirent
  deleted, the new one at the new name's hash, the inode's `dir`,
  `dir_offset` and ctime; rewriting an inline file: the inline key
  replaced or deleted, size, sectors and times (S8, the write study's
  mkdir, unlink, rename, truncate and overwrite pairs). Judged by the
  reference checker and mount (tests/write_oracle.rs).
- Judged: four files in the write-study base and one in the aged image
  pass the reference checker with nothing to fix, and the reference mount
  lists them and reads back their bytes, sizes and modes
  (tests/write_oracle.rs, in the guest).

### Allocating space (`src/write_alloc.rs`) -- learned by write, judge, fix

The write study's create-large pair (S8) gave the key set; the reference
checker judged each attempt and named what was missing until nothing was.

- Free space: the freespace btree (id 11) holds `set` keys (type 25) at
  `dev:end`, `size` buckets long, one per run of free buckets (S8: 0:2016
  len 1924 = buckets 92..2016). A bucket taken shrinks or removes its run.
  `Filesystem::usage` reports these runs' total as the free space and the
  member's buckets from `First bucket` on as the capacity; both are
  checked against show-super and the lister's freespace listing (S3,
  tests/oracle_capi.rs).
- The shared CLI property report (#294) uses the same capacity/free-space
  definition. A black-box VM study on 2026-10-17 recorded the superblock
  printer and freespace lister for default, labeled and 4096-byte-block
  formatter images: 2048 usable 32768-byte buckets and a free run of 1922
  buckets on each one. The formatter's help and printed report distinguish
  `--fs_label` (the filesystem label) from `--label` (a device label).
  `Label`, `External UUID`, `Version`, `Devices`,
  `block_size`, `btree_node_size`, `Clean` and the presence of a `crypt`
  section supply every remaining reported property (S3). `dirty` is the
  inverse of the printed `Clean` flag, including the existing unclean
  fixture; replay in memory does not change that observation. The fixture
  builder now records freespace listings for fresh and aged images, plus
  a labeled set. `tests/cli/test-get.sh` compares the complete JSON report
  with these independent text reports via `tests/cli/get-oracle.py`; that
  translator reads no image bytes and invokes no Rust reader.
- alloc_v4 (type 27, alloc btree id 4) per bucket: word 0 journal seq when
  it became non-empty, word 1 flags in byte 0 (0x23 on every bucket in use;
  bit 0 need_discard, bit 1 need_inc_gen), gen byte 4, oldest_gen byte 5,
  data type byte 6 (1 sb, 2 journal, 3 btree, 4 user, 9 need_discard),
  word 2 dirty sectors, word 3 read clock, word 4 write clock, word 6 the
  journal seq when it became empty (S3 + S4, field by field against the
  lister over 259 keys and the reused buckets of `aged`).
- Generations: bucket_gens (id 14) keys of type 30 at `dev:bucket/256`,
  256 one-byte generations; a pointer carries its bucket's (S3 + S4).
- Data extents: a crc32 entry (crc32c from zero, type 5; sizes minus one in
  7 bits, so at most 128 sectors) then a pointer (S4, the create-large
  extents), one bucket at most each.
- Backpointers (id 13, type 28) at `dev:sector << 16`: value btree id,
  level, data type, 5 bytes, the length in sectors u32, then the bpos of
  the key pointing at the data (S3 + S4; for a btree node, the parent key's
  position one level up).
- lru (id 10): a partly filled bucket has a `set` key at
  `(1 << 61 | dirty * 2^31 / bucket_size) : bucket` (S8, and the checker's
  "missing fragmentation lru entry at pos 2305843009549238272:101:0").
- A first lru key on a filesystem without an lru btree needs its root: an
  empty node (header flags as an existing node's with the btree id in the
  low byte, POS_MIN..SPOS_MAX, format 3 u64s 64/64/32, one empty bset), a
  btree_ptr_v2 root key at SPOS_MAX added to the clean field, and the node
  marked in the member's btree-allocated bitmap: a u64 at member offset
  128, one bit per `2^shift` sectors with the shift a byte at member
  offset 28 (S3: the printer's bitmap, most significant bit first,
  blocksize 128; the checker's "btree ptr not marked in member info btree
  allocated bitmap").
- Accounting positions are a byte string read as a big-endian bpos: kind,
  then fields (S8): 2 replicas `[data type, nr devs, nr required, devs]`
  (value: sectors), 3 dev_data_type `[dev, data type]` (buckets, sectors,
  fragmented), 5 per-snapshot btree counters (keys, bytes, data sectors),
  6 per-btree node usage (sectors, nodes), 8 inode `[inum little-endian]`
  (extents, sectors, sectors).
- Judged: files of 249 bytes to 1 MB on the write-study base and on the
  aged image pass the reference checker with nothing to fix and read back
  through the reference mount byte for byte.
- Freeing (an unlink-large pair made in the guest from create-large, S8):
  an emptied bucket's alloc key becomes data type need_discard (9), gen and
  oldest_gen one higher, need_inc_gen cleared, journal_seq_empty (word 6)
  set; the need_discard btree (id 12) gets a `set` key at
  `journal_seq_empty:bucket`; bucket_gens records the new generation;
  backpointers and lru entries go; accounting moves the bucket from user to
  `dev_data_type need_discard` (and keeps zeroed keys, as the reference
  does). Judged: unlinking, shrinking and growing large files passes the
  reference checker and the mount reads what remains (S8 checker run).
- Pieces of checksummed regions (#169; S8, read off the reference's
  `aged` and `crc128-overwrite` images with this crate's reader, against
  the lister's extents listing): where the reference overwrites part of a
  file it leaves extents whose crc entry covers more than they do, a
  region's pieces each pointing at the region's start with their offset
  into it in the crc entry. A bucket counts a piece's live sectors, not
  its region's: `aged` bucket 105 holds pieces of 111 and 120 sectors of
  one 256-sector region and counts 231, bucket 106 a 148-sector piece of a
  246-sector region and whole extents of 97 and 10 and counts 255, and
  `crc128-overwrite` bucket 56 pieces of 8 and 1008 sectors of a
  1024-sector region and counts 1016. A piece's backpointer sits at the
  region's sector << 16 plus its offset (`26880 << 16 | 136`, `57344 <<
  16 | 16`), with the piece's live length. Only uncompressed regions'
  pieces appear in any fixture.
- A bucket holding several nodes (#109): on a filesystem whose nodes are
  smaller than its buckets the reference formatter fills a bucket with
  more than one node (`write-study/base-small-nodes.img`: 64-sector nodes
  in 256-sector buckets, 17 of its 18 btree buckets holding more than one
  node's sectors). Freeing one of them takes only its sectors from the
  bucket's dirty count and one node from the btree's; the bucket is
  emptied, as any emptied bucket is, only when none is left. This writer
  puts one new node in each bucket. A partly filled btree bucket has a
  fragmentation lru entry, as a partly filled data bucket does, at
  `(1 << 61 | dirty * 2^31 / bucket_size) : bucket` (the reference
  checker's "missing fragmentation lru entry" and "incorrect lru entry" on
  such an image, CI run 38051787458): a new node's bucket gets one, and
  freeing a node moves it.
- Reusing a freed bucket (#132; the reference kernel module's
  `kernel-freed` image, S10: a 4 MiB file written, removed, and ten
  seconds before the unmount, made once by an observation build of the
  kernel oracle in CI run 38017596962, whose `fixtures` artifact keeps the
  reference lister's view of its alloc, need_discard, freespace,
  bucket_gens and accounting btrees): the reference returns a need_discard
  bucket to the free pool on a device it does not discard. Its alloc key
  becomes data type free with gen and oldest_gen kept, need_discard
  cleared and both journal sequence numbers (words 0 and 6) 0, io_time
  kept; its need_discard key goes; freespace holds it again, merged into
  the run beside it (buckets 89 and 90, both gen 1, as one run `0:91 len
  2`); accounting moves it from `dev_data_type need_discard` to `free`.
  The writer does this before any write that allocates, for buckets
  emptied by earlier commits, giving the freed buckets runs of their own
  rather than merging them into an existing run: a node the same commit
  rewrites takes its bucket from the runs on disk and rewrites the run it
  shrinks under the same key, which replaced a merged run and lost the
  buckets merged in (the reference checker, CI run 38024306750: "bucket
  incorrectly unset in freespace btree"). Judged: a 4 MiB file written and removed 25
  times through the 64 MiB base image passes the reference checker and
  reads back through the reference mount (`write_oracle.rs`). Not seen:
  whether a freespace key's position carries a generation's high bits
  once it reaches 16, so buckets of generation 16 or more are left
  waiting.

### Committing through the journal (`src/write_journal.rs`) -- judged

- A transaction is one jset in the layout the reader decodes: seq one past
  the newest entry, last_seq the session's first, flags checksum type |
  0x40 (bit 6 set on every flush entry the reference wrote), a btree_keys
  entry per btree and a btree_root entry per root, at the next free block
  of the journal ring (S4, S8).
- Accounting goes in as signed deltas with version (seq, position), which
  the replay adds once to keys of a lower version (S8: the reference's own
  journal carries `replicas user ... -14`, `inum -1 -14 -14`).
- The superblock's clean bit is cleared after the first entry, and its
  replicas_v0 field lists the journal on the device (`02 01 00`; S4, and the
  checker's "superblock not marked as containing replicas for journal
  entry", its only finding on the first attempt).
- Nothing is reclaimed: no entry's last_seq ever passes the session's
  first, so every entry since then is in the replay window. The ring may
  not come round into the bucket holding the window's first entry; the
  entry that would is refused as "the journal is full" (#101). Reclaim,
  as S1 9.7.2 describes it (write the nodes the oldest entries pin, then
  advance last_seq), is #113.
- Judged: entries 13-16 written to the write-study base (a small file, a
  large file with a new lru root, a mkdir, an unlink) are replayed by the
  reference checker with nothing to fix and read back through its mount;
  an entry left unmarked is ignored by both.

### Links, attributes and extended attributes (`Writer::symlink`, `link`, `set_attributes`, `set_xattr`) -- judged

- A symlink is an inode of mode 120777 whose target is its inline data,
  size the target's length, dirent type 10 (S8: the aged fixture's
  symlinks).
- A hard link adds a dirent; the inode's stored count rises and its
  `dir`/`dir_offset` back-reference moves to the newest name (S8: the aged
  fixture's three-name file points at the last one made).
- Permissions are the mode bits in the flags word; owner and group the uid
  and gid varints (S8 field order, above).
- An xattr's key sits at `xattr::name_slot`: SipHash-2-4 keyed `(inode
  hash_seed, 0)` over the namespace byte then the name, shifted right by
  one, with the final partial word of a longer message taken as question
  15 records (#77; every xattr of every image checked). The first xattr of a filesystem
  without an xattrs btree makes its root, as for lru.
- An empty bset is rejected by the reference checker ("empty bset"), so
  nothing is appended when an operation leaves a btree unchanged (S8,
  measured on the first attempt).
- Judged: a symlink, a second name, chmod and chown, and xattrs set,
  replaced and removed pass the reference checker, in place and through
  the journal, and the reference mount and `getfattr` show them.

### Rewriting and splitting full nodes (`src/write_nodes.rs`) -- judged

- A full node is rewritten into a fresh bucket with its live keys in one
  bset, or split in two when they would fill more than two thirds of a
  node; the parent's pointer keeps its position (the node's max key), a
  split adds one before it, and a root that splits gets a new root one
  level up (its level byte in the clean field's entry follows). The old
  bucket is freed as an emptied data bucket is (need_discard, a new
  generation), its backpointer removed.
- Node header flags: the btree id's low four bits in bits 0..4, the level in
  bits 4..8, bit 8 set, the id's higher bits from bit 9 (S4: every root of
  the aged fixture, ids 0 to 20; id 16 is 0x300, id 20 is 0x304).
- The btree-allocated bitmap covers 64 regions of 2^shift sectors; a node
  beyond them doubles the regions (shift + 1, bits folded pairwise) until
  it fits. The reference checker accepts the superset and schedules its
  own bitmap pass ("has 672k btree buckets and 3.25M marked in bitmap").
- The per-btree accounting key's third counter counts interior nodes (the
  checker's "btree btree=inodes ... should be 256 4 1" after a root split;
  the aged fixture's two-level btrees carry 1).
- Judged: 400 one-operation creates, 134 unlinks and ten 50 KB files on the
  write-study base (32 KiB nodes) pass the reference checker and read back
  through its mount.

## Open questions

Facts this reader needs that neither documentation nor black-box observation
has settled yet. Each needs a fixture that exercises it, not a guess.

1. **Packed fields that are not byte-aligned** (#82). SETTLED as "the
   reference does not write them" (S3): in the 125 node formats of `aged`
   and `large` alone, 588 fields, every width is 0, 8, 16, 32 or 64 bits,
   including 16-bit inode fields over ranges of about 470 that 9 bits would
   hold. The reference rounds a field's width up to whole bytes. Every
   format of every fixture and write-study image is read node for node as
   the lister prints it, and every width checked to be whole bytes
   (tests/oracle_formats.rs). The unpacker's handling of other widths is
   tested only against this crate's own packing (self-consistency, not
   correctness); no reference image can exercise it.
2. **Pointer device and generation bits** (48..55 and 56..63): the
   generation bits are now checked -- every pointer of every fixture,
   including the aged image's nine pointers of generation 1 into reused
   buckets, carries the generation its bucket's `alloc_v4` key records
   (`tests/oracle_pointers.rs`); the device bits are only ever 0. The
   reader refuses a pointer whose generation is not the bucket's (stale,
   S1 9.1.3.1), one for another device, or one with a flag bit set other
   than unwritten (bits 1..3; cached pointers, never observed). Unwritten
   is SETTLED (#158): flags 0b100, on the extents the reference module
   made for `fallocate` in a `nocow` file, which the lister prints
   `unwritten` and which read as zeros; `fallocate` in another file makes
   `reservation` keys. Btree node pointers are not yet judged the same way.
3. **Btree node flags**: what bit 8 and bit 32 mean. The btree id and
   level are settled (question 13); the reader does not need them (it
   trusts the parent), and the writer copies the other bits from an
   existing node.
4. **Unclean filesystems**: SETTLED for single-device images (see Journal
   above), and the superblock's `journal_seq_blacklist` (type 8) is
   SETTLED too: pairs of u64 `(start, end)` with the end EXCLUSIVE. S1
   (1.3, 9.7.5): a reader skips a bset whose journal sequence is
   blacklisted, and the bset stays on disk until its node is rewritten;
   an unclean shutdown blacklists the next 64 sequences beyond the newest
   journal entry found, so a clean image can hold such bsets. Observed
   (S3, S4): the aged image's field reads 307..4403 after the reference
   replayed entries 304-306; no bset of its nodes has a journal sequence
   inside the range, two have exactly 4403, and the lister reads their
   keys -- so 4403 is live and the end is exclusive. The reader ignores
   bsets in `start <= seq < end` (`Node::parse_filtered`). Still open: the
   journal's own `blacklist` and `blacklist_v2` entries (types 3, 4) --
   none in any fixture's journal; a replay window holding one is refused.
   S1 efec39c's journal entry list (11.2) names them: `blacklist` for a
   single journal sequence number, `blacklist_v2` for a range. Their
   layout is still to be observed, which needs a journal that holds one.
5. **Snapshots and subvolumes** (#12) are answered for reading; see
   "Snapshots and subvolumes" below. No route through the reference FUSE
   mount makes one, and the reference key editor fabricates snapshot keys
   but never a whole subvolume the checker passes (#114), so the
   reference tool makes them in the kernel oracle's VM. Still open: what
   the low half of a snapshot key's first word and of a subvolume key's
   first word hold (2 and 0 seen; the lister prints `live` and, for a
   snapshot of a subvolume, `snapshot`), and where the 128-bit bitmap of
   near ancestors sits. S1 efec39c (9.4.2, 9.4.3) says what a snapshot
   node holds: its parent, up to two children, its depth, and a skiplist
   of three ancestor ids for quick ancestor queries, with a 128-bit bitmap
   of near ancestors; trees are grouped under `snapshot_tree` entries
   naming the root snapshot and the master subvolume. The words holding
   the depth, the skiplist and a tree's two fields are observed (#220):
   see "Snapshots and subvolumes -- read".
6. **Encryption (nonces, ChaCha20/Poly1305), erasure coding, reflink,
   multiple devices and replicas**: not seen in any fixture read by this
   crate. (inline_data, xattrs and crc128: seen and read, see above.)
   **Reflink** (#7) is answered; see "Reflinks" below. No route through
   the reference FUSE mount makes one (FICLONE, FICLONERANGE and
   FIDEDUPERANGE are refused before the daemon sees them;
   `copy_file_range` copies), so the reference kernel module makes it.
   **Encryption**: S1 efec39c (9.1.4) now documents in prose the key
   hierarchy (a scrypt-derived passphrase key unlocking a master key kept
   in the superblock), each extent's nonce (from its version, compression
   type and uncompressed size, with a per-CRC offset) and where the MAC
   sits (the CRC entry; 80 bits for data by default, 128 for metadata). The
   byte layouts are still to be learned by computation over known
   plaintext: #159, broken down into #270 to #277. The crypt field's
   layout is observed (#270, "Encryption: the crypt field" above); the
   scrypt parameters' place in it, the salt and how the master key is
   sealed are #273's, learned from `encrypted-passphrase`. The btree
   nodes' nonce, MAC and encrypted bytes are observed (#271, "Encryption:
   btree nodes" above), and so are an uncompressed data extent's (#272,
   "Encryption: data extents" above); a compressed or partly overwritten
   one's are #275's.
7. **Varint fields beyond `dev`** (#81). Their names and order are
   SETTLED (S3): every field of every inode of every dumped image decodes
   to the value the lister prints as `bi_<name>`
   (`inode::FIELD_NAMES`, `InodeV3Raw::field`,
   tests/oracle_inode_fields.rs). What the option fields mean is SETTLED
   for `data_checksum`, `compression`, `background_compression` and
   `data_replicas` (S3 + S4, #81): the reference tool, run in the kernel
   oracle's VM (S10), set each with `set-file-option` on an empty file,
   which was then written, and once on a directory, which a new file was
   created in. What `get-file-option` printed (`kernel.options.txt`) and
   each inode's fields:
   - each field holds the option's value plus one, 0 when unset:
     `data_checksum` none 1, crc64 3, xxhash 4 (so crc32c 2);
     `compression` and `background_compression` lz4 2, gzip 3, zstd 4 (so
     none 1, the filesystem option's numbering plus one); `data_replicas`
     1 is stored as 2;
   - `fields_set` has one bit per option set on the inode itself, counted
     from `data_checksum` in field order (data_checksum 1, compression 2,
     background_compression 8, data_replicas 16);
   - a file created in a directory with an option inherits it: the field
     is stored (`compression` 4) with no `fields_set` bit, and
     `get-file-option` lists it not;
   - the data follows the option: every extent of the lz4, gzip and zstd
     files, the inherited one included, is compressed with that codec
     (tests/oracle_kernel.rs);
   - setting an option also sets inode flag bit 12 (0x1000), on the
     inherited file too; what that bit is called is open.
   Still open: the other option fields (`promote_target`,
   `foreground_target`, `background_target`, `erasure_code`, `nocow`,
   `project`), which no image sets; and a compression level (`zstd:3`).
   The routes that showed nothing:
   - the FUSE mount refuses every `bcachefs.*` option xattr, and the
     reference tool's `set-file-option` through it, with "Operation not
     supported";
   - an option given to the formatter is not copied into any inode: every
     inode of the lz4, zstd, gzip, crc64, xxhash and bgcompress sets has
     its option fields 0, and of the casefold set too (`casefold: 1` in
     the superblock, `bi_casefold=0` in every inode);
   - mount options are filesystem-wide (S1 7.1), none per inode;
   - the reference tool's offline key editor (`kvdb`, S1 6.12) edits an
     inode only up to its fixed header in the pinned release: `update
     inodes <pos> bi_compression=2` answers "bch_inode_v3 has no field
     'bi_compression'" (write-study/kvdb-options.txt).
   The writer leaves the option fields 0 and writes data as the
   filesystem's options say, whatever an inode carries.
8. **Whiteouts and deleted keys across bsets**: SETTLED for what the aged
   image holds. Nodes of up to 78 bsets merge newest-bset-wins with deleted
   keys dropped, and the result equals the lister's keys exactly. A
   `whiteout` key type has not been seen yet.
9. **Dirent names longer than one key, and the 31-bit dirent offset
   change** (1.30) -- not exercised. (Casefolded dirents: question 18.)
10. **Dirent hash collisions** (#78). SETTLED by observation (S8). SipHash
    slots are 63 bits wide, so the write study collides names under
    `--str_hash=crc32c` instead: four 16-byte names whose XOR differences
    have a CRC of 0 collide under any seed. Created in order through the
    reference mount, they took four consecutive offsets in creation order.
    Removing the second left a `hash_whiteout` (u64s 5, no value) in its
    slot, and the others stayed. The dirents counter kept 7 keys, its bytes
    fell by 72 - 40, so a whiteout is a counted 40-byte key. Created again,
    the name took the whiteout's slot. A removal with nothing after it (the
    write study's unlink) deletes the key outright. So a name is looked up
    from its hash slot up, past other names and whiteouts, to an empty
    slot; the reader and the writer both do this. INFERRED for SipHash
    directories, where no collision can be made: the same rule.
13. **Node flags for btree ids of 16 and more** (#80). SETTLED (S8): a
    node's flags carry the id's low four bits in bits 0..4, the level in
    bits 4..8 and the id's higher bits from bit 9, for every node of every
    btree of every clean fixture and write-study image, ids 16 and above
    included (`btree::flags_id_and_level`, tests/oracle_node_flags.rs).
    The writer builds a new root's flags that way for any id. No write it
    makes yet needs a root for an id of 16 or more, so no such root has
    been judged by the reference checker.
12. **Flags bits 32..35 of an inode** are 3 in every inode of every
    dumped image (S4, tests/oracle_inode_fields.rs; #81); what they mean
    is open, and the writer copies them from the parent, which keeps them
    3. The accounting keys' versions are copied unchanged too; the
    reference checker accepts both.
15. **The xattr slot** (#77). SETTLED, INFERRED and checked (S8). The two
    of `aged`'s xattrs that did not fit (`user.on-a-directory`,
    `user.greeting`) were not about when they were set: their inodes'
    seeds never changed, and the same names misfit again on fresh images.
    One xattr of every name length from 1 to 24 showed the rule: the
    message is the namespace byte then the name, and SipHash-2-4 keyed
    `(hash_seed, 0)` over it, shifted right by one, gives the slot, except
    that a message longer than 8 bytes and not a whole number of words has
    a final partial word of a zero byte then all but the message's last
    byte. Names of 1 to 7, 15 and 23 bytes therefore fit the plain hash;
    the rest do not. `xattr::name_slot`, checked on every xattr of the
    write study's xattr images and all 45 of `aged`
    (tests/oracle_xattr_slots.rs).
16. **Superblock copies.** SETTLED. S1 (9.5.1): the copy with the highest
    valid `seq` is authoritative, and the standalone layout at sector 7 is
    consulted when the primary cannot be read. Observed (S4): every fixture
    carries three copies, at sectors 8 and 2056 and at the end of the
    device, each recording its own offset, all with one `seq`. The reader
    reads every copy and takes the highest `seq`, falling back to the
    layout at 3584 when the primary does not read; the checker reports a
    copy that does not read (`tests/oracle_superblock_copies.rs`).
17. **The string hash.** S1 (7.7): the `str_hash` option is one of crc32c,
    crc64 or siphash (the default), for dirents and xattrs alike; the
    inode's flags bits 20..23 carry the hash type. SipHash is 3 (S3, S4:
    every inode of every fixture, and the lister's `hash_type=siphash`).
    The reader scans a directory of any other type instead of hashing
    (`Filesystem::find`), the writer refuses to place a name in one
    (`siphash_only`), and the `strhash` fixture (`--str_hash=crc32c`) is
    read whole by scanning (`tests/oracle_strhash.rs`, which prints the
    number crc32c carries). crc32c is hash type 0, and its dirent offset is
    CRC-32C (initial value all ones, no final XOR) over the directory's
    `hash_seed` as eight little-endian bytes followed by the name
    (INFERRED by computing candidates against the write study's crc32c
    images, then checked against all 314 dirents of `strhash`:
    `inode::name_hash`, tests/oracle_collisions.rs). The xattr hash under
    crc32c is SETTLED (S8, #106): the same CRC over the seed, then the
    namespace byte and the name, with none of SipHash's rearranged final
    word; every xattr of the write study's xcollide images sits there, and
    four colliding names take that slot and the three after it, in the
    order set (`xattr::slot`, tests/oracle_xattr_slots.rs). Still open:
    crc64's number and inputs.
18. **Casefolded directories.** SETTLED for what the reader needs (see
    Inodes and dirents above). The reference mount refuses `chattr +F` and
    `set-file-option --casefold` on a directory (`probe.txt`), but the
    formatter takes `--casefold` for the whole filesystem, and its mount
    then folds lookups. Still open: the folding of names beyond ASCII
    (Unicode's tables, "utf8-12.1.0" by the superblock printer's own
    words), which a lookup of such a name would need to hash, so the
    reader scans for it as stored (issue #111); whether
    `has_case_insensitive` alone marks a directory, or only with the
    superblock option, since no image has one without the other; and what
    bytes 9..10 of a casefolded dirent hold, 0 in every one seen and
    refused otherwise.
19. **Extent entries beyond ptr, crc32 and crc64.** S1 (9.1.3, 9.1.12)
    names crc128 (24 bytes, required under encryption), stripe pointers
    (erasure coding), a flags entry (poisoned) and a `reconcile` entry
    recording IO options, e.g. `[crc32, ptr, ptr, reconcile]`.
    **Reconcile is answered by the `bgcompress` fixture**
    (`--background_compression=lz4`, S3, S4): its listing shows
    `reconcile: need_rb=background_compression replicas=1 checksum=crc32c
    background_compression=lz4` after the pointer of 302 of its 440
    extents; those keys are 8 u64s against 7 for the same `[crc32, ptr]`
    without it, and the extra word in the image is
    `0x0000_0010_9010_0080`, whose first set bit is 7. The reader passes
    over that one word and reads the data; the files read back to the
    mount's SHA-256 (`tests/oracle_bgcompress.rs`). Still open: the
    word's fields, and whether it can be longer with other options set.
    **crc128 (kind 3) and flags (kind 6, poisoned at bit 7) are answered**
    by the `crc128` fixture and the `crc128-overwrite` and `poison` images
    (see Extents above); a crc128 nonce, only ever 0, is refused unless 0.
    **The stripe pointer is kind 4**, one word (S8, S3, S4: the fixture
    build's erasure-coding probe, three devices, `--erasure_code
    --replicas=2`, `probe-ec.txt`). The reference first writes each extent
    with two pointers, then rewrites it as `[crc32, stripe_ptr, ptr]` keys
    of 8 u64s once the stripe is made; in that copy `stripe_ptr: idx 1
    block 0` is the word 0x22010, `idx 1 block 1` 0x22030 and `idx 2 block
    0` 0x42010, through `idx 4`. So bits 5..12 are the block and bits
    17..63 the stripe index; bits 13..16, which the lister does not
    print, hold 1 in every one (S1 names a 4-bit redundancy). This reader
    reads one device, so a stripe pointer is refused by name. Kind 5 is
    the one kind no image has shown.

20. **gzip-compressed extents this writer makes** (#105). The writer
    stores data on an lz4 or zstd filesystem compressed, and the reference
    mount reads it back byte for byte (CI run 38013718306). On a gzip
    filesystem it wrote each piece as a raw deflate stream
    (`miniz_oxide`, level 6), the framing this crate decodes from the gzip
    fixture; the reference checker passed the image, and this crate reads
    it back, but the reference mount's daemon died reading it
    (`bcachefs: fatal SIGSEGV` in `fuse_read`, same run). Whether the
    stream differs from what the reference writes, or its userspace gzip
    path fails on any stream, was open. **Answered by the reference
    kernel module (#141):** the module stores gzip as a raw deflate
    stream too (the kernel image's `opts/compression-gzip`, first bytes
    `ed ca c1 09`), and a gzip file this writer made, on a filesystem the
    module had mounted, reads back through the module byte for byte and
    passes the reference checker (`kernel-written.img`). Data on a gzip
    filesystem is written gzip-compressed; where the reference mount's
    daemon still cannot read it, the module's read is the judge.
21. **How `bi_dev` holds a device node's numbers** (#251). It is a varint
    field after `bi_generation` (S3), but no image has held a character or
    block device, so how it splits into a major and a minor number has not
    been seen. `fs.bcachefs stat` reports both as null and the stored value
    as `bcachefs.dev`. A fixture with device nodes the reference mount made,
    compared with the mount's `st_rdev`, would answer it.

### Reflinks -- read

The reference kernel module (S10, `scripts/kernel-oracle.sh`) clones a
3 MiB file with FICLONE; the reference lister's view of the extents and
reflink btrees is kept as `kernel.extents.txt` and `kernel.reflink.txt`.

- Both files' extents become `reflink_p` keys (type 15, 7 u64s: a 16-byte
  value), each covering a range of the file as an extent does. The lister
  prints `idx N front_pad 0 back_pad 0 may_update_opts`: the first word's
  low 56 bits are `idx` (S1 9.1.6 says 56 bits) and bit 57 is
  `may_update_opts`; the second word is 0 with both pads 0, so where each
  pad sits in it is still open.
- The shared data is in the reflink btree (id 7) as `reflink_v` keys
  (type 16) at `0:end`, `size` sectors long, like an extent: a u64
  refcount (2 for a file and one clone), then an extent's entries
  (`crc32`, then `ptr`).
- A `reflink_p` at file sectors `start..end` reads the reflink btree's
  sectors `idx..idx + (end - start)`.
- Judged: every file the kernel wrote, the clone and its source included,
  reads back here at the SHA-256 its mount reported, and every pointer's
  index and flag match the lister (`tests/oracle_kernel.rs`). The checker
  does not yet follow a reflink, and the writer will not free one (it
  would have to drop the shared extent's refcount).

### Snapshots and subvolumes -- read

The recursive CLI (#260) uses these existing observations to enter each
subvolume at its root and snapshot; directory identity includes both fields.
Its guest regression compares the paths, types and inode numbers with GNU
find through the reference FUSE mount of the reference-made `kernel` tree,
and root/snapshot IDs with the reference lister. Damaged copies exercise
revisited and unreadable directories. These are black-box checks scheduled
for CI; no new format layout is inferred and no reference source is read.

The reference tool, run in the kernel oracle's VM (S10,
`scripts/kernel-oracle.sh`), makes a subvolume `sv`, fills it, snapshots
it as `snap`, then changes, removes and adds a file in `sv`. The lister's
view of the subvolumes, snapshots, snapshot_trees, inodes, dirents and
extents btrees is kept as `kernel.<btree>.txt`.

- Subvolumes (btree 8, key type 21) at `0:id`: the first word's high half
  is the subvolume's snapshot and the second word its root inode (`sv`: 2,
  snapshot 4294967292, root 2305843009213693952; `snap`: 3, snapshot
  4294967293, the same root inode). The third word holds
  `creation_parent` in its low half and `fs_parent` in its high half.
- Snapshots (btree 9, key type 22) at `0:id`: the first word's high half
  is the parent (0 for a tree's root), the second word the two children,
  the third `subvol` in its low half and `tree` in its high half, the
  fourth the depth in its low half and the first skiplist entry in its
  high half, the fifth the other two entries (`sv` and `snap`: `depth 1
  skiplist 4294967294 4294967294 4294967294`; each tree's root: `depth 0
  skiplist 0 0 0`). Taking
  the snapshot made 4294967294 the interior parent of the two leaves
  4294967292 (`sv`) and 4294967293 (`snap`); the keys written before it
  are at 4294967294.
- Snapshot trees (btree 15, key type 31) at `0:id`: the value's one word
  holds the master subvolume in its low half and the root snapshot in its
  high half (tree 2: `subvol 2 root snapshot 4294967294`).
- A dirent of type 16 (`subvol`) names a subvolume: the low half of its
  first word is the subvolume and the high half its parent (the lister's
  `sv -> 1 -> 2`).
- Visibility: a snapshot sees keys at its own id and its ancestors'; at
  one position the nearest wins. `new.txt`'s dirent and the changed
  file's new inode and inline data are at `sv`'s 4294967292, beside the
  old ones at 4294967294; the removed `gone.txt` is a
  `whiteout` (key type 1) there over its dirent at 4294967294, which
  `snap` still sees. A file keeps its inode number in every snapshot.
- Judged: every file, directory and symlink in the root subvolume, in
  `sv` as it is now and in `snap` as it was reads back at the inode
  number, mode and SHA-256 the kernel's mount reported
  (`tests/oracle_kernel.rs`). The writer and the checker still read one
  snapshot, the root subvolume's.
- Links checked (#220): a snapshot tree's master subvolume is in the
  subvolumes btree; a snapshot's tree is in the snapshot_trees btree and
  its root is the root the snapshot's parents lead to; a skiplist entry is
  0, the snapshot, or one of its ancestors. A parent cycle, or a parent the
  snapshots btree does not have, leads to no root, and the walk is
  bounded. The rules come from S1's prose (9.4.2: snapshots form trees,
  each with a `snapshot_tree` entry for its root and its master
  subvolume; 9.4.3 and 9.4.4: a skiplist of ancestors, at or above the
  snapshot; the recovery pass list: a tree's master subvolume is a real
  subvolume) and from the reference checker's findings on damaged images
  (S3): "snapshot tree points to missing subvolume", "snapshot with bad
  skiplist pointer" and "snapshot points to missing/incorrect tree".
  `tests/check_oracle.rs` has the reference judge each damaged image the
  checker is held to, a parent cycle included (CI run 38699997306).

- Snapshot key positions (S3, #219/#220): the retained report from CI run
  38749903964, job 116473259222, at `cb33c4ea03cbe0c7a6ff720f8a3e7aa2983f079e`
  records `default.img` seed 51. Its snapshot key at
  `0:4294967295:4294967294` keeps the root snapshot's value unchanged;
  the reference rejects its nonzero snapshot field with
  `bkey_snapshot_nonzero`, and this checker reports it under the same
  code from its key-field checks (#226). The snapshot ID belongs in the
  offset; the key's snapshot field must be zero.
  `oracle_check_snapshots.rs` checks the unchanged value and the stored
  mutation; `check_oracle.rs` requires both a clean native control and
  the native nonzero-snapshot finding. Current-head CI is still required;
  no differential baseline entry is added or broadened.

### First differential checker baseline (S3, #164)

CI run [38579070302](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38579070302)
at PR #217 head `034ded6` compared 648 cases with reference checker 1.39.7.
The `differential-report` artifact's `report.md` has SHA-256
`9789454d87c7eb69f274b70eefe54a78ab09ce47a73f08da95d45a39d10a8e0d`.
All eight unchanged controls were clean to both checkers; this crate had no
panics and no findings missed by the reference. The reference found corruption
in 97 cases this checker called clean, grouped into 46 exact classes.

The allocation and allowed-key-type gaps (15 classes) are tracked by #163;
metadata key lengths, snapshots, positions and values (23 classes) by #219;
snapshot links and skiplist pointers (three classes) by #220; inode directory
size, journal sequence and sector counts (four classes) by #221; and a torn
node with fewer written sectors than its parent requires (one class) by #222.
These facts come from the tools' printed diagnostics on non-crashing cases,
not from their implementation source. They establish missing validation, not
general rules for unobserved key types.

`fuzz/differential-baseline.tsv` records only these measured direction,
mutation-structure and folded-diagnostic triples, each naming its open issue.
They remain known defects; a fixing PR removes an entry after the same
oracle comparison confirms its class is gone. New triples, invalid controls
and crate panics still fail the differential run. The two reference crashes
or faults were counted only, with their identities and data excluded.

Removed since, each by the pull request that fixed it after a run at seed 0
no longer found it: #220's three snapshot-link classes (#224).

### Rebuilt-fixture differential control (S3, #227)

Main control [38698985871](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38698985871)
at `91a47a206cb4ada85cb322775af438a60b03eb6e` used the fixtures rebuilt by
main run `38680345154`, reference checker 1.39.7, seed 0 and 80 cases per
fixture. Its `differential-report/report.md` has SHA-256
`72011e7f6567b072cc8dcf3bc97e7a0a3dd2560da38b9b306c8e312722db5876`.
The 648 cases comprise eight unchanged controls and 640 mutations: 445 both
clean, 93 both finding something, 104 reference-only, one crate-only and
five reference crashes, faults or hangs (counts only). No crate panic occurred.
All unchanged controls were clean to both after the report's normal
fixture-specific recovery messages were accounted for.

Rebuilt images have different node layouts, so the same node-mutation seeds
can expose different checks. This control adds eight exact baseline classes:

| observed disagreement | example | open work |
|---|---|---|
| accounting key in the extents tree | default seed 50 | #163, allowed key types |
| allocation-key diagnostic, with the allocation-pass prefix | block4k seeds 3 and 45 | #163, allocation validation |
| allocation-key diagnostic, without that prefix | lz4 seed 48 | #163, allocation validation |
| accounting mismatch at an unknown tree id | aged seed 45 | #163, accounting consistency |
| accounting key with trailing junk, inode-count kind | kernel seed 64 | #219, metadata key invariants |
| extent past the inode's reported size | kernel-written seed 19 | #221, inode metadata consistency |
| accounting key with trailing junk, replica kind | kernel-written seed 68 | #219, metadata key invariants |
| missing backpointer here, clean to the reference | default seed 55 | #230, unresolved disagreement |

The last case re-stamps a 32,768-byte node at byte 2,752,512. This checker
reports an absent backpointer for extent `2147483894:1:U32_MAX` at device
sector 15124, offset 0; the reference exits 0 without a finding. This does
not establish which checker is right. Issue #230 requires the changed-byte
and reference-lister evidence before changing the production backpointer
check. Its exact `ours-only / node / check:missing_backpointer` baseline
entry tracks that unresolved question; it does not declare the image valid.

These entries acknowledge measured disagreements, not completed driver
fixes. No wildcard, old baseline entry, checker assertion or oracle check
is removed. New classes, failed controls and crate panics still fail.

PR run [38718924048](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38718924048)
at `3be1c2a9bbb50465182ad10dd8a8158eed6b89f3` (differential job 116368360626,
report SHA-256
`8921a81d0a0d720476af6e2c31f4c2c3f06c13309b36dac66eb9f2e23e25cacb`) shows
the fixture source is not fixed: the job fetches the fixtures of the newest
green main run, so the same seeds reached different nodes. Of its 648 cases,
454 were both clean, 90 both found something, 98 reference-only, none
crate-only and six reference crashes, faults or hangs. None of the eight
classes above occurred, and three further exact classes did:

| observed disagreement | example | open work |
|---|---|---|
| accounting key with nonzero size, device-data kind | kernel seed 37 | #219, metadata key invariants |
| backpointer with nonzero snapshot, node re-stamped | kernel seed 53 | #219, metadata key invariants |
| dirents accounting mismatch after a dirent offset change | kernel-rootsnap seed 0 | #163, accounting consistency |

The third case also shows the reference reporting a duplicate dirent hash key
at a wrong offset, which this crate called clean. These entries are tracked
gaps, not fixes.

The fixture source is recorded, not inferred: the job's own fetch log reads
`fixtures from run 38714446443: 55 images`. That is main CI run
[38714446443](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38714446443)
at `b23f1a198d3a7c9855f52f48ad84a88b85f2b7fc`, fixtures artifact 11963530352,
digest `sha256:ea2151e74da19320ae6de32f1a1d332847cf44bb0bf7643af68c4e151aa7ec3c`.
The baseline lines above were measured on fixtures 38680345154.

Every report now names its fixtures (run, artifact, artifact digest and the
tarball's SHA-256), and a dispatch can judge a named run's fixtures, refusing
one whose artifact digest differs. The three classes above reproduce with:

    gh workflow run differential.yml -R antimatter-studios/rust-fs-bcachefs \
      -f cases=80 -f first_seed=0 -f fixture_run=38714446443 \
      -f fixture_digest=sha256:ea2151e74da19320ae6de32f1a1d332847cf44bb0bf7643af68c4e151aa7ec3c

Ordinary runs keep judging the newest green main run's fixtures, so new
findings still surface. The artifact expires 2026-10-22T10:01:45Z (seven-day
retention).

A replay dispatch (`gh workflow run differential.yml -f replay=true`) takes
the fixture run, digest, case count and seed from
`fuzz/differential-replay.tsv`, which names the three cases (`kernel-37`,
`kernel-53`, `kernel-rootsnap-0`) and each one's exact class. The judge keeps
those cases only (`--keep`; no case is kept by default): each case's mutation
patch, its `cases.tsv` row with this crate's verdict, and the reference's
whole output. It keeps them only when the reference judged the case without
crashing or hanging, so no crash or hang is kept, even as an artifact. The
replay step then fails unless every named case was kept and lands in exactly
its class. The kept cases are uploaded as the `differential-replay` artifact
for 30 days. The fixture image plus the patch rebuilds the mutated image, so
whole images are not stored.

Removed since, each by the pull request that fixed it after a run at seed 0
no longer found it: #221's four inode-metadata classes (#225), and
#219's sixteen size and snapshot classes (#226).

### Inode metadata against the rest (#221)

- A directory's size is 0: the lister prints `bi_size=0` for every
  directory of every listed fixture (S3), and the reference reports
  "directory with nonzero i_size" for one made 2^64 - 1.
- An inode's sector count is the sum of the sizes of its extents btree
  keys of type extent, reservation, reflink_p and inline_data, at its own
  snapshot. Measured (S3) over every inode the lister prints, on default,
  lz4, zstd, gzip, nocsum, xxhash, crc64, block4k, aged, strhash,
  bgcompress, crc128, crc128-overwrite, casefold, kernel, kernel-prealloc
  and probe: no exception, compressed extents counting their key's size.
  Where an inode's extents have keys at other snapshots too, which of them
  a version of the inode counts depends on what its snapshot sees (S1
  9.4), and the checker does not judge it. The reference reports
  "incorrect i_sectors: got N, should be M".
- An inode's journal sequence is the journal's when it was last written
  (S1 6.12 and 8.3 name the field `bi_journal_seq`, and "a
  bi_journal_seq in the future" as a state the reference corrects).
  Every fixture's inodes are at or below the sequence it was shut down at
  (S3). The reference compares with the sequence its own journal is at
  while it checks ("inode journal seq in future (currently at 45)" on lz4,
  shut down at 43): it skips 64 past the last entry read after an unclean
  shutdown (S1 1.3), and its run advanced at most 6 on any fixture, from
  "recovering from clean shutdown, journal seq N" to "clean shutdown
  complete". The checker reports a sequence more than 128 past the newest
  written, past anywhere the reference's journal can be while it checks;
  nearer ones are not judged.

### Dirent hash reachability (#221, #225)

Completed differential run `38759380443` at `c5d1c60`, reference checker
1.39.7, seed 0 and 80 cases per fixture, found the reference-only class
"hash table key at wrong offset" on kernel seed 45. The same branch's
completed CI run `38759379043` failed the five-slot displacement regression;
its hash-whiteout control passed.

The collision observation in question 10 supplies the lookup rule: a
dirent must be reachable from its name's hash slot through consecutive
dirents or hash whiteouts. The checker uses the stored folded name for a
casefolded entry, and leaves an unknown hash type unjudged, matching the
reader's existing policy. `tests/check_oracle.rs` additionally requires the
reference checker itself to reject the five-slot displacement and accept
the one-slot displacement past a hash whiteout. These guest assertions are
retained as independent controls; their post-fix CI result is still pending.

## Retained default seed 74: checksum bytes and independent read

Completed red run 38749903964/job/116473259222 and exact replay
38840695392/job/116779857712 report an extent-data disagreement. Retained
artifact 12020594852 has SHA-256
`b43a64bb47570793022dc04e0bd7231b75e3bc8b351e1c3719302a189fda1c34`;
its patch SHA-256 is
`ee572579a8b995cbe6a8950a759560baec5297638f12dcc95f0981ab84c68c67`.
Applied to fixtures artifact 11977252185 from run 38741040375 (archive
`9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e`),
only six image bytes differ: 3041280–3041283 (the re-stamped bset checksum)
and 3041646–3041647 (the extent checksum's upper 16 bits). No file data changed.
At byte 3041640, the little-endian crc32 entry changes from
`0x0a2f1ba705000002` to `0x00011ba705000002`. Tag, one-sector sizes,
zero offset, checksum type 5 (CRC32C), and no compression remain unchanged.
CRC32C has 32 stored bits here: matching low 16 bits does not validate it.

Existing black-box native artifact 12011667570 from run 38818320145 has
SHA-256 `ae6ea81e1866f4be19d6ad40c9b1753004db949e12c1948e49fff9a54ea326b0`.
Its identical patch and reference extent listings corroborate that decoding.
Reference fsck accepts both images, but its FUSE reader reads the unchanged
inode 2147483904 at `many/file-0242.txt` with the expected content digest,
and returns EIO for that same inode on the mutated image. Thus fsck exit 0
is not a data-integrity verdict. Correlation requires that archive, complete
physical control/mutated image digests, patch digest, reference version and
every case field; absent or different evidence preserves the disagreement.
CI regrades the retained native observations with positive and negative
controls, and independently reapplies the checked-in exact-byte patch. No
GPL source was consulted and no writer or checksum validation was changed.

The same physical disagreement recurred on #225 at `4865d20`, completed
run `38855619610` / job `116826294741`. Run `38857827089` on #224
verified this binding and its rejection controls against the identical
fixture archive. #225 retains its own fixture selector and disagreement
baseline; fresh CI on the integrated head is required.

### A key's own size and snapshot (S3, #219)

Run 38579070302's report shows the reference checker refusing a key after only
its size, or only its snapshot, was changed and the key committed through
the writer. It printed "size != 0, deleting" (counted as
`bkey_size_nonzero`) for a size on an inode_v3 key in inodes, a dirent in
dirents, a subvolume, an lru `set` key, a backpointer, a bucket_gens key, a
snapshot_tree, the logged_ops inode_alloc_cursor, and accounting keys of the
`btree` and `inum` kinds. It printed "nonzero snapshot, deleting" (counted as
`bkey_snapshot_nonzero`) for a snapshot on a subvolume, a snapshot key, and
the `set` keys of lru, need_discard and freespace. A freespace key in the
same report carries a size of 1894 sectors and is refused only for its
snapshot, so neither rule belongs to a key type or a btree alone: the
checker judges exactly the observed pairs of btree and key type, and every
other pair is an open question until an oracle is seen to answer it.

Nightly run 39756419740 (seeds from 1659840) added the logged_ops
inode_alloc_cursor to both rules' neighbours (#376). The cursor moved to
snapshot 4294967294 on `default.img` and `block4k.img` (seed 1659898) was
refused as "nonzero snapshot" (`bkey_snapshot_nonzero`). The cursor moved off
inode 1, to U64_MAX on `aged.img` and `kernel.img` (seed 1659851) and to 0 on
`kernel-rootsnap.img` (seed 1659879), was refused as "k.p.inode bad"
(`inode_alloc_cursor_inode_bad`). Every fixture's cursor is at inode 1, so
the checker reports a cursor at any other inode; its offset varies between
fixtures and is not judged.

Run 39018152705's aged-fixture mutation at seed 1659445 changed only the key
type at `0:2040:0` in the freespace btree, from `set` to
`logged_op_truncate`. The reference checker reported `invalid key type for
btree freespace (logged_op_truncate)` and deleted it; this checker had
reported the image clean (#257). The guest regression repeats that exact
mutation on the pinned `aged.img` and requires both checkers to report the
type refusal. It is the first measured key type for freespace beyond its
ordinary `set` keys; other types there remain open.

The same differential report's aged-fixture mutation at seed 1659510 changed
the key type at `2147487679:1366636819568056465:U32_MAX` in the xattrs btree,
from `xattr` to `lru`. The reference checker reported `invalid key type for
btree xattrs (lru)`; this checker had reported the image clean (#259). The
guest regression repeats that exact mutation on the pinned `aged.img` and
requires both checkers to report the type refusal. It is the first measured
key type refusal for xattrs; no other pair is inferred.

The kernel-fixture mutation at seed 1659510 changes the key at `0:266:0` in
the freespace btree from `set` to `lru`. The reference checker reports
`invalid key type for btree freespace (lru)`; this checker reports
`invalid_bkey` for the same key. The guest regression plants that exact
mutation in the pinned `kernel.img` and requires both verdicts. This
establishes only that `lru` is refused in freespace; no other type is inferred.

The kernel-fixture mutation at seed 1659466 changes the key at `1:4:0` in
`logged_ops` to `inode_v3` and commits it. The reference checker reports
`invalid key type for btree logged_ops (inode_v3)` for its 16-byte value;
this checker now reports `invalid_bkey` for that exact pair. The guest
regression repeats the mutation against both checkers. This establishes only
that `inode_v3` is refused in `logged_ops`; no other pair is inferred.

### A key's position and its value's lengths (S3, #219)

The same report shows four more refusals, each after one change:

- A subvolume moved to offset 0 (`POS_MIN`), or to inode U64_MAX, was
  refused with "invalid pos" (`subvol_pos_bad`). Every subvolume the
  fixtures hold is at `0:id` with an id of 1 or more, so the checker
  refuses a subvolume at offset 0 or at any inode other than 0. The inode
  rule is the one step past what was observed (only U64_MAX was tried). A
  differential run that tried another inode and saw the reference accept
  it would show this as a class only this crate finds.
- A snapshot_tree moved to offset 0 was refused with "bad pos"
  (`snapshot_tree_pos_bad`).
- The lister prints an accounting key whose inode is `6 << 56 | id << 48`
  as `btree btree=NAME`, with the btree of that id: logged_ops (17) and
  subvolume_children (19), with offset and snapshot 0. One such key moved
  to snapshot 1, and one whose inode also had `0xff0000` set, were refused
  with "junk at end of accounting key" (`accounting_key_junk_at_end`).
  Other kinds of accounting key were not seen changed this way and are
  not judged.
- An xattr whose value's first word was set to all ones was refused with
  "value too small (5 < 8225)" (`xattr_val_size_too_small`): a name of
  255 bytes and a value of 65535, after the 4-byte header, need 8225 words
  where the value has 5. The checker refuses an xattr whose header's two
  lengths, with the header, are longer than its value (the layout in
  `src/xattr.rs`).
- Differential run 38753768243 changed a dirent of `kernel-rootsnap.img`
  (seed 10) into an xattr; the reference deleted it with "invalid key type
  for btree dirents (xattr)" (`bkey_invalid_type_for_btree`). The checker
  refuses that one pair, an xattr in the dirents btree; other pairs of key
  type and btree were not seen and are not judged.

The inode node case of the same report (a re-stamped node, `default.img`
seed 44) is still open: the report cuts off the reference's reason.

### Snapshot inode positions (S3, #253)

The current-main nightly-equivalent [run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705)
at `08c02dc10add07ffce512a8787869c9a39abcd3c` judged 648 cases with reference
checker 1.39.7, 80 mutations per fixture starting at seed 1659440. Its
fixtures came from run 38741040375, artifact 11977252185, digest
`sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e`.

Three cases moved a snapshot key to inode U64_MAX while leaving its value,
offset and snapshot unchanged. The report's default and block4k examples
(seed 1659458) move the root snapshot to `U64_MAX:4294967295:0`; the
reference reports `snapshot_pos_bad` ("bad pos, deleting") and this crate
reported clean. Run 38878947701 had already observed the same refusal at
inode 1 (`default` seed 1659387). The checker now reports that problem for
snapshot keys in the snapshots btree with a nonzero inode position. No
rule for another key type, or for a snapshot's offset, is inferred here.

`oracle_check_key_positions.rs` repeats the U64_MAX mutation on default and
block4k; `check_oracle.rs` independently judges inode 1 and U64_MAX against
the reference checker in the guest, with unchanged controls and a check
that the stored snapshot value is unchanged. The unit regression keeps
inode 0 accepted and confines this rule to the observed btree/key-type pair.

### Snapshot-tree inode positions (S3, #254)

The same [run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705)
and pinned fixtures recorded above for #253 contain three snapshot-tree
inode disagreements. The report's default and block4k examples (seed
1659461) move snapshot tree 1 from `0:1:0` to `1:1:0`, keeping its value,
offset and snapshot unchanged. The reference checker 1.39.7 exits 9 and
reports `snapshot_tree_pos_bad` ("bad pos, deleting"); this crate reported
clean. This is black-box output only; no reference source was read.

The checker now reports that problem for a snapshot-tree key in the
snapshot_trees btree whose inode is nonzero, alongside its existing
offset-zero refusal. `oracle_check_key_positions.rs` repeats the reported
inode-1 mutation on default and block4k. `check_oracle.rs` requires the
reference checker to reject inode 1 and U64_MAX on fresh fixtures, after
both checkers accept unchanged controls; it verifies that the stored key
differs only in its inode. The unit regression covers inode 0, 1 and
U64_MAX, preserves the offset-zero refusal, and confines the rule to this
btree/key-type pair. These new regressions run in CI, not locally. No
snapshot-field rule or change to snapshot-tree link checks is inferred.

### Snapshot-tree offsets past U32_MAX (S3, #219)

[Nightly run 38878947701](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38878947701)
(reference checker 1.39.7) moved snapshot tree 1 of `kernel.img` (seed
1659362) to `0:U64_MAX:0`, keeping its value ("subvol 1 root snapshot
4294967295"). The reference exits 9 and reports `snapshot_tree_pos_bad`
("bad pos, deleting"); this crate reported clean. This is black-box output
only; no reference source was read.

A snapshot names its tree in a 32-bit field, and the checker already reads
a tree's id from its offset as 32 bits, so a tree whose offset is past
U32_MAX names no tree a snapshot can hold. The checker refuses a
snapshot_tree at such an offset. Only U64_MAX was observed: the bound is
the one step past what was seen, so `check_oracle.rs` also asks the
reference to refuse offset U32_MAX + 1 as well as U64_MAX, after both
checkers accept the unchanged control and the stored key differs only in
its offset. A reference that accepted U32_MAX + 1 would fail that test
rather than leave the bound assumed. `oracle_check_key_positions.rs`
repeats the observed U64_MAX mutation on `kernel.img`; the unit regression
keeps offsets 1 and U32_MAX accepted and confines the rule to this
btree/key-type pair.

### The key types a btree holds (S3, #163)

`tests/check_oracle_key_types.rs` plants a key of every type into a copy
of `default` and reads the reference checker's verdict on each. Its runs
up to 39020187551 planted eight keys to an image, and the reference named
only some of the keys it refused: a dirent planted alone into
reconcile_work_phys is refused with "invalid key type for btree
reconcile_work_phys (dirent)" (`check_oracle_reconcile_dirent.rs`), and
drew no verdict among seven others. The matrix now plants one key to an
image. What those runs show regardless:

- The reference judges a key's type before its value size: an
  inode_generation key with no value is refused for its type in
  reconcile_work_phys and other btrees, and with "bad val size (0 < 8)" in
  inodes.
- A cookie with no value is refused only for its value size, in every
  btree, so its type passes everywhere. A whiteout and an error key are
  refused only for their snapshot ("snapshot == 0", damage) or size
  ("size == 0", extents, reflink, freespace), never for their type.

The checker lets every btree hold deleted, whiteout, error and cookie
keys, and judges the types of three btrees beyond them: logged_ops holds
the logged operations and the inode allocation cursor, and accounting the
accounting key. All three judged btrees also permit `set` and unnamed
type 40 in the observed single-key cases below. Which types every other
btree holds is open (#305).

Completed merge-queue run
[39037096192](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39037096192),
at group head `1540ad069774d34b5a02be0a400545b6fb4a5fd4` containing PR
#218 head `5b152e5`, judged one planted key per image in logged_ops,
accounting and reconcile_work_phys. Its guest matrix reported zero
reference crashes or hangs, zero unresolved type verdicts and zero failed
plantings. The recorded refusal sets exclude `set` (25) and unnamed type
40 in all three trees, while this checker rejected those two types in
logged_ops and accounting, and type 40 in reconcile_work_phys. The first
two trees' retained `set` controls print no invalid key and complete clean
shutdown. This establishes the type verdict for those cases only; no
meaning or value layout is inferred for type 40, nor any rule for other
unnamed types. The matrix retains its probe one past the documented names.

The same run's exact-dirent case prints a type refusal naming
`btree=reconcile_work_phys`. This checker already found that planted dirent,
but its diagnostic used `btree24`, which the test could not recognize.
Tree 24 is now named using S1's existing documented order, also recorded
in the planting helper. These observations use only the reference tools'
printed output, never their source.

### Inode key inode positions (S3, #255)

The same [run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705)
and pinned fixtures recorded above for #253 contain five inode-position
disagreements. The report's default and block4k examples (seed 1659490)
move an inode_v3 key to `U64_MAX:2147483849:U32_MAX` in the inodes btree,
keeping its value, offset and snapshot unchanged. The reference checker
1.39.7 exits 9 and reports `inode_pos_inode_nonzero` ("nonzero k.p.inode,
deleting"); this crate reported clean. Only black-box output was read.

The checker now reports that problem for an inode_v3 key in the inodes
btree whose position's inode field is nonzero. The inode number remains in
the offset field. The unit regression uses the reported offset and
snapshot, accepts inode 0, rejects inode 1 and U64_MAX, and confines the
rule to this btree/key-type pair. `oracle_check_key_positions.rs` repeats
the inode-only U64_MAX mutation on default and block4k; it selects a
regular file by path because newly generated fixtures can renumber inodes.
`check_oracle.rs` requires the independent reference checker to reject
inode 1 and U64_MAX after both checkers accept unchanged controls, and
verifies that all other stored key fields remain unchanged. These new
regressions run in CI, not locally. No position rule for other inode key
versions or other btrees is inferred, and no GPL source was read.

### Xattr key sizes (S3, #258)

Nightly [run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705),
using the digest-pinned fixtures recorded above for #253, changed only the
size of an xattr key in `aged.img` at seed 1659486. The key is in btree 3
at `2147487679:2679498547732338290:U32_MAX`, with attribute `user.attr29`;
its size becomes 1. The reference checker 1.39.7 exits 9, prints
"size != 0, deleting" and counts `bkey_size_nonzero`; this crate reported
clean. This is black-box output, without reading reference source.

The checker now includes the observed xattrs-btree/xattr-key pair in its
existing zero-size rule. No size rule for other key types or btrees is
inferred. The test-only commit precedes the fix: the unit regression uses
the reported position, accepts size 0, rejects 1 and U32_MAX and checks
the rule's scope. The fixture and live independent-reference regressions
select `user.attr29` by name so regenerated inode numbers and hashes can
change. The latter requires clean controls, rejects sizes 1 and U32_MAX
and verifies that every other stored key field stays unchanged.

The differential workflow's digest-pinned job replays the exact seed,
checks the retained size-only mutation and requires `bkey_size_nonzero`.
Only #258's exact differential allowance is removed. The regressions and
the full 80-case-per-fixture replay from first seed 1659440 run in CI, not
locally; post-fix reference agreement remains for CI. No GPL source was read.

### Backpointer values against their extents (S3, S4, S8, #256)

The same nightly [run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705)
and fixtures recorded for #253 report two value-only disagreements:
`block4k` seed 1659443 at backpointer `0:1116733440:0`, and `aged` seed
1659460 at `0:3114205184:0`. The reference checker 1.39.7 exits 9 with
"backpointer doesn't match extent it points to" and
`backpointer_to_missing_ptr`; this crate reported clean. Report artifact
12096026183 has digest
`sha256:ad43d71ebff7928f2e92b16634da0880523ba8f865af3b30dfed32818b1b2670`.

The mutations replace the value's second little-endian word by 1 and 0,
respectively. That word carries the length and target snapshot: the
block4k value changes from length 8 and snapshot U32_MAX to length 1 and
snapshot 0; aged changes from length 22 and snapshot U32_MAX to zeros.
The referenced extents remain `2147483766:8:U32_MAX` and
`2147487978:22:U32_MAX`. This is black-box checker output and the already
observed value layout above, without reference-source access.

The checker now compares the documented btree, level, data type, physical
length and target position at each already-supported data backpointer
location. Whole compressed regions use their compressed length;
uncompressed pieces use their live length. The existing unclean-filesystem,
unwritten-extent and compressed-piece exclusions remain. Bytes 3 through 7
are not interpreted. #230's absent-backpointer disagreement and exact
baseline entry remain unchanged.

The target snapshot changed alone is not damage to the reference: in
[run 39094186901](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39094186901)
(#348) the `default` image passed the word-one and length mutations, and
the reference checker 1.39.7 judged the snapshot-only mutation (U32_MAX to
0) clean, with no finding. The comparison therefore covers the target
inode and offset, not its snapshot; the fixture and reference regression
requires both checkers to call that case clean. This is black-box checker
output only.

The test-only commit precedes the fix. Unit tests cover those fields,
short values and the existing lookup boundaries; fixture and independent
reference regressions change the length and snapshot together and alone,
with clean controls and unchanged extents. The digest-pinned CI job replays
both exact seeds. Tests run in CI, not locally; no GPL source was read.

### A pinned extent checksum and the reference's data read (S3, S8, #219)

[Run 38783376400](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38783376400)
records two different observations from the reference tools 1.39.7. Its
checker accepts both the unchanged image and the changed image with status
0. Through its FUSE mount, inode 2147483904 in the unchanged image reads
successfully with SHA-256
`fd7023d87ea3679ca779d9e8d671ac04a58df4c91ce26b2774cd06623f1f6a9d`;
the same inode in the changed image returns `Input/output error`.

This observation is bound to fixtures artifact 11977252185 from run
38741040375, digest
`sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e`.
The changed image has SHA-256
`3b58375372070ca854c7600c54cca29d64a51921d68facea8eabaf4285e433eb`.
The reference's extent listing names key `2147483904:1:U32_MAX`, with
stored CRC32C `0x11ba7`; the unchanged image lists `0xa2f1ba7`, matching
the independently computed checksum. Artifact 11996897949 retains the
listing and read observations.

`scripts/pinned-default74-reference.sh` requires that exact fixture,
physical image, extent, inode and checksum binding, the recorded control
bytes, and the changed image's EIO. Its CI controls reject successful
reads, timeouts, permission failures, missing observations and mismatched
identity. The differential class remains open under #219; this evidence
establishes the distinction between the reference checker's acceptance
and its data reader's integrity refusal for this one pinned case.

### A primary-only bucket-size change (S3, S4, #247)

Nightly differential run
[38878947701](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38878947701)
reported `aged.img` seed 1659384 as a crate-only
`missing_fragmentation_lru_entry`; the reference checker 1.39.7 accepted
the control and the changed image with exit 0. Its inputs were fixtures
artifact 11977252185 from run 38741040375, archive SHA-256
`9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e`,
tarball SHA-256
`27b15c9dffa3240b2ec3f3f3a84221c255c210b157f5888d2285f4e358025869`.

Replaying the mutation's RNG over the recorded 8272-byte superblock
identifies one flipped bit before re-stamping: byte 915, bit 5. The
fixture's bytes put that byte in member 0's `bucket_size` in `members_v2`:
the field starts at superblock byte 872, its member at 888, and the
little-endian bucket size at 914 changes from 256 to 8448 sectors.
Only the primary at sector 8 changes; its sequence stays 133, equal to
the copies at sectors 2056 and 522240. These are observations of the
image and the reference's output, not implementation-source facts.

`oracle_superblock_copies.rs` retains the primary-only regression by
locating the member field, so it also runs over rebuilt fixtures.
`check_oracle.rs` asks the reference checker about the control, each
changed copy at the same sequence, a newer primary, and all copies
changed together, before comparing this crate's verdicts.

## CLI device windows (#295)

`--offset` changes the device window, without adding any interpretation of
the on-disk format. `tests/cli/test-offset.sh` embeds the reference-made
default fixture at byte 1048576, compares read-only verb output byte for byte
with the offset-zero image, and checks every regular file against the guest's
recorded SHA-256 manifest (S3). The verb table requires every implemented
command, including later additions, to reject a past-end device offset;
unimplemented commands retain their refusal before opening any target.

`tests/cli/test-write.sh` repeats every write verb in place and through the
journal on an embedded reference-made image. Prefix and suffix sentinels
must survive. `cli_offset_writes_are_read_by_the_reference` in
`tests/write_oracle.rs` extracts the exact written partition bytes for the
guest's reference checker, lister and read-only mount to judge (S3, S8).
This is black-box CLI use only; no reference source is read or compared.

### Optional fields are validated after superblock selection (S3, S4, #261)

[Nightly run 39018152705](https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/39018152705)
reported four crate-only `superblock_copy` disagreements. Its examples,
`aged-unclean` and `kernel` seed 1659440, change the primary superblock's
field-16 length at byte 7288 from 3 words to 0 and re-stamp its checksum.
The parser's diagnostic offset 6536 is relative to the optional-field area,
which starts at byte 752. Both reference verdicts are clean, exit 0.
The fixtures and their digests are the pinned artifact recorded for #253.

Fresh black-box observations on 2026-10-17 use that exact aged-unclean
fixture and the harness guest's installed reference build `1.39.7 fuse`.
[The retained observation](../tests/data/superblock-copy-261-reference.txt)
records the invocation, image hashes, mutations, statuses and exact diagnostic
text. The unchanged control, a primary-only zero-length field, and a
middle-copy-only zero-length field are accepted with exit 0 and no superblock
diagnostic. The same change to the last equal-sequence copy, a primary whose
sequence is incremented, or every copy is refused with exit 1 and
`invalid_sb_field_size`: "Invalid superblock: optional field with size 0
(type 16)". Thus optional-field validation applies to the sequence-winning
copy; an invalid selected field is not grounds for falling back to an older
copy. Existing last-copy sequence tie-breaking is corroborated here.

Two physical-damage controls also exit 0 but explicitly report "primary
superblock unreadable": one for a bad checksum, one for zeroed magic. The
checker retains those diagnostics. It verifies each candidate's header and
checksum before selection and parses optional fields only in the winner;
the standalone single-copy parser still rejects a zero-length field.

The test-only commit precedes the implementation. The pinned CI regression
replays both reported examples; ordinary fixture and live-reference cases
cover selected and unselected copies, a newer primary, all copies, and an
unrestamped mutation. Differential pull requests replay the same pinned
fixtures, 80 mutations per fixture and first seed 1659440. Only #261's exact
baseline allowance is removed. Suites run in CI, not locally. No reference
source was read; the observations above came only from its executable output.

## Oversized stale journal entries (#262, 2026-10-17)

Source: black-box execution of the harness guest's installed reference
checker, build marker `1.39.7 fuse`, on the reference-made aged-unclean
image from CI run 38741040375, artifact 11977252185. Artifact digest:
`sha256:9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e`;
tarball SHA-256:
`27b15c9dffa3240b2ec3f3f3a84221c255c210b157f5888d2285f4e358025869`.
[Retained observations](../tests/data/journal-length-262-reference.txt)
record the command, mutations, statuses and exact verdict lines. No reference
source was read.

The exact seed 1659479 changes jset-relative bytes [34,48) to ff at image
byte 2490368 (sector 4864, sequence 206). The baseline checker reports clean;
the reference exits 9, names `jset_past_bucket_end`, and prints
`journal entry too big (34359738416 bytes), fixing`. Its version label is
`(unknown version) (65473.39)`. The emitter calls this mutation re-stamped,
but its restamper actually leaves the checksum unchanged because the claimed
length exceeds its input buffer.

Isolation controls distinguish the cause from that version label. Changing
only the word at [40,44) to `u32::MAX`, without changing its checksum, gives
the same size diagnostic and exit 9 with the original version. Changing
only [34,36) to ff, with a recomputed CRC32C, is accepted with exit 0.
Changing [34,40) while preserving the size is also accepted. Thus this
evidence does not establish an unknown-version rejection rule. The printed
size equals 56 + 8 * the little-endian word at offset 40, corroborating the
existing header and length interpretation. The unchanged image and a control
with zeroed magic and an oversized length also exit 0.

The reference replays sequences 304 through 306 in all these cases, so the
size violation at sequence 206 is diagnosed even outside the replay window,
and before a checksum can be verified. The reader now reports an oversized
matching journal header immediately instead of treating it as absent. This
does not change the existing treatment of sub-entry damage outside replay,
wrong magic, or torn checksums. Tests cover the exact pinned mutation and
size/version isolation on freshly generated reference fixtures; the native
case requires the exact reference statuses and size diagnostic. Floors rise
by one unit, one oracle and one native case. Suites remain CI-only under the
issue-agent brief; local validation builds the test targets and reproduces
the differential verdict directly. Only #262's exact allowance is removed.

## No comparison against the reference source

Decided 2026-10-17 (#160). This record is evidence about the *process*: the
sources used and what each told us. The other half of a clean-room defence is
evidence about the *outcome*, that no expression of the GPL implementation
ended up here, and the obvious way to produce it is to compare this crate
against the reference source. **The project does not do that, in any form**:
no similarity scan, no CI job and no tool in this repository reads the
reference source beside this crate's code.

The reason is that a comparison needs the reference source as an input.
Built into the project, it becomes a standing channel from that source to the
people writing this crate: a report that a passage resembles the reference
tells its reader something about the reference, and a rewrite made to clear
the report is shaped by it. That is the contamination this record exists to
rule out. Shipping the tool would also invite the reading that the project
consults GPL code as a matter of routine, which is the opposite of what it
is meant to show. The DKMS oracle (S10) is different in kind: its container
builds the reference module and runs it, and nothing in it reads the source
or reports on its text.

If outcome evidence is ever needed, it comes from outside the project: an
independent audit commissioned by the owner, in which the auditor (a person
or a third-party agent) is given both this crate and the reference source,
and everything it worked with is discarded afterwards. The audit's report is
its only artifact. The report names this crate's side only and quotes
nothing from the reference, and nothing from the audit becomes part of this
repository's tooling.

## Confirmation

No GPL source code (kernel `fs/bcachefs`, `bcachefs-tools`, or any crate or
snippet derived from them) was read while writing this crate. The only file
from the tools' repository that was opened is `INSTALL.md` (build
instructions, S2); the only other access was GitHub API metadata (S7) and
the package repository's index and dependency metadata (S10). The
reference kernel module is built by DKMS in a throwaway container on the CI
runner and its source discarded with it, unread (#110).
