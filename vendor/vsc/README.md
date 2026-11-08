# vsc-forensic

LovelyCarve carries the reader only, with local integrity and API patches.
See [LOVELYCARVE.md](LOVELYCARVE.md) and the complete
[libvshadow capability comparison](../../docs/formats/VSS.md) for the shipped
implementation, verification and limitations. The remainder describes the
upstream project; its analyzer, hosted documentation and CI are not vendored.

[![Crates.io vsc-core](https://img.shields.io/crates/v/vsc-core.svg?label=vsc-core)](https://crates.io/crates/vsc-core)
[![Crates.io vsc-forensic](https://img.shields.io/crates/v/vsc-forensic.svg?label=vsc-forensic)](https://crates.io/crates/vsc-forensic)
[![Docs.rs](https://img.shields.io/docsrs/vsc-core?label=docs.rs)](https://docs.rs/vsc-core)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-orange.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/vsc-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/vsc-forensic/actions/workflows/ci.yml)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)

**Windows Volume Shadow Copy (VSS) forensics for Rust — a fuzzed, panic-free-by-lint reader for the shadow-copy store/catalog structures, and a graded anomaly analyzer that turns each NTFS snapshot into evidence you can diff across time.**

VSS is how Windows keeps point-in-time snapshots of an NTFS volume under `System Volume Information`: each shadow copy preserves the blocks that were about to change, so the live volume plus the VSS stores together encode the temporal cohort of the filesystem's past states. `vsc-forensic` is the `[P^H]` disk-history member of the forensic fleet — it navigates that VSS region by snapshot, enumerates the catalog of stores and their metadata, and surfaces shadow-copy timeline and integrity anomalies as fleet findings.

## Quick start

Point the reader at a raw NTFS volume (offset 0 = the NTFS boot sector) and enumerate its shadow copies:

```rust
use std::fs::File;
use vsc::VssVolume;

let mut vol = VssVolume::open(File::open("ntfs_volume.raw")?)?;
println!("shadow copies: {}", vol.store_count());

for store in vol.stores() {
    println!(
        "{}  size {}  created FILETIME {}",
        store.store_id_string(), store.volume_size, store.creation_time,
    );
}

// Read a store's per-snapshot metadata (shadow-copy IDs, attribute flags, machine).
if vol.store_count() > 0 {
    let info = vol.store_info(0)?;
    println!("shadow copy {}", info.shadow_copy_id_string());

    // Reconstruct the volume as it was at the snapshot — copy-on-write blocks
    // overlaid on the live volume — and read any 16 KiB block back.
    let mut snap = vol.snapshot(0)?;
    let block = snap.read_block(0)?; // the NTFS boot sector as it was at snapshot time
    println!("snapshot boot sector: {:02x?}", &block[3..11]); // b"NTFS    "
}
# Ok::<(), vsc::error::VssError>(())
```

Then run the analyzer to get graded findings — no shadow copies where you expected some (consistent with MITRE T1490 deletion), a sequence gap, a non-persistent store:

```rust
use vsc_forensic::audit;

for anomaly in audit(&mut vol) {
    println!("[{:?}] {} — {}", anomaly.severity, anomaly.code, anomaly.note);
    // e.g. [Info] VSC-STORE-PRESENT — shadow copy 1afc8871-… created 2023-01-04T21:38:00Z
}
```

## The two-crate split

Following the fleet reader/analyzer standard, the workspace ships two crates:

| Crate | Role | Depends on | Emits |
|---|---|---|---|
| [`vsc-core`](https://crates.io/crates/vsc-core) | reader / decoder | `uuid`, `thiserror` | typed VSS catalog / store records |
| [`vsc-forensic`](https://crates.io/crates/vsc-forensic) | anomaly analyzer | `vsc-core`, `forensicnomicon` | graded [`forensicnomicon::report::Finding`](https://crates.io/crates/forensicnomicon)s |

The reader stays pure — it decodes bytes and makes no judgments. All *forensic meaning* lives in the analyzer, a side-effect-free function of already-decoded records. That separation is why `vsc-core` is useful on its own and why `vsc-forensic` drops straight into a fleet `Report` next to every other analyzer.

## Findings

| Code | Meaning |
|---|---|
| `VSC-NO-SHADOW-COPIES` | a VSS volume header is present but the catalog holds zero stores — consistent with MITRE T1490 shadow-copy deletion *or* a volume that never had snapshots |
| `VSC-STORE-PRESENT` | one finding per enumerated shadow copy, carrying its GUID and creation time |
| `VSC-SEQUENCE-GAP` | non-contiguous catalog sequence numbers — consistent with a deleted intermediate store |
| `VSC-STORE-NON-PERSISTENT` | a store whose attribute flags mark it non-persistent |

Findings are observations, not verdicts — the "consistent with" framing is deliberate; the analyst or tribunal draws the conclusion.

## Capabilities

| Capability | Status |
|---|---|
| VSS volume header + catalog enumeration (store GUID, size, sequence, creation time) | ✅ |
| Store metadata decode (shadow-copy IDs, attribute flags, originating machine) | ✅ |
| `vsc-forensic` anomaly auditor (`VSC-*` findings → `forensicnomicon::report`) | ✅ |
| Fuzzed (`fuzz_catalog` / `fuzz_store` / `fuzz_reconstruct`) + Tier-1 validated against `libvshadow` | ✅ |
| COW block-list reconstruction — materialize a snapshot's view of the volume, read any block back | ✅ |
| `vfs` feature — publish a reconstructed shadow copy as a [`forensic-vfs`](https://crates.io/crates/forensic-vfs) `ImageSource`, the fleet's `[H]` state-history byte source (needs Rust 1.85) | ✅ |

## Trust but verify

Both crates enforce the fleet hardening contract: `#![forbid(unsafe_code)]`, the Paranoid-Gatekeeper clippy set (`unwrap_used`/`expect_used` denied), bounds-checked readers that never panic on malformed input, `cargo-deny` supply-chain gating, and a 100%-line-coverage CI gate. Every parsed structure has a `cargo-fuzz` target whose invariant is "must not panic".

Correctness is proven against an **independent third-party oracle** — [`libvshadow`](https://github.com/libyal/libvshadow) (via `pyvshadow`) — run on a real public CTF disk image: `vsc-core`'s catalog and store-metadata output matches the oracle field-for-field (store count, GUID, volume size, creation FILETIME, shadow-copy IDs), and its **reconstruction** reproduces the snapshot's bytes block-for-block across every path — passthrough, bitmap zero-fill, plain copy-on-write, and 512-byte overlay merge (validated over 1,415 blocks; the algorithm is documented in [docs/RECONSTRUCTION.md](docs/RECONSTRUCTION.md)). See [`tests/data/README.md`](tests/data/README.md) and the env-gated `oracle_pcmus001` / `reconstruct_pcmus001` integration tests.

## Documentation

The curated docs site is built with MkDocs and served from GitHub Pages. See [docs/RESEARCH.md](docs/RESEARCH.md) for the VSS on-disk format research behind the design.

[Privacy Policy](https://securityronin.github.io/vsc-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/vsc-forensic/terms/) · © 2026 Security Ronin Ltd
