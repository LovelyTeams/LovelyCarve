# 2. Package `filevault-core`, import alias `filevault`; analyzer `filevault-forensic`

Date: 2026-07-24
Status: Accepted

## Context

The reader crate wants the ergonomic import path `use filevault::…` (see the
doctests in `core/src/lib.rs` and the README), but the bare `filevault` name is
not available to us as a standalone package under the fleet's naming grammar —
the reader/analyzer split reserves the crate names `<x>-core` (reader) and
`<x>-forensic` (analyzer) for a single-format repo (`ronin-issen/CLAUDE.md` →
"Crate naming grammar → Pattern A" and the `[lib] name = "<x>"` rule in
"Crate-structure standard").

## Decision

Publish the reader as package **`filevault-core`** with a library alias so
consumers still write `use filevault::…`:

```toml
# core/Cargo.toml
[package]
name = "filevault-core"
[lib]
name = "filevault"
```

Publish the analyzer as package **`filevault-forensic`** (`forensic/Cargo.toml`),
imported as `filevault_forensic`. The workspace dependency wires the alias:
`filevault = { version = "0.1", path = "core", package = "filevault-core" }`
(`Cargo.toml` `[workspace.dependencies]`).

## Consequences

- The import path (`use filevault::FileVaultVolume`) is stable and reads
  cleanly, while the published crate names follow the fleet grammar
  (`filevault-core` / `filevault-forensic`), self-describing on crates.io.
- The README badges and `crates.io` links point at the package names
  (`filevault-core`, `filevault-forensic`), not the alias.
- The precise reason the bare `filevault` package name is unavailable (a
  third-party crate collision vs. a deliberate fleet reservation) is not recorded
  in-repo; the `-core`/`-forensic` + `[lib] name` pattern is applied per the
  constitution regardless of which holds (Rationale reconstructed from structure;
  original intent not recovered in available history).
