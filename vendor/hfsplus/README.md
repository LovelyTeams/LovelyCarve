<div align="center">

# hfsplus

**Cross-platform Rust library for reading Apple HFS+ and HFSX filesystems**

[![Crates.io](https://img.shields.io/crates/v/hfsplus.svg)](https://crates.io/crates/hfsplus)
[![Documentation](https://docs.rs/hfsplus/badge.svg)](https://docs.rs/hfsplus)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-windows%20%7C%20linux%20%7C%20macos-lightgrey)

Parse HFS+ / HFSX volumes from raw disk images on any platform — no kernel drivers or FUSE required.

**Pure Rust, zero unsafe** — works everywhere Rust compiles.

</div>

---

## Why hfsplus?

**hfsplus is the only pure-Rust library for reading HFS+ filesystems with B-tree traversal and extent overflow support.**

| Feature | **hfsplus** | hfsplus-rs | hfs-rs |
|---------|:-----------:|:----------:|:------:|
| HFS+ | ✓ | ✓ | ❌ |
| HFSX (case-sensitive) | ✓ | ❌ | ❌ |
| B-tree catalog | ✓ | ✓ | partial |
| Extent overflow | ✓ | ❌ | ❌ |
| Streaming reads | ✓ | ❌ | ❌ |
| Resource forks | ✓ | ❌ | ❌ |
| Extended attributes | ✓ | ❌ | ❌ |
| decmpfs compression | ✓ | ❌ | ❌ |
| Unicode names | ✓ | ✓ | ❌ |
| Generic `Read+Seek` | ✓ | ❌ | ❌ |
| Minimal dependencies\* | ✓ | ❌ | ❌ |

\* `byteorder`, `thiserror`, and [`cmpfs`](../cmpfs) for decmpfs — all pure
Rust. No FFI, no system libs.

> **Example:** macOS Kernel Debug Kit DMGs contain HFSX (case-sensitive HFS+) partitions.
> Most Rust HFS libraries can't read case-sensitive volumes — hfsplus handles both.

## Features

| | |
|---|---|
| **List directories** | Browse filesystem tree with names, sizes, timestamps |
| **Read files** | Extract file contents into memory or stream to a writer |
| **Streaming I/O** | `ForkReader` provides `Read+Seek` access without buffering |
| **File metadata** | BSD permissions, creation/modification dates, fork info |
| **Extended attributes** | Read one by name or list them all from the Attributes B-tree |
| **decmpfs** | Transparently compressed files read as their contents, not as nothing |
| **Recursive walk** | Walk entire filesystem tree with full paths |
| **Path resolution** | Navigate by Unix-style paths (`/Library/Extensions/foo.kext`) |

### Format Support

| Format | Support | Description |
|--------|:-------:|-------------|
| HFS+ | ✓ | Standard Mac OS Extended |
| HFSX | ✓ | Case-sensitive variant |
| Legacy HFS | ❌ | Classic Mac OS (pre-1998) |
| APFS | ❌ | Apple File System (2017+) |

## Quick Start

### Open and Browse

```rust
use hfsplus::HfsVolume;
use std::fs::File;
use std::io::BufReader;

let file = File::open("partition.raw")?;
let mut vol = HfsVolume::open(BufReader::new(file))?;

// List root directory
for entry in vol.list_directory("/")? {
    println!("{:?} {:>12} {}", entry.kind, entry.size, entry.name);
}
```

### Read a File

```rust
// Read into memory
let data = vol.read_file("/System/Library/Kernels/kernel")?;

// Or stream to a writer (low memory)
let mut out = File::create("kernel")?;
vol.read_file_to("/System/Library/Kernels/kernel", &mut out)?;
```

### Walk Entire Filesystem

```rust
for entry in vol.walk()? {
    if entry.entry.kind == hfsplus::EntryKind::File {
        println!("{}: {} bytes", entry.path, entry.entry.size);
    }
}
```

### Streaming File Access

```rust
use std::io::Read;

// Open file for random-access Read+Seek without loading into memory
let mut reader = vol.open_file("/large-file.bin")?;
let mut buf = [0u8; 4096];
let n = reader.read(&mut buf)?;
```

### File Metadata

```rust
let stat = vol.stat("/Library/Extensions/AppleHDA.kext")?;
println!("Size: {} bytes", stat.size);
println!("Owner: {}", stat.permissions.owner_id);
println!("Mode: {:o}", stat.permissions.mode);
println!("Resource fork: {} bytes", stat.resource_fork_size);
```

## Documentation

| | |
|---|---|
| [Format Specification](docs/FORMATS.md) | HFS+ volume header, B-tree, and catalog structures |
| [Implementation Notes](docs/IMPLEMENTATION.md) | Tricky parts: Unicode, extent overflow, node parsing |

## Example Output

Via `dpp-tool hfs` (which uses the hfsplus library internally):

```
$ dpp-tool hfs ls Kernel_Debug_Kit.dmg /

  Kind          Size  Name
  --------------------------------------------------------
  dir              -  .HFS+ Private Directory Data\r
  dir              -  .Trashes
  dir              -  Library
  dir              -  System
  dir              -  usr
              12288  .DS_Store

  1 file(s), 5 directory(ies)
```

```
$ dpp-tool hfs info Kernel_Debug_Kit.dmg

  HFS+ Volume: Kernel_Debug_Kit.dmg
  ════════════════════════════════════════════════════════════

  Volume Header
  ────────────────────────────────────────────────────────────
  Signature                HFSX (case-sensitive)
  Version                  5
  Block size               4096 bytes
  Total blocks             260,608
  Free blocks              6
  Files                    3,847
  Folders                  612
```

## Alternatives

| Crate | HFS+ | HFSX | B-tree | Extents | Streaming | Generic R | Platform |
|-------|:----:|:----:|:------:|:-------:|:---------:|:---------:|----------|
| **hfsplus** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | All |
| [hfsplus-rs](https://github.com/penguin359/hfsplus-rs) | ✓ | ❌ | ✓ | ❌ | ❌ | ❌ | All |
| [hfs-rs](https://sr.ht/~az1/hfs-rs/) | ❌ | ❌ | partial | ❌ | ❌ | ❌ | Unix |
| [hfsfuse](https://github.com/0x09/hfsfuse) | ✓ | ✓ | ✓ | ✓ | ✓ | N/A | Unix (C) |

**Choose hfsplus if you need:**
- Pure Rust with no FFI or system dependencies
- HFSX (case-sensitive) volume support
- Extent overflow file handling for large/fragmented files
- Generic `Read+Seek` interface (works with files, memory, network streams)
- Integration with the `dpp` pipeline for DMG → HFS+ workflows

**Choose hfsfuse if you need:**
- FUSE mounting (kernel-level filesystem access)
- Hard link support

## License

MIT
