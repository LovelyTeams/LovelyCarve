# vhdx-forensic

[![Crates.io: vhdx-core](https://img.shields.io/crates/v/vhdx-core.svg?label=vhdx-core)](https://crates.io/crates/vhdx-core)
[![Crates.io: vhdx-forensic](https://img.shields.io/crates/v/vhdx-forensic.svg?label=vhdx-forensic)](https://crates.io/crates/vhdx-forensic)
[![Docs.rs](https://img.shields.io/docsrs/vhdx-core)](https://docs.rs/vhdx-core)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/SecurityRonin/vhdx-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/vhdx-forensic/actions/workflows/ci.yml)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

**Read and audit Hyper-V VHDX disk images in pure Rust — a hardened `Read + Seek` container reader plus a 63-code structural integrity analyzer for DFIR.**

This workspace ships two crates: **`vhdx-core`** — the VHDX/Hyper-V virtual-disk container reader (dynamic, fixed, differencing, automatic dirty-log recovery), exposing a `Read + Seek` view over the virtual sector stream (published as `vhdx-core`, imported as `vhdx`); and **`vhdx-forensic`** — the integrity analyzer that audits the raw bytes for tampering, corruption, and anti-forensic GUID/log wiping, emitting `forensicnomicon::report::Finding` plus optional in-memory CRC repair. Zero unsafe code, no C bindings, no external tools.

```toml
[dependencies]
vhdx-core = "0.2"       # reader — imported as `vhdx`
vhdx-forensic = "0.2"   # analyzer — graded structural findings
```

```rust
use vhdx_forensic::{anomalies_at_least, Severity, VhdxIntegrity};

let image = std::fs::read("disk.vhdx")?;
let anomalies = VhdxIntegrity::new(&image).analyse();
for a in anomalies_at_least(&anomalies, Severity::Error) {
    println!("[{:?}] {}", a.severity(), a.forensic_significance());
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Usage

### Open a VHDX and read sectors

```rust
use vhdx::VhdxReader;
use std::io::{Read, Seek, SeekFrom};

let mut reader = VhdxReader::open("disk.vhdx")?;

println!("Virtual disk size: {} bytes", reader.virtual_disk_size());
println!("Logical sector size: {} bytes", reader.logical_sector_size());

// Read the first sector
let mut sector = vec![0u8; reader.logical_sector_size() as usize];
reader.seek(SeekFrom::Start(0))?;
reader.read_exact(&mut sector)?;
```

### Pass to a filesystem crate

`VhdxReader` implements `Read + Seek`, so it drops directly into any crate that accepts a reader:

```rust
use vhdx::VhdxReader;

let reader = VhdxReader::open("disk.vhdx")?;
// e.g. ext4fs_forensic::Filesystem::open(reader)?;
```

### Read from an in-memory buffer

```rust
use vhdx::VhdxReader;

let data: Vec<u8> = std::fs::read("disk.vhdx")?;
let reader = VhdxReader::from_bytes(data)?;
```

### Open a differencing (child) disk with its parent

```rust
use vhdx::VhdxReader;

let parent = VhdxReader::from_bytes(std::fs::read("base.vhdx")?)?;
let reader = VhdxReader::from_bytes_with_parent(std::fs::read("child.vhdx")?, parent)?;
// Reads absent blocks in the child are transparently served from parent.
```

---

## CLI

The `vhdx-cli` crate (included in this workspace) provides a `vhdx info` command:

```
$ vhdx info disk.vhdx
File:              disk.vhdx
Format:            VHDX v1 (dynamic)
Virtual disk size: 16,777,216 bytes (16.00 MiB)
Logical sectors:   512 bytes
```

---

## Supported formats

| Format | Supported |
|--------|:---------:|
| VHDX Version 1 (Windows 8 / Server 2012+) | ✓ |
| Dynamic disks (sparse, BAT-addressed) | ✓ |
| Fixed disks (pre-allocated) | ✓ |
| Differencing disks (single-level parent chain) | ✓ |
| Log replay (dirty-log recovery) | ✓ |

Read-only. Differencing disks require the parent image to be supplied via `VhdxReader::from_bytes_with_parent`. Log replay is applied automatically on open when the active header carries a non-zero LogGuid.

---

## Forensic analysis — `vhdx-forensic`

`VhdxIntegrity::new(&bytes).analyse()` walks the raw container across six phases — container/magic, CRC integrity, header semantics, region layout, metadata, and BAT/data-block analysis — and returns a `Vec<VhdxIntegrityAnomaly>`. It works on raw bytes and does not require a fully valid structure. Each variant carries a stable `code` string, a graded `severity()`, a `forensic_significance()` narrative, and `mitre_techniques()`. There are **63** distinct anomaly codes; a representative sample:

| Code | Severity | Meaning |
|------|----------|---------|
| `VHDX-BAD-MAGIC` | Critical | File does not start with the `vhdxfile` signature |
| `VHDX-BOTH-HEADER-COPIES-INVALID` | Critical | Both header copies fail CRC32C — header recovery impossible |
| `VHDX-HEADER-CHECKSUM-MISMATCH` | Error | A header copy's stored CRC32C does not match the computed value |
| `VHDX-REGIONS-OVERLAP` | Error | Two region-table entries claim overlapping container bytes |
| `VHDX-LOG-ENTRY-CRC-MISMATCH` | Error | A log entry's CRC fails — replay would corrupt the image |
| `VHDX-BAT-ENTRY-BEYOND-CONTAINER` | Error | A BAT entry points past the end of the file |
| `VHDX-FILE-WRITE-GUID-ALL-ZEROS` | Warning | FileWriteGuid wiped — consistent with anti-forensic tampering |
| `VHDX-GHOST-DATA-IN-ABSENT-BLOCK` | Warning | Non-zero payload in a block marked absent — residual/hidden data |
| `VHDX-DIFFERENCING-DISK` | Warning | Image is a child disk requiring a parent to read |
| `VHDX-DIRTY-LOG` | Info | Active log present — image was not cleanly closed |

Findings are observations, never legal conclusions — MITRE mappings are surfaced as "consistent with," for the analyst to weigh. `VhdxRepair::new(bytes).attempt_repair()` rebuilds header and region-table CRC32C checksums from a valid peer copy in memory; it never alters payload data.

---

## Trust but verify

- **Panic-free on hostile input.** No `.unwrap()`/`.expect()`/`panic!` or unchecked indexing in production code (`unwrap_used`/`expect_used` are hard `deny` lints). All length, offset, and count fields are bounds-checked before any arithmetic; BAT addressing uses `checked_mul`/`checked_add`.
- **Fuzzed.** A `cargo-fuzz` workspace exercises the parse and analyse paths; the invariant is "must not panic."
- **Validated against real artifacts.** Tested against independently produced images from the [log2timeline/dfvfs](https://github.com/log2timeline/dfvfs) corpus and QEMU v11.0.0 output. The decoded byte stream is verified **byte-identical to `qemu-img convert -O raw`** (an independent C codebase), and virtual disk sizes are cross-validated against `qemu-img info`. Detection is proven by injecting corruptions at spec-mandated MS-VHDX §2.0 byte offsets into real QEMU images. See the [validation report](https://securityronin.github.io/vhdx-forensic/validation/) ([source](docs/validation.md)).

---

## Related crates

### Container readers

| Crate | Format | Notes |
|-------|--------|-------|
| [`ewf`](https://github.com/SecurityRonin/ewf) | E01 / EWF / Ex01 | Dominant professional forensic acquisition format |
| [`aff4`](https://github.com/SecurityRonin/aff4) | AFF4 v1 | Evimetry / aff4-imager forensic disk images with Map streams |
| [`vmdk`](https://github.com/SecurityRonin/vmdk) | VMware VMDK | Monolithic sparse disk images from VMware Workstation / ESXi |
| [`vhd`](https://github.com/SecurityRonin/vhd) | Legacy VHD | Virtual PC / Hyper-V Generation-1 fixed and dynamic disk images |
| [`qcow2`](https://github.com/SecurityRonin/qcow2) | QCOW2 v2/v3 | QEMU / KVM / libvirt disk images |
| [`ufed`](https://github.com/SecurityRonin/ufed) | Cellebrite UFED | Physical mobile device dumps with UFD XML segment mapping |
| [`dd`](https://github.com/SecurityRonin/dd) | Raw / flat / gz | dd, dcfldd, and gzip-wrapped raw images |
| [`iso9660-forensic`](https://github.com/SecurityRonin/iso9660-forensic) | ISO 9660 | Optical disc images: multi-session, UDF bridge, Rock Ridge, Joliet, El Torito |
| [`dmg`](https://github.com/SecurityRonin/dmg) | Apple DMG / UDIF | macOS disk images with koly trailer, mish block tables, zlib decompression |
| [`dar`](https://github.com/SecurityRonin/dar) | DAR archive | Disk ARchiver archives with catalog index and CRC32 validation |

### Forensic analysers

| Crate | Format | Notes |
|-------|--------|-------|
| [`vhdx-forensic`](https://github.com/SecurityRonin/vhdx-forensic) | VHDX | Forensic integrity analyser and in-memory repair tool built on this crate |
| [`ewf-forensic`](https://github.com/SecurityRonin/ewf-forensic) | E01 | Structural integrity audit, Adler-32 / MD5 hash verification, and in-memory repair |

---

[Privacy Policy](https://securityronin.github.io/vhdx-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/vhdx-forensic/terms/) · © 2026 Security Ronin Ltd
