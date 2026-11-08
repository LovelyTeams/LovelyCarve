# HFS+ Format Specification

This document describes the HFS+ / HFSX on-disk format as implemented by the hfsplus library.

## Overview

HFS+ (Hierarchical File System Plus) is Apple's filesystem format used from Mac OS 8.1 (1998) through macOS 10.15 (2019). HFSX is the case-sensitive variant introduced in Mac OS X 10.3.

```
+----------------------------+
| Reserved (1024 bytes)      |  Boot blocks
+----------------------------+
| Volume Header (512 bytes)  |  At offset 1024
+----------------------------+
| Allocation Bitmap          |  Block usage tracking
+----------------------------+
| Extents Overflow B-tree    |  Extra extent records
+----------------------------+
| Catalog B-tree             |  File/folder metadata
+----------------------------+
| Attributes B-tree          |  Extended attributes
+----------------------------+
| Data blocks                |  File data
+----------------------------+
| Alternate Volume Header    |  Backup at -1024 from end
+----------------------------+
```

## Volume Header

The volume header is **512 bytes** at **offset 1024** from the start of the volume.

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 2 | u16 BE | Signature: `0x482B` (H+) or `0x4858` (HX) |
| 2 | 2 | u16 BE | Version: 4 (HFS+) or 5 (HFSX) |
| 4 | 4 | u32 BE | Attributes |
| 8 | 4 | u32 BE | Last mounted version |
| 12 | 4 | u32 BE | Journal info block |
| 16 | 4 | u32 BE | Creation date |
| 20 | 4 | u32 BE | Modification date |
| 24 | 4 | u32 BE | Backup date |
| 28 | 4 | u32 BE | Checked date |
| 32 | 4 | u32 BE | File count |
| 36 | 4 | u32 BE | Folder count |
| 40 | 4 | u32 BE | Block size |
| 44 | 4 | u32 BE | Total blocks |
| 48 | 4 | u32 BE | Free blocks |
| 52 | 4 | u32 BE | Next allocation |
| 56 | 4 | u32 BE | Resource clump size |
| 60 | 4 | u32 BE | Data clump size |
| 64 | 4 | u32 BE | Next catalog ID |
| 68 | 4 | u32 BE | Write count |
| 72 | 8 | u64 BE | Encoding bitmap |
| 80 | 32 | bytes | Finder info (8 x u32) |
| 112 | 80 | ForkData | Allocation file |
| 192 | 80 | ForkData | Extents file |
| 272 | 80 | ForkData | Catalog file |
| 352 | 80 | ForkData | Attributes file |
| 432 | 80 | ForkData | Startup file |

### Signatures

| Value | Name | Description |
|-------|------|-------------|
| `0x482B` | HFS+ | Standard Mac OS Extended |
| `0x4858` | HFSX | Case-sensitive Mac OS Extended |

### Dates

HFS+ dates are **seconds since 1904-01-01 00:00:00 UTC**. To convert to Unix time, subtract 2082844800.

## Fork Data

Each special file is described by a Fork Data structure (80 bytes):

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 8 | u64 BE | Logical size |
| 8 | 4 | u32 BE | Clump size |
| 12 | 4 | u32 BE | Total blocks |
| 16 | 64 | ExtentDescriptor[8] | First 8 extents |

### Extent Descriptor (8 bytes)

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 4 | u32 BE | Start block |
| 4 | 4 | u32 BE | Block count |

## B-tree Structure

HFS+ uses B-trees for the catalog, extents, and attributes files. Each B-tree has a header node at node 0.

### B-tree Header Record

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 2 | u16 BE | Tree depth |
| 2 | 4 | u32 BE | Root node |
| 6 | 4 | u32 BE | Leaf records |
| 10 | 4 | u32 BE | First leaf node |
| 14 | 4 | u32 BE | Last leaf node |
| 18 | 2 | u16 BE | Node size |
| 20 | 2 | u16 BE | Max key length |
| 22 | 4 | u32 BE | Total nodes |
| 26 | 4 | u32 BE | Free nodes |

### Node Types

| Value | Type | Description |
|-------|------|-------------|
| -1 | Leaf | Contains records |
| 0 | Index | Points to child nodes |
| 1 | Header | B-tree header |
| 2 | Map | Node allocation bitmap |

## Catalog Records

The catalog B-tree maps (parent CNID, name) → record.

### Record Types

| Value | Type | Description |
|-------|------|-------------|
| 1 | Folder | Directory record |
| 2 | File | File record |
| 3 | Folder thread | Maps CNID → parent |
| 4 | File thread | Maps CNID → parent |

### Special CNIDs

| CNID | Name | Description |
|------|------|-------------|
| 1 | Root parent | Parent of root folder |
| 2 | Root folder | The filesystem root `/` |
| 3 | Extents file | Extents overflow B-tree |
| 4 | Catalog file | Catalog B-tree |
| 5 | Bad blocks | Bad allocation blocks |
| 6 | Allocation file | Allocation bitmap |
| 7 | Startup file | Boot file |
| 8 | Attributes file | Extended attributes |
| 15 | Repair catalog | Repair data |
| 16 | Bogus folder | First user CNID |

## Unicode Handling

HFS+ file names use a decomposed Unicode variant (similar to NFD but with Apple-specific rules). The catalog B-tree key contains the name length (u16) followed by UTF-16BE characters.

HFSX volumes use binary comparison; HFS+ volumes use case-insensitive comparison with Unicode folding tables.
