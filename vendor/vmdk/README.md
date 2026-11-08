# vmdk-forensic

[![vmdk-core](https://img.shields.io/crates/v/vmdk-core.svg?label=vmdk-core)](https://crates.io/crates/vmdk-core)
[![vmdk-forensic](https://img.shields.io/crates/v/vmdk-forensic.svg?label=vmdk-forensic)](https://crates.io/crates/vmdk-forensic)
[![docs.rs](https://img.shields.io/docsrs/vmdk-core)](https://docs.rs/vmdk-core)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![CI](https://github.com/SecurityRonin/vmdk-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/vmdk-forensic/actions)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

**Read VMware VMDK disk images others give up on, then audit them for tampering.** Two crates in one workspace: the [`vmdk-core`](https://crates.io/crates/vmdk-core) reader (imported as `vmdk`) presents the virtual disk as a plain `Read + Seek` byte stream — and **recovers data from a damaged disk through the redundant grain directory that `qemu-img` and `libvmdk` throw away** — while the [`vmdk-forensic`](https://crates.io/crates/vmdk-forensic) analyzer turns the structure into severity-graded, evidence-grade findings.

## The two crates

| Crate | Role | Import | `cargo add` |
|---|---|---|---|
| [`vmdk-core`](https://crates.io/crates/vmdk-core) | Read-only VMDK reader: decoded virtual-sector `Read + Seek`, RGD-fallback recovery, `ddb` provenance | `use vmdk::…` | `cargo add vmdk-core` |
| [`vmdk-forensic`](https://crates.io/crates/vmdk-forensic) | Integrity analyzer → canonical `forensicnomicon::report::Finding` (re-exports the reader) | `use vmdk_forensic::…` | `cargo add vmdk-forensic` |

## Command-line tool

One command — no subcommand — tells you what a VMDK is and whether it's sound:

```console
$ vmdk disk.vmdk
```

```text
File:              disk.vmdk
Format:            VMDK v1 (monolithicSparse)
Virtual disk size: 4,194,304 bytes (4.00 MiB)
Sectors:           8,192
CID:               dc80b6c7

Provenance:
  Content ID:       dc80b6c7
  Adapter:          ide
  Geometry (C/H/S): 8/16/63
  HW version:       4
  Clean shutdown:   yes
  Redundant GD:     present

Integrity: OK (no anomalies detected)
```

The default `examine` view folds identity, `ddb` provenance, companion-file
discovery, and the canonical forensic findings — RGD mismatch, dangling grain
tables, unclean shutdown, FTP-mangled headers — into one answer, and **exits
non-zero on any High-severity finding**, so it drops straight into a triage
pipeline. Three flags extend it:

- `--fingerprint` (`--fp`) — append a labelled virtual-disk SHA-256 + MD5
- `--json` — machine-readable output for a SIEM / pipeline
- `--recover` — read through a damaged primary grain directory (RGD fallback)

```console
$ vmdk --fingerprint --json disk.vmdk | jq '.findings, .fingerprint'
```

Three more verbs cover extraction and comparison:

- `dump` — raw virtual-disk bytes to stdout or a file (`-o`), a byte range
  (`--offset` / `--length`), a hex view (`--hex`), or a sparse-only
  reconstruction (`--allocated-only`). `dump --recover` resolves reads through
  the redundant grain directory, extracting data behind corruption that
  `qemu-img` fails on — pipe it into a filesystem tool (NTFS, ext4, …) to read
  the guest's files.
- `map` — allocated (non-sparse) grain ranges as `start_lba,sector_count`
- `diff` — byte-by-byte comparison of two VMDK virtual disks

## Rust library

The reader is published as `vmdk-core` and imported as `vmdk`; the analyzer is `vmdk-forensic`:

```bash
cargo add vmdk-core       # the reader (imported as `vmdk`)
cargo add vmdk-forensic   # the analyzer (re-exports the reader)
```

## Quick start

```rust
use vmdk::VmdkReader;
use std::io::{Read, Seek, SeekFrom};

// Open any `Read + Seek` source — a File, a Cursor, another container reader.
let mut disk = VmdkReader::open(std::fs::File::open("disk.vmdk")?)?;

println!("virtual size: {} bytes", disk.virtual_disk_size());

// Read decoded virtual sectors like any byte stream — sparse/compressed grains
// are decompressed and zero-filled transparently.
let mut first_mib = vec![0u8; 1 << 20];
disk.seek(SeekFrom::Start(0))?;
disk.read_exact(&mut first_mib)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

For path-based images with companion files — `monolithicFlat`, the
`twoGbMaxExtent*` split sets, raw-device maps — use `VmdkFileReader::open_path`,
which locates and opens the extent files for you. For snapshot/delta trees, use
`VmdkChainReader::open`, which layers a delta on its parent chain.

## What makes this different from `qemu-img` and `libvmdk`

Most VMDK readers answer one question: "give me the bytes." `vmdk` answers the
questions a digital forensics examiner actually needs — and reads disks the
others give up on:

| Capability | qemu-img / libvmdk | vmdk |
|---|---|---|
| Sparse / streamOptimized / flat read | ✅ | ✅ |
| COWD (`vmfsSparse`/`vmfsThin`) + seSparse (VMFS6) | partial | ✅ |
| Snapshot / delta chain traversal | ✅ | ✅ |
| **Recover data behind a damaged primary GD** (redundant-GD fallback) | ✗ | ✅ |
| **Recover an individual lost grain-table entry** from the redundant copy | ✗ | ✅ |
| Redundant-GD validation (grain-table *contents*, not pointers) | ✗ | ✅ via `vmdk-forensic` |
| Structural integrity scan (dangling GD/GT/grain pointers) | ✗ | ✅ via `vmdk-forensic` |
| `ddb.*` disk database (adapter, geometry, UUID, tools/HW version) | discarded | ✅ |
| Header provenance — unclean-shutdown flag, FTP-ASCII-mangling check | ✗ | ✅ via `vmdk-forensic` |
| Change Block Tracking (`-ctk`) reference | ✗ | ✅ |
| `longContentID` resolution (the `CID == 0xFFFFFFFE` sentinel) | ✗ | ✅ |
| Raw Device Mapping (`VMFSRDM`) extent enumeration | ✗ | ✅ |
| Streaming SHA-256 + MD5 of the virtual disk | ✗ | ✅ |
| Adversarial-input hardening + fuzz testing | ✗ | ✅ |
| Pure Rust, zero `unsafe`, no C library | ✗ | ✅ |

## Formats

Every VMDK `createType` and extent type in the VMware Virtual Disk Format spec
(cross-checked against QEMU `block/vmdk.c` and `libvmdk`):

| `createType` | Notes |
|---|---|
| `monolithicSparse`, `streamOptimized` | header v1/v2/v3; DEFLATE grains; `GD_AT_END` footer |
| `monolithicFlat`, `vmfs`, `vmfsPreallocated`, `vmfsEagerZeroedThick` | preallocated flat extents |
| `twoGbMaxExtentSparse`, `twoGbMaxExtentFlat` | split 2 GB extent sets |
| `vmfsSparse`, `vmfsThin` | ESXi COWD copy-on-write sparse |
| `seSparse` | vSphere 6.5+ space-efficient sparse (nibble-typed, bit-rotated grains) |
| `vmfsRaw`, `vmfsRawDeviceMap`, `vmfsPassthroughRawDeviceMap`, `fullDevice`, `partitionedDevice` | device / raw-LUN maps |
| `custom` | arbitrary extent mix, routed by extent type |

Extent types: `FLAT`, `VMFS`, `VMFSRAW`, `VMFSRDM`, `ZERO`, `SPARSE`,
`VMFSSPARSE`, `SESPARSE`; access `RW` / `RDONLY` / `NOACCESS`. `ZERO` and
`NOACCESS` regions read as zeros without touching disk.

## Forensic recovery

VMware writes the grain tables **twice** — the grain directory (GD) and a
redundant copy (RGD) point to separate physical copies. `qemu-img` and `libvmdk`
read only the primary and fail when it is damaged. `vmdk` uses the redundant copy
to keep reading:

```rust
use vmdk::VmdkReader;
use std::io::Read;

let mut disk = VmdkReader::open(std::fs::File::open("damaged.vmdk")?)?;

// Opt in to recovery, then read normally — damaged pointers resolve through the RGD.
disk.enable_rgd_fallback();
let mut buf = vec![0u8; 1 << 20];
let _ = disk.read(&mut buf);
println!("recovered {} grain(s) via the RGD", disk.rgd_recovery_count());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Recovery is opt-in and never changes a healthy read; without it a dangling pointer
simply errors (the safe default). To *triage* a damaged image first — how much of
the primary grain directory the RGD can recover, plus tamper/anomaly detection — use
the companion [`vmdk-forensic`](https://crates.io/crates/vmdk-forensic) crate.

## Forensic metadata

The text descriptor carries provenance that other readers parse and then throw
away. `vmdk` surfaces all of it:

```rust
use vmdk::VmdkReader;

let mut disk = VmdkReader::open(std::fs::File::open("disk.vmdk")?)?;

let ddb = disk.disk_database();                 // ddb.* disk database
println!("adapter:   {:?}", ddb.adapter_type);  // ide / lsilogic / pvscsi …
println!("geometry:  {:?}", ddb.geometry);      // CHS cylinders/heads/sectors
println!("disk UUID: {:?}", ddb.uuid);
println!("HW / tools: {:?} / {:?}", ddb.virtual_hw_version, ddb.tools_version);

println!("CBT file:   {:?}", disk.change_track_path());       // -ctk.vmdk reference
println!("content ID: {}",  disk.effective_content_id());     // resolves longContentID
# Ok::<(), Box<dyn std::error::Error>>(())
```

Header provenance (unclean-shutdown flag, FTP-ASCII-mangling check) and the integrity
/ anomaly analysis live in the [`vmdk-forensic`](https://crates.io/crates/vmdk-forensic)
companion crate — see [Related](#related).

## API highlights

| Method | Purpose |
|---|---|
| `VmdkReader::open(reader)` | open any `Read + Seek` source |
| `VmdkFileReader::open_path(path)` | open path-based images (flat / multi-extent / device maps) |
| `VmdkChainReader::open(path)` | layer a delta on its parent snapshot chain |
| `read` / `seek` (`std::io`) | decoded virtual-sector byte stream |
| `info()` → `VmdkInfo` | version, CID, geometry, compression, descriptor, disk database |
| `is_allocated(lba)` / `iter_allocated_grains()` | sparse-map queries |
| `hash()` → `VmdkDigest` | streaming SHA-256 + MD5 of the virtual disk |
| `validate_rgd()` / `check_integrity()` | redundant-GD + structural integrity |
| `grain_directory_recovery()` / `enable_rgd_fallback()` / `rgd_recovery_count()` | RGD recovery |
| `disk_database()` / `header_provenance()` / `change_track_path()` / `effective_content_id()` | forensic metadata |

`serde` derives on the public report types are available behind the `serde` feature.

## Security

`vmdk` is built to run on untrusted, potentially crafted disk images:

- **No panics on malicious input** — every allocation derived from header fields
  is bounds-checked; reads are clamped; compressed-grain sizes are capped.
- **Allocation-amplification hardened** — `numGTEsPerGT` is capped at the spec
  value (512), matching QEMU, so a crafted header can't drive a multi-gigabyte
  grain-table allocation.
- **Zero `unsafe`** — `unsafe_code = "forbid"` workspace-wide; no C dependency.
- **Fuzz-tested** — four `cargo fuzz` targets cover the open path, the full
  read surface, the RGD recovery paths, and the forensic analysis pipeline; run
  in CI on every change and deeper on a schedule.

Hardened further in **0.6.0** (all on the untrusted-input path):

- **Descriptor path-traversal sandboxed** — extent and `parentFileNameHint`
  paths are confined to the image directory; an absolute or `..`-climbing path
  is refused, so a crafted descriptor can't read arbitrary host files.
- **Decompression-bomb bounded** — a compressed grain is decoded only up to its
  grain size and refused if it expands further, so a few-KB payload can't
  amplify into a multi-megabyte allocation.
- **Snapshot-chain reads grain-clamped** — a sparse grain can no longer
  zero-mask an allocated grain that follows it within the same read.
- **Mixed-extent descriptors rejected** — a `custom` image listing both flat and
  sparse extents fails loud instead of silently dropping the sparse ones.

```bash
# Requires nightly Rust and cargo-fuzz
rustup install nightly
cargo install cargo-fuzz

cargo +nightly fuzz run fuzz_open
cargo +nightly fuzz run fuzz_read
cargo +nightly fuzz run fuzz_recover
cargo +nightly fuzz run fuzz_forensic
```

## Testing

280+ tests (unit + integration) covering every public API, every format branch,
the recovery paths, and adversarial inputs. The real VMware-written
`monolithicSparse` corpus image (`dfvfs_ext2.vmdk`, from
[log2timeline/dfvfs](https://github.com/log2timeline/dfvfs), Apache-2.0) is read
**byte-for-byte against `qemu-img convert -O raw`**, and COWD / seSparse output is
cross-validated against `qemu-img`'s independent reader — the synthetic fixtures
and the reader cannot share a blind spot. Coverage is enforced in CI. Which oracle
and corpus back each capability — with evidence tiers — is documented in
[the validation page](https://securityronin.github.io/vmdk-forensic/validation/).

```bash
cargo test
cargo +stable llvm-cov --workspace --all-features --summary-only
```

## Related

**vmdk** gives you the virtual disk as bytes. These crates read other container
formats the same way — a pure `Read + Seek` over the decoded sector stream:

| Crate | Format |
|---|---|
| [`ewf`](https://github.com/SecurityRonin/ewf) | E01 / Expert Witness Format (EnCase, FTK Imager) |
| [`vhdx`](https://github.com/SecurityRonin/vhdx) | Microsoft VHDX (Hyper-V, Azure) |
| [`vhd`](https://github.com/SecurityRonin/vhd) | Legacy VHD (Virtual PC / Hyper-V Gen-1) |
| [`qcow2`](https://github.com/SecurityRonin/qcow2) | QEMU / KVM QCOW2 |
| [`dd`](https://github.com/SecurityRonin/dd) | Raw / flat / dd images |

Audit a VMDK for tampering, corruption, and recoverability with its forensic sibling:

| Crate | Role |
|---|---|
| [`vmdk-forensic`](https://github.com/SecurityRonin/vmdk-forensic) | VMDK integrity analysis — RGD adjudication, dangling-pointer scan, recovery triage, header provenance, graded anomalies |

Once you have the bytes, these parsers analyse the partition layout inside:

| Crate | Scheme |
|---|---|
| [`mbr-forensic`](https://github.com/SecurityRonin/mbr-forensic) | Master Boot Record — anomalies, slack carving, boot-code ID |
| [`gpt-forensic`](https://github.com/SecurityRonin/gpt-forensic) | GUID Partition Table — backup-header reconciliation, CRC32 |
| [`disk-forensic`](https://github.com/SecurityRonin/disk-forensic) | **Orchestrator** — auto-detects MBR/GPT/APM and dispatches |

Container-format knowledge (magic numbers, header layouts, encoding rules) lives
in [`forensicnomicon`](https://github.com/SecurityRonin/forensicnomicon).

---

[Privacy Policy](https://securityronin.github.io/vmdk-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/vmdk-forensic/terms/) · © 2026 Security Ronin Ltd
