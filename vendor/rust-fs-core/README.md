# rust-fs-core

Published on crates.io as **`rust-fs-core`** (the repository's name) and
imported as `fs_core`. Up to 0.2.24 it was published as `am-fs-core`.

Pure-Rust block-device framework. The shared substrate every filesystem
driver and disk-image reader plugs into.

## What it gives you

- `BlockRead` — read-only random-access block device (`read_at` + `size_bytes`)
- `BlockDevice: BlockRead` — adds optional `write_at` / `flush` /
  `is_writable`, and `set_len` / `can_grow` for the devices that can change
  their own length
- `FileDevice` — backed by a regular file, optional read-only; the one device
  here that can grow, when it is open read-write on a regular file
- `CallbackDevice` — backed by host-process-owned callbacks (FFI from
  Swift / Go / C++)
- `CachingDevice` — LRU read-cache decorator over a `BlockRead`, and over a
  `BlockDevice` when the caller has one to give (`new` vs `read_only`)
- `CountingDevice` — counts the reads and the bytes a driver asks its device
  for, so drivers can be compared against each other and against themselves
  later by one instrument rather than several look-alike ones
- `ReadOnlyDevice` — rejects writes whatever the inner type allows
- `SliceReader` / `OwnedSlice` / `OwnedRwSlice` — a sub-range of a device
  presented as a device, for partition walkers and container readers
- `BlockReadStreamer` — `std::io::Read + Seek` over any `BlockRead`
- A unified `Error` type with a `Custom(String)` escape hatch so each
  driver can lift its own internal errors to the trait boundary
- `cli` — behind the `cli` cargo feature: the command-line plumbing every
  tool in the family shares. One multi-call binary dispatching on `argv[0]`,
  the `<tool> (<crate>) <version>` line, `<repo> doctor`, JSON results with
  `--text` as the opt-out, the `{"error": ..., "code": N}` failure on
  stderr and its exit statuses, and `generate names|man|completions` for
  packaging. A crate describes its tools once, as a `cli::Family`, and calls
  `cli::main`. **Use it; do not copy it.** It began as a directory each
  crate carried by hand, and the copies drifted within weeks. Its
  `transfer` module is the `export` verb every tool shares: a file copied
  out of an image in bounded chunks, with its byte count and SHA-256,
  published as a local file only when complete. A driver supplies a
  positioned read (`cli::transfer::FileSource`).

### The `cli` feature

```toml
[features]
cli = ["dep:clap", "rust-fs-core/cli"]

[[bin]]
name = "rust-fs-<fs>"
path = "src/cli/main.rs"
required-features = ["cli"]
```

Turn it on only under the crate's own `cli` feature, beside the
`required-features` on its binary. clap, clap_complete, clap_mangen and sha2
are optional here and reached only through the feature, so the default build —
and every static library built from it — gains no dependency. The crate's
own `clap` requirement stays, for its tools' commands; `fs_core::cli::clap`
re-exports the one this module is built against.

## Intended consumers

Filesystem drivers — `rust-fs-ext4`, `rust-fs-ntfs`, `rust-fs-xfs`,
`rust-fs-btrfs`, `rust-fs-erofs`, `rust-fs-squashfs`, `rust-fs-bcachefs`;
future exFAT, HFS+, APFS, FAT and ISO 9660 drivers. Each only writes
format-specific code; the block plumbing comes from here.

Disk-image readers — `rust-img-qcow2`, `rust-img-vhd`, `rust-img-vhdx`,
`rust-img-vmdk`; future VDI and raw readers. Same pattern:
format-specific container logic, shared block I/O.

Block-layer utilities — `rust-disk-partitions` (GPT/MBR probe); future
LUKS, LVM and MD-RAID layers. Same trait, opposite direction: consumes a
`BlockRead` to expose slices of it.

## Layout

```
src/
  lib.rs              public re-exports
  error.rs            Error / Result
  block.rs            BlockRead + BlockDevice traits
  file_device.rs      FileDevice (backed by std::fs::File)
  callback_device.rs  CallbackDevice (FFI-friendly)
  caching_device.rs   CachingDevice (LRU decorator)
  counting_device.rs  CountingDevice (counts reads + bytes asked for)
  slice.rs            SliceReader / OwnedSlice / OwnedRwSlice
  readonly.rs         ReadOnlyDevice (rejects writes whatever the inner type allows)
  stream.rs           BlockReadStreamer (std::io::Read + Seek over a BlockRead)
  ffi.rs              the C ABI — FsCoreDevice, FsCoreCallbackCfg
  cli.rs              the tools' shared plumbing (feature `cli`), and in cli/:
                      dispatch, version, doctor, output, docs, family, transfer
tests/
  cache.rs            CachingDevice + interop tests
.github/actions/
  install-chore/      the composite action that installs `chore` in CI
```

## The CI gate, and the chore installer

The one required check is `ci-ok`, and it stands for every job — see
`.github-guard`, which argues why at length. What holds that true is
**`scripts/core.sh ci-gate`**, run by the `fmt` job: the gate workflow must run on
`pull_request`, `ci-ok` must `needs:` every gating job and nothing that does
not exist, it must carry `if: always()` rather than a narrowing of it, and
`.github-guard` must require `ci-ok` alone.

Those rules were `tests/ci_aggregate_gate.rs` here, and in ten sibling
repositories as ten variants of the same 161–173 lines; then briefly a
`crates/am-ci-guard` dev-dependency (#156, #157). Both were the wrong
container. The rules test nothing this crate ships — they parse a YAML file
and compare strings — and the `test` job enforces an executed-test floor, so
a meta-test inflates the very count used to satisfy the gate. A `ci:gate`
subcommand inside `chore` was wrong too: it made a release of a shared tool a
prerequisite for a change here. They are `scripts/ci-gate.sh` now, one of this
crate's family scripts: every repository in the family runs this copy as
`bash scripts/core.sh ci-gate` against its own `ci.yml` and `.github-guard`,
and `scripts/core.sh family-check` refuses a repository that commits its own.

The gate requires git and **Python 3 with PyYAML**, imported by the `python3`
on `PATH`. Use the calling repository's host-tools setup when it provides
one (`chore tools` in the drivers). Core has no host-tools installer. For an
isolated environment, set it up explicitly:

```sh
python3 -m venv tmp/ci-gate-venv
tmp/ci-gate-venv/bin/python3 -m pip install PyYAML
PATH="$PWD/tmp/ci-gate-venv/bin:$PATH" bash scripts/core.sh ci-gate
```

The check never installs dependencies. A missing module exits 2 and names the
interpreter that needs it.

`CI_GATE_NON_GATING` may be declared as a literal, space-separated string in
the workflow's top-level `env`. The gate reads that declaration locally too,
so lint sees the same conditional jobs as CI. A process environment override
takes precedence, including an empty string that clears the declaration.
Expressions such as `${{ vars.CONDITIONAL_JOBS }}` need a resolved process
override; the local checker cannot evaluate GitHub Actions expressions.

This declaration permits a job's scheduling condition; it does not change
the aggregate's verdict. Full-profile jobs still belong in `ci-ok`'s needs,
and its profile verdict must reject failures, cancellations and unexpected
skips. Undeclared conditional jobs and stale exemption names still fail.

`.github/actions/install-chore` is the composite action that installs the
binary (antimatter-studios/chore#52), for the other thing every repository
was writing out by hand:

```yaml
- uses: antimatter-studios/rust-fs-core/.github/actions/install-chore@<ref>
  with:
    version: "0.11.0"
```

Three repositories each carried a copy of `scripts/ci-install-chore.sh`, one
inlined the asset mapping four times, and a fifth hand-written copy asked for
a tarball name that did not exist and 404'd on every scheduled run. The
convention belongs to whoever publishes the releases, so it is spelled once,
checksum-verified. Pin the `@ref`.

## Roadmap

Planned additions (not yet implemented):

- `Logger` hook — pluggable `set_logger(callback)` so consuming crates
  can route diagnostics to a host-provided sink without each crate
  hard-coding a logging dependency.
- `IoStats` hook — the **write** side of the counters, and a hit rate
  generalised beyond `CachingDevice::stats`. The read side has shipped:
  `CountingDevice` counts reads and bytes-read for any `BlockRead`, and
  `CachingDevice::stats` reports its own hits and misses. What is left is
  writes/bytes-written, and one accessor that reads the same way across
  every adapter rather than per type.

## Git hooks

The guards are [github-guard](https://github.com/antimatter-studios/agent-skills)'s,
installed once per clone into `.git/hooks`:

```sh
~/.claude/skills/github-guard/install.sh .
```

Nothing is committed for them: a hook inside the working tree is a hook a
branch checkout can replace, which is what moving them out of it prevented.
The one tracked file is `.github-guard`, which declares the checks `main`
requires.

### Why the dependency-pinning guard looks half-idle here

`pre-commit.d/rust-deps-pinned.sh` is the same file every sibling
project runs, and part of it has nothing to do in this one. That is expected
rather than a misconfiguration, and it is recorded here so nobody has to work
it out twice.

The guard does two jobs. It refuses a workflow that clones a **sibling
project** at a floating ref instead of a tag — and this crate sits at the
bottom of the dependency graph, with no dependency at all in its default build
and only the optional clap crates and sha2 behind `cli`, so there is no sibling to pin
and that half never fires. It also refuses a missing or drifted
`Cargo.lock` and runs `cargo metadata --locked` as a stale-lock check, and
that half applies in full, because this crate does track its lockfile.

The guard is the same in every repository deliberately. The value of a
shared guard is that one audit covers every repository, so a local edit to
trim the idle half would cost more than the idle half does. It is updated by
re-running github-guard's installer, never by editing the copy in `.git/hooks`.

## Verifying a release

From the next release onward, every version published to crates.io is
also attached to the GitHub release for its tag, with a build-provenance
attestation signed by this repository's release workflow. It proves the
crate was built by `.github/workflows/release.yml` from a commit in this
repository, not uploaded from someone's machine. To check the crates.io
download of version `X.Y.Z`:

```sh
curl -sSfLo rust-fs-core-X.Y.Z.crate https://static.crates.io/crates/rust-fs-core/rust-fs-core-X.Y.Z.crate
gh attestation verify rust-fs-core-X.Y.Z.crate \
  --repo antimatter-studios/rust-fs-core \
  --signer-workflow antimatter-studios/rust-fs-core/.github/workflows/release.yml
```

The workflow refuses to attest a `.crate` whose sha256 differs from the
checksum crates.io records for that version, so the file on the release
page and the crates.io download are the same bytes.

## License

MIT.

### Non-filesystem targets

The block traits, callback devices and cache also build for `wasm32-unknown-unknown`.
`FileDevice` and the `fs_core_file_open` C convenience function are available only
on Unix and Windows, where the native positioned-file implementation exists.
Browser embedders supply a callback device; native behavior is unchanged.
