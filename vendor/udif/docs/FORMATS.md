# DMG Format Specification

This document describes the Apple DMG (Disk Image) binary format.

## Overview

DMG files contain compressed disk image data with the following structure:

```
+------------------------+
| Compressed Data Blocks |  Partition data (LZFSE, Zlib, etc.)
+------------------------+
| XML Plist              |  Block maps and partition info
+------------------------+
| Koly Trailer           |  512-byte header at end of file
+------------------------+
```

## Koly Trailer

The koly trailer is **always 512 bytes** located at the **end of the file**.

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 4 | ASCII | Magic `koly` (0x6B6F6C79) |
| 4 | 4 | u32 BE | Version (usually 4) |
| 8 | 4 | u32 BE | Header size (512) |
| 12 | 4 | u32 BE | Flags |
| 16 | 8 | u64 BE | Running data fork offset |
| 24 | 8 | u64 BE | Data fork offset |
| 32 | 8 | u64 BE | Data fork length |
| 40 | 8 | u64 BE | Resource fork offset |
| 48 | 8 | u64 BE | Resource fork length |
| 56 | 4 | u32 BE | Segment number |
| 60 | 4 | u32 BE | Segment count |
| 64 | 16 | bytes | Segment ID (UUID) |
| 80 | 4 | u32 BE | Data checksum type |
| 84 | 4 | u32 BE | Data checksum size |
| 88 | 128 | bytes | Data checksum |
| 216 | 8 | u64 BE | **Plist offset** |
| 224 | 8 | u64 BE | **Plist length** |
| 232 | 64 | bytes | Reserved |
| 296 | 4 | u32 BE | Master checksum type |
| 300 | 4 | u32 BE | Master checksum size |
| 304 | 128 | bytes | Master checksum |
| 432 | 4 | u32 BE | Image variant |
| 436 | 8 | u64 BE | Sector count |
| 444 | 68 | bytes | Reserved (padding to 512) |

## XML Plist

The plist contains partition information in the `resource-fork/blkx` array:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>resource-fork</key>
    <dict>
        <key>blkx</key>
        <array>
            <dict>
                <key>Attributes</key>
                <string>0x0050</string>
                <key>CFName</key>
                <string>Partition Name</string>
                <key>Data</key>
                <data>BASE64_ENCODED_MISH_DATA</data>
                <key>ID</key>
                <string>0</string>
                <key>Name</key>
                <string>Partition Name</string>
            </dict>
            <!-- ... more partitions ... -->
        </array>
    </dict>
</dict>
</plist>
```

## Mish Block Map

Each partition has a mish (block map) structure. **Total header size is 204 bytes**.

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 4 | ASCII | Magic `mish` (0x6D697368) |
| 4 | 4 | u32 BE | Version |
| 8 | 8 | u64 BE | First sector |
| 16 | 8 | u64 BE | Sector count |
| 24 | 8 | u64 BE | Data offset |
| 32 | 4 | u32 BE | Buffers needed |
| 36 | 4 | u32 BE | Block descriptor count (often partition index) |
| 40 | 24 | bytes | Reserved |
| 64 | 4 | u32 BE | Checksum type |
| 68 | 4 | u32 BE | Checksum size |
| 72 | 128 | bytes | Checksum |
| **200** | **4** | **u32 BE** | **Actual block run count** |
| 204 | ... | BlockRun[] | Block run array |

> **Important**: The actual block count is at offset 200, not offset 36.

## Block Run

Each block run is **exactly 40 bytes**:

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 4 | u32 BE | Block type |
| 4 | 4 | u32 BE | Comment |
| 8 | 8 | u64 BE | Sector number (output offset) |
| 16 | 8 | u64 BE | Sector count |
| 24 | 8 | u64 BE | Compressed offset (in data fork) |
| 32 | 8 | u64 BE | Compressed length |

### Block Types

| Value | Name | Description |
|-------|------|-------------|
| `0x00000000` | ZeroFill | Output sectors are zeros (no data stored) |
| `0x00000001` | Raw | Uncompressed data |
| `0x00000002` | Ignore | Skip/ignore block |
| `0x80000004` | ADC | ADC compression (legacy) |
| `0x80000005` | Zlib | Zlib compression |
| `0x80000006` | Bzip2 | Bzip2 compression |
| `0x80000007` | LZFSE | LZFSE compression (Apple) |
| `0x80000008` | XZ | XZ (LZMA2) compression |
| `0x7FFFFFFE` | Comment | Comment block (no data) |
| `0xFFFFFFFF` | End | End of partition marker |

## Compression Formats

### LZFSE

Apple's native compression. Data starts with magic:
- `bvxn` - LZFSE compressed
- `bvx$` - LZFSE end marker
- `bvx-` - LZVN compressed (handled transparently by the lzfse decoder)

### XZ (LZMA2)

Standard XZ format. Data starts with magic bytes `FD 37 7A 58 5A 00`.

### Zlib

Standard zlib format with header bytes `78 9C` (default) or `78 01` (fast).

### Bzip2

Standard bzip2 format with magic `BZ`.

## Partition Types

Common partition names:

| Name | Description |
|------|-------------|
| `Apple_HFSX` | HFS+ case-sensitive filesystem |
| `Apple_HFS` | HFS+ filesystem |
| `Apple_APFS` | APFS container |
| `Apple_Free` | Free/unused space |
| `EFI` | EFI system partition |
| `MBR` | Protective Master Boot Record |
| `GPT Header` | GUID Partition Table header |
| `GPT Partition Data` | GPT partition entries |

## Checksums

DMG files use CRC32 checksums (type 2) for integrity verification at three levels:

### Checksum Types

| Type | Value | Description |
|------|-------|-------------|
| None | 0 | No checksum |
| CRC32 | 2 | 32-bit CRC (standard) |

### Checksum Locations

| Location | Field | Verifies |
|----------|-------|----------|
| Koly trailer | `data_checksum` | Data fork (compressed blocks) |
| Koly trailer | `master_checksum` | CRC32 of all mish checksums concatenated |
| Mish header | `checksum` | Decompressed partition data |

### Checksum Format

Checksums are stored in 128-byte arrays with the actual CRC32 value in the first 4 bytes (big-endian):

```
Checksum Array (128 bytes):
├── CRC32 value:   4 bytes (u32 BE)
└── Padding:       124 bytes (zeros)
```

### Verification Order

When opening a DMG:
1. Read koly header
2. Verify data fork checksum (if present)
3. Parse plist and mish headers
4. Verify master checksum (if present)
5. On partition extraction: verify mish checksum (if present)

> **Note:** Zero checksums are skipped during verification. Legacy DMGs may have zero checksums.

## Sector Size

All sector values use **512-byte sectors**.

```
uncompressed_size = sector_count * 512
output_offset = sector_number * 512
```
