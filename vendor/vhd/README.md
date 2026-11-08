# vhd-forensic

[![Crates.io: vhd-core](https://img.shields.io/crates/v/vhd-core.svg?label=vhd-core)](https://crates.io/crates/vhd-core)
[![Crates.io: vhd-forensic](https://img.shields.io/crates/v/vhd-forensic.svg?label=vhd-forensic)](https://crates.io/crates/vhd-forensic)
[![Docs.rs](https://img.shields.io/docsrs/vhd-core)](https://docs.rs/vhd-core)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/SecurityRonin/vhd-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/vhd-forensic/actions/workflows/ci.yml)
[![unsafe: forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/SecurityRonin/vhd-forensic)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

**Read and audit legacy VHD (Virtual PC / Hyper-V Gen-1) disk images in pure Rust — a hardened `Read + Seek` container reader plus a footer integrity analyzer for DFIR.**

This workspace ships two crates: **`vhd-core`** — the MS-VHD container reader (Fixed and Dynamic disks), exposing a `Read + Seek` view over the virtual sector stream (published as `vhd-core`, imported as `vhd`); and **`vhd-forensic`** — the integrity analyzer that parses the footer *raw* (which the reader validates-and-discards) and reports tamper / structural anomalies as `forensicnomicon::report::Finding`. Zero unsafe code, no C bindings, no external tools.

```toml
[dependencies]
vhd-core = "0.2"       # reader — imported as `vhd`
vhd-forensic = "0.2"   # analyzer — graded footer findings
```

## Usage

### Audit a VHD footer for tampering

```rust
use vhd_forensic::audit;
use forensicnomicon::report::Observation;

for anomaly in audit(&image_bytes) {
    let finding = anomaly.to_finding(source);   // canonical forensicnomicon Finding
    println!("{} — {}", finding.code, finding.note);
}
```

### Open a VHD and read the virtual sector stream

```rust
use std::io::Read;
use vhd::VhdReader;

let reader = VhdReader::open(std::path::Path::new("disk.vhd"))?;
println!("current size:  {} bytes", reader.virtual_disk_size()); // CurrentSize @48 (readable)
println!("original size: {} bytes", reader.original_size());     // OriginalSize @40 (creation)
// original_size() != virtual_disk_size()  ⇒  the disk was resized after creation
```

`VhdReader::open_reader` accepts any `Read + Seek + Send + Sync`, so a VHD stored
inside an archive can be read without extracting it to a temp file.

## Forensic analysis — `vhd-forensic`

`audit(&[u8])` parses the trailing 512-byte footer at the documented MS-VHD offsets
and returns typed anomalies; each implements `Observation`, so `.to_finding(source)`
yields a graded finding.

| Code | Severity | Meaning |
|---|---|---|
| `VHD-FOOTER-TRUNCATED` | High | file smaller than the 512-byte footer |
| `VHD-FOOTER-COOKIE-INVALID` | High | cookie != `conectix` |
| `VHD-FOOTER-CHECKSUM-MISMATCH` | High | one's-complement checksum tamper / corruption |
| `VHD-FORMAT-VERSION-UNEXPECTED` | Medium | format version != 1.0 |
| `VHD-DISK-TYPE-UNKNOWN` | Medium | disk type not Fixed / Dynamic / Differencing |
| `VHD-DATA-OFFSET-INCONSISTENT` | Medium | `DataOffset` inconsistent with the disk type |
| `VHD-SAVED-STATE` | Low | image captured in a saved (suspended) state |
| `VHD-SIZE-RESIZED` | Low | `OriginalSize`@40 != `CurrentSize`@48 — disk resized after creation (History) |

## Trust but verify

- **Panic-free** — `unsafe_code = forbid`, `clippy::unwrap_used`/`expect_used = deny`,
  bounded readers, and `checked_add`/`checked_mul` on every offset/length from the image.
- **Fuzzed** — `fuzz_open` (reader) and `fuzz_audit` (analyzer) over arbitrary bytes;
  local smoke ran 8.1 M / 52 K executions with no panic.
- **Validated against real qemu-img images with an independent oracle** — including the
  `current_size` offset bug caught by spec research and fixed against qemu-img's own
  reported size. See [Validation](https://securityronin.github.io/vhd-forensic/validation/).

## Supported disk types

| Type | Read | Notes |
|---|---|---|
| Fixed | ✅ | raw sector data + trailing footer |
| Dynamic | ✅ | BAT-addressed sparse blocks |
| Differencing | — | rejected (parent-locator resolution out of scope) |

---

[Privacy Policy](https://securityronin.github.io/vhd-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/vhd-forensic/terms/) · © 2026 Security Ronin Ltd
