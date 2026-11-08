# Implementation Notes

This document describes non-obvious implementation details discovered while building this library.

## Tricky Pieces

### 1. Koly Header Position

The koly magic bytes are at **offset -512** from the end of the file, not -4.

```
Wrong:  file_size - 4   = koly magic  ❌
Right:  file_size - 512 = koly magic  ✓
```

The entire koly header is 512 bytes, and "koly" appears at the start of this header.

```rust
// Correct way to check for DMG
fn is_dmg(reader: &mut R) -> bool {
    reader.seek(SeekFrom::End(-512))?;  // Not -4!
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    magic == *b"koly"
}
```

### 2. Mish Block Count Location

The **actual block count** is at offset 200 in the mish header, not offset 36.

```
Offset 36:  Often contains partition index (misleading!)
Offset 200: Contains actual block run count  ✓
```

The field at offset 36 (`block_descriptor_count`) often contains the partition's index or other metadata, not the number of block runs.

```rust
// Wrong
let block_count = read_u32_at_offset(36);  // ❌ Unreliable

// Correct
let block_count = read_u32_at_offset(200); // ✓ Actual count
```

### 3. Mish Header Size is 204 Bytes

The mish header is **204 bytes**, not 200. Block runs start at offset 204.

```
Offset 0-199:   Standard mish fields + checksum
Offset 200-203: Actual block count (4 bytes)
Offset 204+:    Block runs begin
```

This extra 4-byte field is often undocumented but critical for correct parsing.

### 4. LZFSE Buffer Size Requirements

The LZFSE decoder requires an output buffer **larger than the actual decompressed size**.

```rust
// This may fail with BufferTooSmall
let mut output = vec![0u8; expected_size];
lzfse::decode_buffer(&compressed, &mut output)?;  // ❌

// This works - allocate extra space
let mut output = vec![0u8; expected_size * 2];
let actual_size = lzfse::decode_buffer(&compressed, &mut output)?;
output.truncate(actual_size);  // ✓
```

The library uses 2x the expected size to be safe.

### 5. Partial Sector Decompression

Decompressed data may be **smaller than `sector_count * 512`** when the original file wasn't sector-aligned.

```rust
// Wrong - assumes exact size
decoder.read_exact(&mut buffer[0..sector_count * 512])?;  // ❌ May fail

// Correct - allow partial reads
let bytes_read = decoder.read(&mut buffer[0..sector_count * 512])?;  // ✓
// bytes_read may be less than sector_count * 512
```

This happens when creating DMGs from files that aren't multiples of 512 bytes.

### 6. Block Run Size is Fixed

Each block run is **exactly 40 bytes**. This is critical for parsing.

```
Block Run (40 bytes):
├── block_type:        4 bytes (u32 BE)
├── comment:           4 bytes (u32 BE)
├── sector_number:     8 bytes (u64 BE)
├── sector_count:      8 bytes (u64 BE)
├── compressed_offset: 8 bytes (u64 BE)
└── compressed_length: 8 bytes (u64 BE)
                      ────────────────
                      40 bytes total
```

### 7. Koly Header Padding

The koly header must be **exactly 512 bytes**. Careful calculation of padding is required:

```
Koly Header Fields:
  magic + version + header_size + flags           = 16 bytes
  running_offset + data_offset + data_length      = 24 bytes
  rsrc_offset + rsrc_length                       = 16 bytes
  segment_number + segment_count + segment_id     = 24 bytes
  data_checksum_type + size + checksum            = 136 bytes
  plist_offset + plist_length                     = 16 bytes
  reserved                                        = 64 bytes
  master_checksum_type + size + checksum          = 136 bytes
  image_variant + sector_count                    = 12 bytes
  ────────────────────────────────────────────────────────────
  Subtotal                                        = 444 bytes
  Padding needed                                  = 68 bytes
  ────────────────────────────────────────────────────────────
  Total                                           = 512 bytes
```

### 8. Raw Block Decompression Length

For Raw/uncompressed blocks, you must read **`compressed_length` bytes**, not `sector_count * 512`.

```rust
// Wrong - tries to read more bytes than stored
let size = block_run.sector_count * 512;
reader.read_exact(&mut output[..size])?;  // ❌ May read past data!

// Correct - read only the stored data
let size = block_run.compressed_length;
reader.read_exact(&mut output[..size])?;  // ✓
// Remaining bytes stay zero-filled
```

This matters because the writer stores the actual data size, which may be smaller than the padded sector size.

### 9. Checksum Verification

CRC32 checksums are stored in the first 4 bytes of 128-byte arrays (big-endian).

```rust
// Extract CRC32 from checksum array
let crc = u32::from_be_bytes(checksum_array[0..4].try_into()?);

// Zero checksum means "not set" - skip verification
if crc == 0 {
    return Ok(());
}
```

Three checksums exist:
- **Data fork checksum**: CRC32 of all compressed data blocks
- **Master checksum**: CRC32 of all partition checksums concatenated (4 bytes each)
- **Mish checksum**: CRC32 of the decompressed partition data (padded to sector boundary)

### 10. Partition Checksum Padding

When calculating mish checksums, data must be **padded to sector boundary** with zeros:

```rust
// Wrong - checksum of raw data
let checksum = crc32(data);  // ❌

// Correct - pad to sector boundary first
let sector_count = (data.len() + 511) / 512;
let padded_size = sector_count * 512;
let mut padded = data.to_vec();
padded.resize(padded_size, 0);
let checksum = crc32(&padded);  // ✓
```

## Testing Strategy

All tricky pieces have dedicated tests:

| Test | Validates |
|------|-----------|
| `test_koly_header_size_is_512` | Koly serializes to exactly 512 bytes |
| `test_koly_magic_position` | Magic is at -512, not -4 |
| `test_mish_block_count_at_offset_200` | Block count from offset 200, not 36 |
| `test_mish_header_size_is_204` | Header is 204 bytes |
| `test_lzfse_needs_larger_buffer` | LZFSE buffer sizing |
| `test_zlib_partial_sector` | Partial decompression handling |
| `test_block_run_size` | Block runs are exactly 40 bytes |
| `test_is_dmg_checks_correct_offset` | DMG detection uses correct offset |
| `test_checksum_roundtrip_with_verification` | Checksums written and verified correctly |
| `test_checksum_detection_corrupted_data` | Corrupted data fork detected |
| `test_checksum_all_compression_methods` | Checksums work with all compression types |

Run tests with:

```bash
cargo test -p udif
```

## References

- [Apple Disk Image (Wikipedia)](https://en.wikipedia.org/wiki/Apple_Disk_Image)
- [DMG format analysis](https://newosxbook.com/DMG.html)
- [hdiutil man page](https://ss64.com/osx/hdiutil.html)
- [apple-platform-rs](https://github.com/indygreg/apple-platform-rs) — Related Rust crates for Apple platforms
