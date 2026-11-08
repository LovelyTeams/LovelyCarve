<div align="center">

# udif

**Cross-platform Rust library for Apple DMG disk images (aka Universal Disk Image Format)**

[![Crates.io](https://img.shields.io/crates/v/udif.svg)](https://crates.io/crates/udif)
[![Documentation](https://docs.rs/udif/badge.svg)](https://docs.rs/udif)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-windows%20%7C%20linux%20%7C%20macos-lightgrey)

Read, write, and manipulate Apple DMG files on any platform.

**Full LZFSE support** — works with modern macOS disk images that other libraries can't read.

</div>

---

## Why udif?

**udif is the only cross-platform Rust crate that can both read AND write DMGs with modern compression.**

| Feature | **udif** | dmgwiz | apple-dmg | dmg-oxide |
|---------|:-------:|:------:|:---------:|:---------:|
| Read | ✓ | ✓ | ✓ | ✓ |
| **Write** | ✓ | ❌ | ❌ | ✓ |
| LZFSE | ✓ | ✓ | ❌ | ❌ |
| XZ | ✓ | ❌ | ❌ | ❌ |
| Bzip2 | ✓ | ✓ | ❌ | ❌ |
| Zlib | ✓ | ✓ | ✓ | ✓ |
| **Checksum** | ✓ | ❌ | ❌ | ❌ |

> **Example:** A typical macOS Kernel Debug Kit DMG uses 100% LZFSE compression.
> Only udif and dmgwiz can read it — but only udif can write new DMGs with LZFSE.

## Features

| | |
|---|---|
| **List partitions** | Parse DMG structure, show partition table |
| **Extract data** | Decompress partitions to raw disk images |
| **Create DMG** | Build DMG files with multiple compression options |
| **Checksum verification** | CRC32 integrity validation on read and write |
| **Cross-platform** | Works on Windows, Linux, and macOS |

### Compression Support

| Format | Read | Write | Description |
|--------|:----:|:-----:|-------------|
| LZFSE | | | Apple's native compression |
| XZ | | | XZ (LZMA2) compression |
| Zlib | | | Best compatibility |
| Bzip2 | | | Better ratio, slower |
| Raw | | | No compression |

## Quick Start

### Read DMG

```rust
use udif::DmgArchive;

// Open and list partitions
let mut archive = DmgArchive::open("image.dmg")?;
for p in archive.partitions() {
    println!("{}: {} bytes", p.name, p.size);
}

// Extract main HFS+/APFS partition
let data = archive.extract_main_partition()?;
std::fs::write("disk.raw", &data)?;
```

### Create DMG

```rust
use udif::{DmgBuilder, CompressionMethod};

let disk_data = std::fs::read("disk.raw")?;

DmgBuilder::new()
    .compression(CompressionMethod::Zlib)
    .add_partition("Macintosh HD", disk_data)
    .build("output.dmg")?;
```

### Checksum Verification

By default, checksums are verified when opening a DMG. To skip verification (e.g., for corrupted files):

```rust
use udif::{DmgArchive, DmgReaderOptions};

// Default: checksums verified
let archive = DmgArchive::open("image.dmg")?;

// Skip verification for corrupted/legacy files
let options = DmgReaderOptions { verify_checksums: false };
let archive = DmgArchive::open_with_options("image.dmg", options)?;
```

## Documentation

| | |
|---|---|
| [Format Specification](docs/FORMATS.md) | DMG binary format details |
| [CLI Tool](docs/CLI.md) | Command-line tool usage |
| [Implementation Notes](docs/IMPLEMENTATION.md) | Tricky parts and gotchas |

## Example Output

```
$ udif-tool info Kernel_Debug_Kit.dmg

DMG Information: Kernel_Debug_Kit.dmg
============================================================

Header:
  Version:          4
  Sector count:     2351263
  Data fork length: 1023271318 bytes
  Segment:          1/1

Size:
  Uncompressed:     1203845632 bytes (1148.08 MB)
  Compressed:       1023271318 bytes (975.87 MB)
  Compression:      15.0%

Partitions:         7

Block types used:
  LZFSE:            988 blocks
```

```
$ udif-tool list Kernel_Debug_Kit.dmg

Partitions in Kernel_Debug_Kit.dmg:
================================================================================
  ID       Sectors          Size     Ratio  Name
--------------------------------------------------------------------------------
  -1             1         512 B     89.6%  Protective Master Boot Record (MBR : 0)
   0             1         512 B     78.9%  GPT Header (Primary GPT Header : 1)
   1            32      16.00 KB     98.7%  GPT Partition Data (Primary GPT Table : 2)
   2       2089050    1020.04 MB      4.3%  Untitled 1 (Apple_HFSX : 3)
   3        262144     128.00 MB    100.0%   (Apple_Free : 4)
```

## Alternatives

| Crate | Read | Write | LZFSE | XZ | Bzip2 | Zlib | Checksum | Encrypted | Notes |
|-------|:----:|:-----:|:-----:|:--:|:-----:|:----:|:--------:|:---------:|-------|
| **udif** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ❌ | Full featured |
| [dmgwiz](https://crates.io/crates/dmgwiz) | ✓ | ❌ | ✓ | ❌ | ✓ | ✓ | ❌ | ✓ | Read-only |
| [apple-dmg](https://crates.io/crates/apple-dmg) | ✓ | ❌ | ❌ | ❌ | ❌ | ✓ | ❌ | ❌ | Zlib only |
| [dmg-oxide](https://crates.io/crates/dmg-oxide) | ✓ | ✓ | ❌ | ❌ | ❌ | ✓ | ❌ | ❌ | Zlib only |
| [dmg](https://crates.io/crates/dmg) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | macOS only (hdiutil) |

**Choose udif if you need:**
- Write/create DMG support AND modern compression (LZFSE/XZ)
- Cross-platform support (Windows, Linux, macOS)
- XZ compression (no other cross-platform crate supports it)
- Ensure integrity (no other cross-platform crate have checksum generation / validation)

**Choose dmgwiz if you need:**
- Encrypted DMG support (we don't have this yet)
- Read-only access is sufficient

See [Comparison](docs/COMPARISON.md) for detailed analysis.

## License

MIT
