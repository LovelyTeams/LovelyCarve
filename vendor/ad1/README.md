# ad1-forensic

[![Crates.io](https://img.shields.io/crates/v/ad1-core.svg)](https://crates.io/crates/ad1-core)
[![Docs.rs](https://img.shields.io/docsrs/ad1-core)](https://docs.rs/ad1-core)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/SecurityRonin/ad1-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/ad1-forensic/actions/workflows/ci.yml)
[![Fuzz](https://github.com/SecurityRonin/ad1-forensic/actions/workflows/fuzz.yml/badge.svg)](https://github.com/SecurityRonin/ad1-forensic/actions/workflows/fuzz.yml)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/h4x0r)

**Open AccessData AD1 logical images in Rust — the format TSK, libewf, and
BitCurator can't read. List files, extract bytes, and verify the stored
per-file hashes.**

FTK Imager's AD1 ("Custom Content Image") is a *logical* evidence container — a
tree of files with metadata and zlib-compressed data, not a disk image. Nothing
in the open-source disk-forensics stack parses it. `ad1-core` does, with no
`unsafe` and no C dependencies.

## 30 seconds to a file listing

```toml
# Cargo.toml
[dependencies]
ad1-core = "0.1"
```

```rust
use ad1::Ad1Reader;

let img = Ad1Reader::open(std::path::Path::new("evidence.ad1"))?;
for entry in img.entries() {
    let kind = if entry.is_dir { "DIR " } else { "FILE" };
    println!("{kind} {:>10}  {}", entry.size, entry.path);
}

// Read a file's bytes (only the overlapping zlib chunks are inflated):
if let Some(f) = img.entries().iter().find(|e| !e.is_dir) {
    let mut buf = vec![0u8; f.size as usize];
    let n = img.read_at(f, 0, &mut buf)?;
    println!("read {n} bytes of {}", f.path);
}
# Ok::<(), ad1::Ad1Error>(())
```

`Ad1Reader::open` discovers split segments (`.ad1`, `.ad2`, …), parses the file
tree, and exposes each entry's path, size, stored MD5/SHA1, and timestamps.
`read_at` is positioned — it inflates only the chunks your range overlaps, not
the whole file.

## Tamper detection

`ad1-forensic` recomputes each file's hash and compares it to the value stored in
the image, emitting graded [`forensicnomicon`](https://crates.io/crates/forensicnomicon)
findings:

```rust
for finding in ad1_forensic::audit(std::path::Path::new("evidence.ad1")) {
    println!("[{:?}] {} — {}", finding.severity, finding.code, finding.note);
}
```

| Code | Meaning |
|---|---|
| `AD1-HASH-MISMATCH` | stored hash ≠ recomputed hash (tamper signal) |
| `AD1-ENCRYPTED` | `ADCRYPT` image — content not verifiable |
| `AD1-SEGMENT-MISSING` | a declared `.adN` segment is absent |
| `AD1-SIZE-LIE` | fewer bytes decompressed than declared |
| `AD1-UNREADABLE` | the structure could not be parsed |

## Built for untrusted input

- **No `unsafe`** (`#![forbid(unsafe_code)]`), **no C** (pure-Rust zlib + hashes).
- **Panic-free on malformed input** — bounds-checked reads, capped allocations,
  cycle guards. Two `cargo-fuzz` targets back this up.
- **Encrypted (`ADCRYPT`) images are refused**, never decoded to garbage.

See [docs/format.md](docs/format.md) for the on-disk layout and
[docs/validation.md](docs/validation.md) for how correctness is established.

---

[Privacy Policy](https://securityronin.github.io/ad1-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/ad1-forensic/terms/) · © 2026 Security Ronin Ltd
