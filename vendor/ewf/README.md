# ewf

[![Crates.io](https://img.shields.io/crates/v/ewf.svg)](https://crates.io/crates/ewf)
[![docs.rs](https://img.shields.io/docsrs/ewf)](https://docs.rs/ewf)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![CI](https://github.com/SecurityRonin/ewf/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/ewf/actions)
[![Coverage](https://img.shields.io/badge/coverage-99.86%25-brightgreen)](docs/corpus-validation.md)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

Pure Rust reader for Expert Witness Format (E01/EWF) forensic disk images. Zero GPL dependencies. Includes a CLI and MCP server for AI-assisted forensic analysis.

## Install

### CLI (pre-built binary)

```bash
# macOS (Homebrew)
brew install SecurityRonin/tap/ewf

# macOS / Linux (install script)
curl -sSL https://raw.githubusercontent.com/SecurityRonin/ewf/main/install.sh | bash

# Windows (winget)
winget install SecurityRonin.ewf

# Debian / Ubuntu
sudo dpkg -i ewf-cli_*.deb

# From source (requires Rust)
cargo install ewf-cli
```

### Rust library

```toml
[dependencies]
ewf = "0.2"
```

## CLI usage

```bash
ewf info image.E01              # Metadata, hashes, case info
ewf verify image.E01            # Full-media MD5/SHA-1 verification
ewf read image.E01 -o 510 -l 16 # Hex dump at offset
ewf sections image.E01          # List internal EWF sections
ewf search image.E01 55aa       # Search for byte pattern
ewf extract image.E01 -o 0 -l 512 -O mbr.bin  # Extract bytes to file
ewf info image.E01 --json       # JSON output for scripting
```

## MCP server

The `ewf mcp` subcommand starts an [MCP](https://modelcontextprotocol.io/) server for AI-assisted forensic image inspection over JSON-RPC stdio.

| Tool | Description |
|------|-------------|
| `ewf_info` | Image metadata, geometry, stored hashes, acquisition errors |
| `ewf_verify` | Full-media hash verification (MD5 + SHA-1) |
| `ewf_read_sectors` | Read hex bytes at any offset |
| `ewf_list_sections` | List all section descriptors across segments |
| `ewf_search` | Byte-pattern search with hex input |
| `ewf_extract` | Extract byte range to file |

### Register with Claude Code

```bash
claude mcp add ewf -- ewf mcp
```

### Claude Desktop configuration

```json
{
  "mcpServers": {
    "ewf": {
      "command": "ewf",
      "args": ["mcp"]
    }
  }
}
```

## Library quick start

```rust
use std::io::{Read, Seek, SeekFrom};

let mut reader = ewf::EwfReader::open("disk.E01")?;

// Read the first sector
let mut mbr = [0u8; 512];
reader.read_exact(&mut mbr)?;

// Seek anywhere — O(1) via flat chunk index
reader.seek(SeekFrom::Start(1_048_576))?;
```

`EwfReader` implements `Read + Seek`, so it plugs directly into crates like [`ntfs`](https://crates.io/crates/ntfs), [`fatfs`](https://crates.io/crates/fatfs), or anything expecting a seekable stream.

## Library features

- **EWF v1 format** — reads images from EnCase, FTK Imager, Guymager, ewfacquire, etc.
- **EWF v2 format (Ex01/Lx01)** — reads EnCase 7+ images with format auto-detection
- **L01 logical evidence files** — in progress; see the format table below
- **Multi-segment** — auto-discovers `.E01` through `.EZZ` (v1) and `.Ex01` through `.EzZZ` (v2)
- **zlib decompression** with LRU caching (configurable, default 100 chunks ~ 3.2 MB)
- **O(1) seeking** — flat chunk table indexed by `offset / chunk_size`
- **Hash verification** — `verify()` streams all media data through MD5/SHA-1 and compares against stored hashes
- **Stored hashes** — reads MD5 and SHA-1 from hash/digest sections (v1) and Md5Hash/Sha1Hash sections (v2)
- **Case metadata** — parses case number, examiner, description, notes, acquisition dates from header (v1) and CaseData (v2) sections
- **Acquisition errors** — extracts read-error entries from error2 sections
- **table + table2 resilience** — handles both section types, deduplicates correctly
- **DoS-safe** — guards against malformed images with absurd table entry counts
- **Apache-2.0 licensed** — no GPL, safe for proprietary DFIR tooling

## Library API examples

### Verify image integrity

```rust
let mut reader = ewf::EwfReader::open("case001.E01")?;
let result = reader.verify()?;
if let Some(true) = result.md5_match {
    println!("MD5 verified: {:02x?}", result.computed_md5);
}
```

### Read case metadata

```rust
let reader = ewf::EwfReader::open("case001.E01")?;
let meta = reader.metadata();
println!("Case: {:?}", meta.case_number);
println!("Examiner: {:?}", meta.examiner);
println!("Software: {:?}", meta.acquiry_software);
```

### Check stored hashes

```rust
let reader = ewf::EwfReader::open("case001.E01")?;
let hashes = reader.stored_hashes();
if let Some(md5) = hashes.md5 {
    println!("Stored MD5: {:02x?}", md5);
}
```

### Tune cache for large images

```rust
// 1000 chunks ~ 32 MB cache — useful for sequential scans
let mut reader = ewf::EwfReader::open_with_cache_size("case001.E01", 1000)?;
```

### With the ntfs crate

```rust
use ewf::EwfReader;
use ntfs::Ntfs;

let mut reader = EwfReader::open("disk.E01")?;
// Seek to NTFS partition offset, then:
let ntfs = Ntfs::new(&mut reader)?;
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `verify` | Yes | Enables `verify()` method (adds `md-5` and `sha-1` dependencies) |

To disable hash verification and reduce dependencies:

```toml
[dependencies]
ewf = { version = "0.2", default-features = false }
```

## Format support

| Format | Status |
|--------|--------|
| E01 (EWF v1) | Supported |
| E01 multi-segment (.E01-.EZZ) | Supported |
| Ex01 (EWF v2) | Supported |
| L01 (logical evidence, v1) | **Not yet** — rejected at the LVF signature |
| Lx01 (logical evidence, v2) | **Not yet** — EWF v2 logical variant |
| S01 (SMART) | Not yet |

## Testing

- **127 tests** (92 unit + 27 e2e + 8 validation) with **99.86% line coverage** (694/695 lines)
- Full-media MD5 comparison against libewf and The Sleuth Kit confirms bit-identical output across 6 public forensic images (303+ GiB of media)
- Test images sourced from [Digital Corpora](https://digitalcorpora.org/) and [The Evidence Locker](https://theevidencelocker.github.io/) (Kevin Pagano)
- Three small images are committed as test fixtures and run in CI

See [docs/corpus-validation.md](docs/corpus-validation.md) for detailed results, image sources, and reproduction steps.

## Acknowledgments

Architecture informed by [Velocidex/go-ewf](https://github.com/Velocidex/go-ewf) (Apache-2.0).

## Related

**ewf** reads and decodes E01 images — it gives you a `Read + Seek` stream over the evidence data. It does not tell you whether the image has been tampered with or is structurally sound.

[**ewf-forensic**](https://github.com/SecurityRonin/ewf-forensic) is the auditor layer: it reads the raw E01 bytes and reports *what is wrong* — signature forgery, broken section chains, Adler-32 descriptor corruption, out-of-bounds table entries, and MD5 hash mismatches verified via per-chunk zlib decompression. It also repairs Adler-32 errors in-memory without touching your original file. Use `ewf` to read; use `ewf-forensic` to verify and triage.

### Container readers

When evidence arrives in a different container format, these crates provide the same `Read + Seek` interface:

| Crate | Format | Notes |
|-------|--------|-------|
| [`aff4`](https://github.com/SecurityRonin/aff4) | AFF4 v1 | Evimetry / aff4-imager forensic disk images with Map streams |
| [`vmdk`](https://github.com/SecurityRonin/vmdk) | VMware VMDK | Monolithic sparse disk images from VMware Workstation / ESXi |
| [`vhdx`](https://github.com/SecurityRonin/vhdx) | Microsoft VHDX | Hyper-V, Windows 8+, WSL2, Azure disk container |
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
| [`vhdx-forensic`](https://github.com/SecurityRonin/vhdx-forensic) | VHDX | Forensic integrity analyser and in-memory repair tool for VHDX containers |

## License

Apache-2.0

---

[Privacy Policy](https://securityronin.github.io/ewf/privacy/) · [Terms of Service](https://securityronin.github.io/ewf/terms/) · © 2026 Security Ronin Ltd
