# AD1 on-disk format

The AccessData AD1 ("Custom Content Image") layout as implemented by `ad1-core`,
derived from the [al3ks1s/AD1-tools](https://github.com/al3ks1s/AD1-tools)
reverse-engineered C reference (`libad1`) and cross-checked against the
[Cerbero AD1 package](https://blog.cerbero.io/ad1-format-package/) and
[DFIRScience's overview](https://dfir.science/2021/09/What-is-an-AD1.html).

AD1 is a **logical** container — a tree of files/folders with per-file metadata
and zlib-compressed data — not a sector image. All integers are **little-endian**.

## Logical addressing

Tree/metadata/chunk addresses are offsets into a virtual space formed by
concatenating each segment's body with its 512-byte margin removed:

```
usable_per_segment = fragments_size * 65536 - 512
segment_index      = logical_offset / usable_per_segment
within_segment     = logical_offset % usable_per_segment
physical_offset    = within_segment + 512
```

A read that runs past one segment's data continues in the next (`.ad2`, …).

## Detection (first bytes of segment 1)

| Bytes | Meaning |
|---|---|
| `ADCRYPT\0` | encrypted variant — **refused** (`Ad1Error::Unsupported`) |
| `ADSEGMENTEDFILE\0` | normal AD1 |
| anything else | not AD1 (the bytes are shown in the error) |

## Segment header (offset 0 of each `.adN`)

| Offset | Type | Field |
|---|---|---|
| 0x00 | char[16] | signature `ADSEGMENTEDFILE\0` |
| 0x18 | u32 | segment_index |
| 0x1c | u32 | segment_number (total segments) |
| 0x22 | u32 | fragments_size (segment size in 64 KiB units) |
| 0x28 | u32 | header_size |

## Logical header (physical 0x200 = logical 0)

| Offset | Type | Field |
|---|---|---|
| 0x200 | char[15] | signature |
| 0x210 | u32 | image_version (commonly 3 or 4) |
| 0x218 | u32 | zlib_chunk_size (max decompressed bytes per chunk) |
| 0x21c | u64 | logical_metadata_addr |
| 0x224 | u64 | first_item_addr |
| 0x22c | u32 | data_source_name_length |
| 0x234 | u64 | data_source_name_addr |

## Item header (at a logical offset)

| Offset | Type | Field |
|---|---|---|
| +0x00 | u64 | next_item_addr (sibling) |
| +0x08 | u64 | first_child_addr |
| +0x10 | u64 | first_metadata_addr |
| +0x18 | u64 | zlib_metadata_addr (chunk table; 0 if none) |
| +0x20 | u64 | decompressed_size (0 for directories) |
| +0x28 | u32 | item_type (0 = file, 5 = folder) |
| +0x2c | u32 | item_name_length |
| +0x30 | char[name_length] | item_name (`/` mapped to `_`) |
| +0x30+len | u64 | parent_folder |

Tree walk: from `first_item_addr`, recurse `first_child` then `next_item`
siblings. Paths join ancestor names with `/`.

## Metadata record (at a logical offset)

| Offset | Type | Field |
|---|---|---|
| +0x00 | u64 | next_metadata_addr |
| +0x08 | u32 | category |
| +0x0c | u32 | key |
| +0x10 | u32 | data_length |
| +0x14 | u8[data_length] | data |

Category `0x01` HASH_INFO (key `0x5001` MD5 — 32 ASCII hex; `0x5002` SHA1 — 40
ASCII hex); `0x05` TIMESTAMP (keys `0x07` access, `0x08` modified, `0x09` change;
value `YYYYMMDDThhmmss`).

## Chunk table (at `item.zlib_metadata_addr`) and data

```
u64 chunk_count
u64 address[0 ..= chunk_count]   // chunk_count + 1 entries
```

Chunk *i* compressed bytes occupy `[address[i], address[i+1])`; each is an
independent **zlib** stream inflating to at most `zlib_chunk_size` bytes (the
last is smaller). Decompressed byte *b* lives in chunk `b / zlib_chunk_size`, so
`read_at` inflates only the chunks a range overlaps.

## Hash verification (the auditor)

`ad1-forensic` recomputes MD5/SHA1 over a file's decompressed content and
compares to the stored hex; a mismatch is `AD1-HASH-MISMATCH`.
