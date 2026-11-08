# Repository Guidelines

This file guides AI agents and contributors working in this repository. Follow the local architecture and existing patterns before introducing new abstractions.

## Project Overview

`lovelycarve` is a forensic evidence tool written in Rust: a Cargo workspace with a native desktop app built with GPUI Kit and a standalone CLI. Both frontends embed the same in-process backend and call its command functions directly; there is no WebView, IPC bridge or web frontend. The backend uses async tasks, memory-mapped raw image access, and pure Rust image, filesystem and encrypted-container readers. Evidence includes disk images, folders, archives, AD1 logical images, iOS local backups, physical disks and assembled RAID arrays. The app targets Windows, macOS, and Linux, but the current desktop development and release recipes are Windows-only; macOS/Linux artifacts need separate verification. Desktop UI copy and CLI messages are Simplified Chinese.

Core workflow: open or create a case, load evidence, detect partitions, browse filesystem data, preview native file types, hash / export / carve, build volume snapshots, run global search and timeline projections, and run LPP plugins. Read `docs/ARCHITECTURE.md` before adding backend modules or desktop pages; before changing the desktop UI also read `crates/desktop/src/theme.rs`, `ui.rs` and `assets.rs`.

Use `docs/README.md` for the documentation map and `CONTRIBUTING.md` / `docs/DEVELOPMENT.md` for setup and verification. Before changing readers, read `docs/formats/README.md` (shared reader rules and the format index) and the relevant capability/limitation guide in `docs/formats/` — file systems `NTFS.md`, `EXT.md`, `FAT.md`, `XFS.md`, `REFS.md`, `APFS.md`, `HFS.md`, `CDFS.md`; partitions and arrays `GPT.md`, `MBR.md`, `LVM.md`, `MDRAID.md`, `FIRMWARE_RAID.md`; encryption and snapshots `LUKS.md`, `BITLOCKER.md`, `FILEVAULT.md`, `VSS.md`; image containers `SMRAW.md`, `QCOW.md`, `PHDI.md`, `UBI.md`; compression and backups `GZIP.md`, `ANDROID_BACKUP.md`, `MIUI_BAK.md` — or `docs/IOS_BACKUPS.md`. Vendored sources and local patches are recorded in `vendor/UPSTREAM.md`; third-party licenses and attribution belong in `THIRD_PARTY_NOTICES.md` and the corresponding vendor directory.

## Project Structure

- `Cargo.toml`: the workspace — its four members, shared dependency versions, lints, build profiles and the `notatin` patch. `vendor/`, `plugins/` and `python/` are excluded and keep their own manifests and lockfiles. `default-members` leaves the desktop app out of a bare `cargo build` / `cargo test` (see Backend Architecture).
- `crates/core/` (`lovelycarve-core`): the forensic engines in twelve domains — evidence, filesystem, catalog, casework, analysis, carving, search, timeline, preview, extensions, platform and common. `examples/` holds the explicitly declared probes and benchmarks; `tests/data`, `tests/fixtures` and `test_data/` hold its committed fixtures.
- `crates/backend/` (`lovelycarve-backend`): the in-process backend — shared state (`state.rs`, `state/`), the task registry (`task_manager.rs`, `task_manager/`), the host types commands receive (`host.rs`), startup, shutdown, errors and cross-domain workflows (`app/`), the per-domain command functions (`commands/`, facade `commands.rs`), the embedding interface frontends use (`embed.rs`) and the command registry (`lib.rs`). Its build script stages the runtime DLLs beside every executable.
- `crates/cli/` (`lovelycarve-cli`): the standalone CLI — entry, the `commands.toml` command schema, argument parsing, the shared execution steps, output, prompts, search, the JSONL session, resident sessions and the shell. `build.rs` and `build/cli_catalog/` generate the dispatch and the command catalogue from the backend registry and `commands.toml`; `cli.manifest` is its Windows manifest.
- `crates/desktop/` (`lovelycarve-desktop`, executable `LovelyCarve`): the GPUI Kit desktop app — `main.rs`, the in-process backend runtime (`backend.rs`), the event hub (`events.rs`), shared stores (cases, tasks, RAID, platform, preferences), the pages (`files/`, `search/`, `timeline/`, `carving/`, `analysis/`, `bcp/`, `evidence/`, `raid/`, `cases/`, `tasks/`, `settings.rs`) inside the `workspace/` shell, the theme (`theme.rs`), shared presentation pieces (`ui.rs`) and its asset source (`assets.rs`, `assets/`). Its build script embeds `assets/icons/icon.ico` as the executable's Windows icon; GPUI Kit embeds the manifest. Its Windows x64 原类型视图 uses an externally installed Outside In Viewer in a separate process.
- `vendor/`: vendored filesystem and parser crates.
- `binaries/`: load-time DLLs shipped beside the executables (Dokan, OpenSSL, the app-local VC++ runtime; OpenCV for QR builds; the legacy ReFS DLL for the oracle only).
- `resources/`: the OCR and QR models a QR/OCR CLI build loads at run time.
- `plugins/`: Git submodule pointing to `LovelyTeams/LovelyCarve-Plugins`, with a separate Cargo workspace — `sdk` (crate `lovelycarve-plugin`: C ABI, LPP v2 payloads, record schemas, Rust plugin API and the `testing` harness; also a host dependency), `sdk-derive` (`#[derive(Record)]`), `host` (`lc-plugin-host`, the process-isolation loader), and the bundled analyzer `linuxinfo`, a `cdylib` next to its `plugin.toml`, `schema.json` and `locales/`. Initialize with `just bootstrap`; commit and push plugin changes before updating the main repository's submodule pointer.
- `python/`: the read-only Python binding over `lovelycarve-core` (`reader-library` feature).
- `integrations/dsh-lovelycarve/`: the DeepSeek Harness plugin that drives the CLI; `skills/lovelycarve-cli/`: the packaged agent skill.
- `scripts/`: packaging (`pack.ps1`, `pack-cli.ps1`, `pack-plugins.ps1` and the shared `lib/release-bundle.ps1`), the plugin build (`build-plugins.mjs`, run with Bun), the Windows build environment, fixture generators and the CLI smoke tests.
- `docs/`: Chinese documentation — the user guide (`USAGE.md`), CLI manual, iOS backup and plugin guides, development, architecture and release guides; `docs/formats/` holds per-format capability and limitation guides.

## Build, Test, and Development Commands

Use the repository-root `Justfile` as the task entry point. Do not replace an existing recipe with a direct `cargo` or `bun` command. Run `just bootstrap` for initial setup; it initializes the pinned plugin submodule. Run `just --list` when the available tasks may have changed, and use `just --show <recipe>` or `just --dry-run <recipe>` to inspect platform-specific commands and dependencies. Prerequisites and environment overrides are documented in `docs/DEVELOPMENT.md`; the toolchain version comes from `rust-version` in the root `Cargo.toml`.

- `just verify-rust`: run `fmt`, `check`, `clippy`, the engine/backend/CLI tests and the desktop tests.
- `just verify-plugins`: run formatting, plugin linting and plugin tests.
- `just dev`: build the plugins and run the desktop app (development build). Program arguments follow `--` (`just dev -- --case <directory> <evidence>`): `--case` opens (or creates) a case and other arguments load evidence.
- `just build [args]` / `just check-desktop [args]` / `just test-desktop [args]`: build, compile-check and test only the desktop app.
- `just build-release`: build the release plugins and the release desktop app (Windows). `just pack` / `just repack` create or recreate the portable Windows archive in `dist-pack/`; pass `-NoUpx` to skip UPX.
- `just fmt`: format the workspace crates, the plugin workspace and the maintained vendored reader crates.
- `just check`: compile-check the engines, backend and CLI, then the desktop app, without the optional OpenCV QR feature.
- `just check-examples`: compile-check every explicitly declared example without the optional OpenCV QR feature.
- `just clippy`: lint the engines, backend and CLI, then the desktop app, without the optional OpenCV QR feature.
- `just test`: run the engine, backend and CLI tests without the optional OpenCV QR feature.
- `just test <name>`: run a targeted engine/backend/CLI test by substring.
- `just test-ewf`: test the vendored pure Rust EWF reader and sample corpus.
- `just test-bitlocker` / `just clippy-bitlocker`: test / lint the vendored pure Rust BitLocker decryptor; set `BDE_ORACLE_IMAGE` to run the full external dfvfs oracle.
- `just test-vhd` / `just test-vhdx`: test the vendored Rust VHD/VHDX readers and committed image fixtures.
- `just test-vmdk`: test the vendored pure Rust VMDK reader and committed image fixtures.
- `just test-qcow2` / `just clippy-qcow2` / `just test-vdi`: test / lint the standalone vendored QCOW and VDI readers; `just generate-qcow-fixtures` regenerates the invented QCOW fixtures (needs Python `cryptography`).
- `just test-smraw` / `just clippy-smraw`, `just test-phdi` / `just clippy-phdi` and `just test-gzipf` / `just clippy-gzipf`: test / lint the standalone split RAW, Parallels PHDI and GZIP readers.
- `just clippy-ubi`: lint the read-only UBI library; its tests run through `just test-vendor ubidump`.
- `just test-vsc` / `just clippy-vsc`: test / lint the Volume Shadow Copy reader; set `VSC_ORACLE_IMAGE` for the external pyvshadow oracle. `just probe-vsc` inspects a raw NTFS volume's VSS metadata and `just compare-vsc <directory> <count>` compares pyvshadow dumps.
- `just test-refs` / `just clippy-refs`: test / lint the standalone pure Rust ReFS reader.
- `just test-ad1`: test the AD1 reader; set `AD1_USERBSS` for the real-image stored-hash oracle.
- `just test-ios-backup`: test the iOS backup reader, auditor and independent encrypted fixtures.
- `just test-sqlite-core`: test SQLite record recovery; oracle tests need `sqlite3` on PATH or `SQLITE3_BIN`.
- `just test-vendor <directory> [args]`: run library tests for a vendored crate without a dedicated reader recipe, such as `fatfs`, `exfat` or `ext4-view`.
- `just build-plugins [--release]`: build the plugin workspace (`lc-plugin-host` and the plugin libraries), copy each library next to its `plugin.toml` and write its `schema.json` (`lc-plugin-host --describe`); `just dev`, `just build-release` and `just pack-cli` run it first.
- `just test-plugins` / `just clippy-plugins`: test / lint the plugin workspace.
- `just run-example probe_partitions <image-path>`: run the standalone partition probe.
- `just run-example probe_partitions <image-path> --mbr`: inspect raw MBR/EBR slots, provenance and recovery warnings.
- `just run-example-fast <name> [args]`: run a probe with `release-fast` and one Cargo build job.
- `just run-cli [args]`: run the standalone `lovelycarve-cli` (development build without QR/OCR). See `docs/CLI.md`.
- `just build-cli`: build the QR/OCR-free standalone release CLI; `just build-cli-full` enables QR/OCR. The build script embeds the `asInvoker` Windows manifest. `just pack-cli` / `just pack-cli-full` package the matching Windows CLI.
- `just test-cli`: build and smoke-test CLI navigation, JSONL sessions, encrypted-volume unlock/export, carving and clean shutdown.
- `just smoke-cli <executable>`: smoke-test an existing CLI binary; `just repack-cli` packages an existing Windows release according to its actual capabilities.
- `just update-cli-skill`: regenerate the agent skill's command reference (`skills/lovelycarve-cli/references/commands.md`) from the CLI command catalogue; a CLI test fails while it is stale.
- `just test-luks` / `just clippy-luks` / `just test-veracrypt`: test / lint the standalone vendored encrypted-container readers.
- `just test-filevault` / `just clippy-filevault`: test / lint the FileVault 2 (CoreStorage) core and forensic auditor; set `FVDE_ORACLE_IMAGE` for the real-sample oracle.
- `just test-apfs` / `just clippy-apfs` and `just test-vendor hfsplus` / `just clippy-hfsplus`: test / lint the standalone APFS and HFS+ readers.
- `just pack-plugins`: build every workspace plugin in release and package each as an installable `.lcpkg` (Windows) in `dist-pack/`.
- `cargo sort -w`: sort Rust manifests after dependency edits, from the repository root; this is allowed directly because no `just` recipe currently wraps it.
- `just bench-filedb-store`: synthetic filedb metadata-index benchmark (`release-fast` profile).
- `just update-rust` / `just update-plugins`: refresh dependencies within reviewed manifest constraints.
- `just qr-env` / `just build-opencv-contrib` / `just qr-test <image> [count]`: show the resolved Visual Studio and OpenCV paths, build contrib OpenCV with `wechat_qrcode` into `OPENCV_INSTALL`, and decode a QR image with the WeChat detector.

Windows `build-cli-full` enables `qrcode-wechat,ocr-cuda` (CUDA OCR with CPU fallback); Unix `build-cli-full` enables `qrcode-wechat,ocr`. The desktop app has no QR/OCR view and builds without them. Default builds need neither OpenCV nor CUDA. Windows host and plugin recipes load `scripts/windows-env.ps1` in the same shell, initialize the VS x64 environment, and default to VS's `lld-link`, one Cargo job, one linker thread and Thin LTO for release. Install the Visual Studio C++ Clang tools; only OpenCV builds configure libclang. Preserve caller overrides through the documented Cargo environment variables and just variables rather than hard-coding local paths into recipes. Standalone vendored reader recipes retain their own Cargo configuration.

The shipping `release` profile is size-first (`opt-level = "s"`). Windows recipes default `CARGO_PROFILE_RELEASE_LTO` to `thin` for both host and plugin workspaces; the manifests retain fat LTO for other platforms. Byte-crunching crates (hashers, zstd / xz / bzip2 / zlib, SQLCipher, memchr / aho-corasick / regex-automata) get `opt-level = 3` through `[profile.release.package.*]` overrides in the root `Cargo.toml`. Add a new hot-path dependency there, and compare against `--profile release-fast` when tuning.

Rust recipes can hold Cargo's global lock for a long time. Be patient with `just build`, `just check`, `just test`, and `just clippy`; do not kill them merely because they appear idle.

Local `just` recipes are the verification source of truth. Windows GitHub Actions call those same recipes for PR checks and tag-triggered releases; macOS/Linux and native GUI/driver behavior still require separate verification.

ReFS uses the vendored pure Rust `refs-reader`; production builds do not load or ship `librefs-13.dll`. The explicit `refs-dll-oracle` feature of `lovelycarve-core` and the `just probe-refs` / `just test-refs-oracle` recipes retain the legacy DLL for comparison only. Compression does not require separate bzip2 or zlib runtime DLLs.

## Backend Architecture

The workspace crates form a strict chain. `lovelycarve-core` depends on no other workspace crate. `lovelycarve-backend` depends on core and re-exports it as `lovelycarve_backend::core`; backend modules reach the engines as `crate::core::…`. The CLI and the desktop app depend on the backend only and alias the backend modules they use at their crate roots. An item another crate uses is `pub`; keep everything else `pub(crate)` or narrower. Fixture builders shared by several crates' tests live in core `test_support` modules gated on `any(test, feature = "test-support")`; only dev-dependencies enable `test-support`.

GPUI turns on serde_json's `preserve_order`, which would change the key order of the CLI's dynamic JSON output if both were compiled in one Cargo invocation. `default-members` therefore excludes the desktop app, and the `just` recipes build it in its own invocation; never compile `-p lovelycarve-desktop` together with the CLI when checking CLI output.

`crates/backend/src/lib.rs` owns the single command registry, the `command_registry![…]` list. Every new command must be registered there: the expansion references each function, so a renamed or removed command fails to compile, and the CLI build script reads the same list to generate its dispatch.

Commands are plain `pub` functions (usually `async`) in the domain modules under `crates/backend/src/commands/`. They take the host types from `crate::host` — `State` (borrowed shared state), `AppHandle` (events and the per-user data directory), `LeaseOwner` (who holds a materialised media lease) — plus JSON-deserializable parameters, and return `Result<T, AppError>`; raw bytes use `crate::host::Response`. The CLI's generated dispatch and the desktop app call them directly. Extend `host` when a command needs a new host capability rather than passing frontend types into commands.

`crates/backend/src/commands.rs` is the command facade and transport helper location. Command implementations live under the ten domain modules in `crates/backend/src/commands/`. Cross-domain orchestration lives in `crates/backend/src/app/workflows/`; application workflows must not depend on commands. Startup and shared shutdown helpers live in `app/setup.rs` and `app/lifecycle.rs`; frontends reach them only through `embed.rs` (`initialize`, `shutdown`, `sanitize_export_name`).

`crates/backend/src/state.rs` owns long-lived runtime state; auxiliary types live in `state/`. Commands receive state through `host::State` and avoid hidden globals. Immediate tasks live in `task_manager`; persistent plugin tasks remain in `core/extensions/task_scheduler`; `app/workflows/task_center.rs` lists both as one task center (`task_list`, `task-entry`).

`AppState.images` stores `Arc<core::filesystem::evidence_session::EvidenceSession>`. A session owns lazy Docker and deleted-NTFS projections and external volume handles; `core::evidence::image::DiskImage` owns source bytes, read caches and decryptors. Byte-only operations borrow `session.disk_image()`. Native filesystem readers accept the byte reader; filesystem traversal accepts the session. External providers implement `filesystem::external_volume::ExternalVolume`; its registry must be drained before asynchronous close. Keep concrete plugin handles out of the byte reader.

Native decrypted partition views are attached to `EvidenceSession` with the source image and partition IDs preserved; filesystem dispatch resolves the attached view. Do not replace source identity or geometry with decrypted offsets. Verified `index_only` sessions may browse persisted directory metadata when the original evidence is missing, but byte reads must fail explicitly; do not start automatic snapshot, hash or platform-detection work on them.

Plugin execution, analysis publication, timeline projection and result cleanup are wired through `app/workflows/scheduler.rs`. The persistent scheduler calls the existing `TaskRunner` contract; it does not open analysis or timeline stores. Validated JSON analysis models live under `core/analysis/results/models.rs`; LPP and ABI definitions remain in the plugin SDK.

`crates/core/src/` contains the twelve domain engines. Place new code in its owning domain; core must not depend on the backend, the frontends or a GUI toolkit. Keep each file focused on one concern. Add new parsers, filesystems, or scanners as explicit modules or trait implementations instead of growing broad switch statements. Do not create new `mod.rs` paths for new modules.

Long tasks should return a task identifier or useful result and propagate errors to the caller. Progress or completion events must be intentional and documented by the command that emits them.

The CLI uses `<collection>… <verb>` commands, writes results to stdout and messages to stderr. Its dispatch and catalogue are generated from the registered backend commands and `crates/cli/src/commands.toml`. Every registered command maps to exactly one CLI command (or a `[[superseded]]` entry naming the command that covers it), and the build script enforces the rules: plural kebab-case collections, the fixed verb vocabulary (`get`, never info/status/show; `delete`, never remove/drop; `list`, never ls/page/query; `update` replaces, `edit` changes part), unique paths, no positional arguments on `list` / `count`, and no collisions with the reserved CLI options and short aliases. Mark a command `concurrent` only after checking its lifecycle under parallel session queries. Help screens are rendered from the catalogue in yargs's layout (`help.rs`); give a command `examples` when its use is not obvious, and `hide_command = true` when it should run without appearing in help, completion, suggestions or the skill's command reference. The commands that need no evidence (`schema`, `system get`) run without starting the backend. Coding agents learn the CLI from the packaged skill `skills/lovelycarve-cli`: `SKILL.md` and `references/workflows.md` are written by hand and their command examples are parsed against the catalogue in tests, while `references/commands.md` is rendered from the catalogue by `just update-cli-skill`. Verify CLI behavior with `just test-cli` when changing a shared command or workflow. Do not edit generated dispatch output or the generated command reference.

### Backend invariants

These are easy to break silently; check them when touching the related code.

- **Cancellation.** Long work registers through `state.tasks` (`TaskManager`) and receives a `tokio_util::sync::CancellationToken`. Pass `&CancellationToken` straight into blocking code and poll `is_cancelled()` — it is `Sync` and needs no runtime. Do not add a polling thread that mirrors the token into an `AtomicBool`. (`bde_vmk` / BitLocker unlock still take `&AtomicBool` on purpose to stay tokio-agnostic.)
- **Task center.** Tasks of kinds the task center lists (`TaskKind::listing`) report what they work on, their progress and counts through their registration (`update`, `emit_progress`) and their outcome (`finish` / `finish_with`) before the last registration drops; one dropped without an outcome shows as cancelled or interrupted. Do not rebuild native task state in a frontend from domain events or command results; extend the task view instead.
- **Read integrity.** Anything that streams the full image (hash, RAW export, decrypted export) must fail when it ends with `offset != total`. Validate file logical lengths, extent/run/cluster bounds and offset arithmetic too. A short read must never produce a "complete" digest or export. Format-defined holes, unwritten extents and uninitialized tails return zeroes; allocated data beyond the volume or a truncated source is an error.
- **Read-fault audit.** Tolerant evidence backends may synthesize zeroes for source bad sectors only with recorded `BadRegion` provenance. Preserve operation-scoped fault collection and the `bad_regions`, `synthetic_bytes` and `read_faults_detected` result fields in hash/export workflows; do not silently treat those bytes as authentic source content. See `docs/ARCHITECTURE.md`.
- **Evidence path identity.** Compare evidence paths only through `core::casework::case::evidence_path_key` (case-insensitive, separator-normalised on Windows). This covers the `image_load` single-flight lock, runtime-id reuse, and case evidence lookup.
- **Image unload.** `app::workflows::evidence::remove_image_runtime` is best-effort: it keeps cleaning after a failure and returns the first error. Every per-image cache in `AppState` (preview sessions, `flat_walk_cache`, `size_trees`, media temp, session snapshots, plugin registries, Dokan mounts) must be purged there. Add new per-image state to that function in the same change.
- **Ranged file reads.** `fs_dispatch::Volume::read_range` is the seekable read path. NTFS, ext2/3/4, FAT/exFAT, XFS, ReFS, APFS and HFS+/HFSX implement it; full, streamed and ranged reads must return identical logical bytes. `FsSession::read_range_direct` also serves contiguous image spans (`unalloc:` / `diskgap:` / `sysfile:` / `deleted:run:`) and AD1 block ranges. Adapters without seeking return `Ok(None)` and fall back to streaming, which costs O(offset). Implement `read_range` when the extent/run/cluster map allows seeking; FAT range reads still traverse cluster metadata and are not O(1).
- **Partition provenance.** Preserve MBR/EBR raw slots, type bytes, boot flags, CHS and record offsets separately from the refined filesystem kind. Validate complete data/link ranges, bound traversal, and report cycles, malformed records and limits; never wrap partition IDs. GPT keeps raw slots, GUIDs and attributes, prefers copies whose header and array CRCs both verify, and takes the disk GUID and partitions from the same copy. See `docs/formats/MBR.md` and `docs/formats/GPT.md`.
- **Filesystem metadata.** Preserve timestamp role, precision and timezone evidence; unknown FAT/exFAT local time must not be labelled UTC. Keep same-name deleted records distinct and validate recoverable content separately from directory identity. ext inline data/EA-inode and indirect/extent maps must obey their format boundaries. See `docs/formats/README.md`, `docs/formats/EXT.md` and `docs/formats/FAT.md`.
- **Encrypted containers.** Verify recovered LUKS master keys against the header digest; LUKS2 keyslot-area and volume-key sizes are independent. Unsupported segments, integrity or reencryption layouts must fail explicitly. Random and sequential decryption must agree, regardless of cache or call order. See `docs/formats/LUKS.md` and `docs/formats/BITLOCKER.md`.
- **Host file names.** Names from evidence written to the host go through `fs_tree_export::sanitize_export_component` (illegal characters, trailing dots / spaces, Win32 device names such as `aux.c` / `CON`); the desktop app reaches it as `embed::sanitize_export_name`. Create export files with `create_new`, never truncate an existing host file.
- **Partial outputs.** Snapshot builders declare `volume_snapshot::PartialOutput` before creating the output file and call `keep()` only after the encoder finishes, so errors and cancellation never leave a truncated `.dirz`.
- **Large reads bypass the block cache.** `DiskImage::read_into` requests of at least 1 MiB skip the 64 KiB located cache for FFI-backed images, so bulk scans do not evict hot metadata blocks. Keep small, random metadata reads below that size.
- **Plugins.** Every discovered plugin package is trusted and dispatchable; there is no approval or trust store. The directory fingerprint is display-only and computed lazily (`PluginDefinition::fingerprint`, blocking). Never hash plugin directories during startup. Plugins are libraries behind the C ABI in `plugins/sdk/src/abi.rs`; every instance runs in `lc-plugin-host` and speaks LPP v2 frames through `PluginConnection` (tests serve an entry point over in-memory pipes through the same client and `sdk::bridge`). Only LPP v2 packages load; each handshake checks the library's `plugin.hello` against `plugin.toml` and `schema.json`. Each task gets its own instance; a run's view, file handles and records belong to its `RunContext` and end with the run. A loaded library is never unloaded.
- **Analysis records.** Runs write typed records into a staging run of the evidence's `records.db` and publish it in one transaction that supersedes the job's previous run; never write records outside `record_store`, and never project naive (offset-less) timestamps into the case timeline. Results of earlier versions in `analysis.db` are read-only (`legacy_query`); do not migrate or rewrite them.
- **CSV exports.** Use `commands::quote_csv_field` (or the core helper it wraps) for every CSV cell; it neutralizes formula prefixes while preserving plain negative numbers. Do not add a separate quoting implementation.

## Command Contract

A feature crossing the backend/frontend boundary normally requires all of these in the same change:

1. Rust core logic in `crates/core/src/`.
2. A command function under `crates/backend/src/commands/` (host arguments from `crate::host`).
3. Registration in `command_registry!` in `crates/backend/src/lib.rs`.
4. A CLI command for it in `crates/cli/src/commands.toml`; the build fails until the command is mapped.
5. When the feature has desktop UI, the view that calls the command through `Backend::run` (`crates/desktop/src/backend.rs`).

Skipping a link usually fails the build; the CLI mapping is enforced by its build script. After command changes, run `just check`, `just test` and `just test-cli`, and `just test-desktop` when the desktop app changed.

## Rust Coding Guidelines

- Prioritize correctness and clarity over speed unless performance is the task.
- Prefer existing files unless the change is a new logical component.
- Avoid `unwrap()` and `expect()` in production code; propagate with `?` or handle explicitly.
- Be careful with indexing and slicing that can panic.
- Never discard fallible results with `let _ =`. Propagate, log visibly, or handle with `match`.
- Ensure async failures reach the caller with meaningful context.
- Inline variables in format strings: use `format!("hello {name}")`. `clippy::uninlined_format_args` is enabled in `[workspace.lints.clippy]` in the root `Cargo.toml`. Existing violations are warnings; `cargo clippy --fix` rewrites them.
- Collapse nested `if` statements when practical.
- Prefer method references over redundant closures.
- Avoid ambiguous bool or `Option` parameters at call sites; prefer enums, named methods, or newtypes.
- Prefer exhaustive `match` arms over wildcard `_` arms.
- Newly added traits need doc comments explaining their role and implementer expectations.
- Use full words for variable names; avoid abbreviations such as `q` for `queue`.
- Do not create single-use helper functions unless they clarify nontrivial logic.
- Use variable shadowing to scope clones for async moves and minimize borrowed lifetimes.

## Module Size and Structure

Target Rust modules under roughly 500 lines excluding tests. If a file approaches 800 lines, add new functionality in a sibling module unless there is a strong reason not to. When extracting code, move related tests and type/module docs with the implementation. Avoid turning `lovelycarve-core` into a catch-all; consider whether an existing crate or a new crate boundary is more appropriate.

## Desktop App Guidelines

The desktop app (`crates/desktop`) is built with GPUI Kit (`gpui-kit`: GPUI, GPUI Component and their icon assets). Views are GPUI entities composed from `gpui_kit::component` widgets; prefer an existing component over hand-drawn chrome.

- Call the backend only through `Backend::global(cx).run(…)`, which spawns the command on the backend's tokio runtime and returns a future to await from `cx.spawn`. Never block the UI thread on backend work; long work runs as backend tasks that report through events.
- Backend events reach the UI thread through `events::EventHub`; models subscribe and pick their events by name. The task center reads `task_list` and `task-entry` (`TaskStore`); do not rebuild task state from domain events.
- Shared state lives in GPUI globals and entities (`CaseStore`, `TaskStore`, `RaidStore`, `PlatformStore`, `Preferences`). Do not add another global store without explicit direction. Interface preferences persist in `<app data>/gpui-preferences.json` (`prefs.rs`).
- Every view must work in the light and the dark theme. Read colors from the active theme, whose palettes live in `theme.rs`; do not hard-code colors in views. Panels use `theme::PANEL_RADIUS` and dialogs `theme::DIALOG_RADIUS`.
- Pages open on a toolbar (search and filters on the left, page actions on the right), then the work surface, then a footer with the page context and counts; the shared pieces are in `ui.rs`. The workspace shell in `workspace.rs` owns the title bar, the evidence tabs and the case, task and settings entries; do not add top-level chrome elsewhere.
- Icons are the Lucide set shipped with GPUI Kit. Every icon a view uses must be listed in `icon_assets!` in `assets.rs` (GPUI Kit's default component icons excepted), or it renders blank although the code compiles and logs `could not find asset` every frame; a test in `assets.rs` fails on an icon that is used but not listed, or listed but unused. The file browser's file type, folder and chevron icons are the bundled set in `assets/file_icons/` (third-party, see its README), mapped by `files/icons/file_types.rs`. Do not add emoji or another icon library.
- Fonts are local: only the Quicksand brand font is bundled (`assets/fonts/`, OFL), registered at startup and used for the brand and wordmark only; text and monospace use the system fonts.
- No coloured stripe on the left edge of cards, list rows, table rows, notifications or panels. Show status with a badge, an icon or a status banner, and selection with the tint plus primary text. Deleted rows keep ordinary row backgrounds with red text, a struck-through name and a deletion badge.
- UI copy is Simplified Chinese. Use the forensic glossary consistently (检材 evidence, 源镜像 source image, 命中 hit, 标签页 tab, 分区间隙 partition gap). Show evidence names, paths, plugin-provided fields and persisted values exactly as stored.
- On Windows x64 the 原类型视图 embeds an Outside In Viewer that runs in a separate process (`files/native.rs`); the backend holds the preview lease until that process exits. GPUI's HWND swap chain is selected at startup so the embedded control is visible.

After desktop changes run `just check-desktop` while iterating and `just test-desktop` before review; verify interactive behavior in the running app (`just dev`), because unit tests do not cover rendering.

## Testing Guidelines

Rust tests are colocated in `#[cfg(test)]` modules. Use focused tests for parser behavior, filesystem traversal, indexing, search, timeline, and command-level changes. Prefer whole-object `assert_eq!` comparisons instead of field-by-field assertions. Use `pretty_assertions::assert_eq` in tests when available. Avoid mutating process environment in tests; pass dependencies or flags from the caller instead. Test fixtures a test in another crate needs belong in the owning crate's `test_support` module behind `test-support`, not in copies.

Use this minimum local verification matrix:

- Rust changes: `just fmt`, `just check`, `just clippy`, and `just test`; desktop changes also need `just test-desktop`; module moves or public reader-interface changes also need `just check-examples`.
- Command changes: the Rust checks above plus `just test-cli`.
- Plugin workspace changes (`plugins/`): `just fmt`, `just clippy-plugins`, `just test-plugins`; SDK changes also need the Rust checks above, since the host depends on the SDK.
- Vendored reader changes: `just fmt` plus the corresponding `just test-*` recipe, or `just test-vendor <directory>` when none exists; host integration changes also need Rust checks. Use reader lint recipes where available.
- CLI command/workflow changes: relevant Rust checks plus `just test-cli`, or `just smoke-cli <executable>` when verifying an already-built binary; run `just update-cli-skill` when commands, options or their descriptions change, and update the skill's hand-written pages when a workflow changes.
- Documentation-only changes: check relative links, referenced paths, recipe names and descriptions against the current implementation; application builds are not required.

The `verify-rust` and `verify-plugins` recipes collect the corresponding checks; they do not replace reader, example or CLI checks required by the changed scope. Use minimal synthetic fixtures and independent known outputs for parser, addressing and corruption regressions. Keep public fixture provenance, licenses and hashes; never commit real evidence, credentials, local case/cache/export directories or machine-specific paths. Distinguish source/API comparisons from actually executed oracle tests, and report any skipped or unavailable verification accurately.

## Logging and Output

Keep CLI/user output separate from diagnostics. Use `println!` or `eprintln!` only for requested data, copyable/actionable content, hard top-level errors, or interactive prompts. Everything else belongs in `tracing`.

Default behavior should be quiet on success. The desktop app's default filter is `warn`; set `RUST_LOG` (for example `RUST_LOG=lovelycarve_backend=info`) to see more. Lifecycle messages are `tracing::info!`, recoverable fallbacks are `tracing::warn!`, and module-level detail is `tracing::debug!`. Do not print success confirmations when the exit code already communicates success.

## Changelog and Documentation

Update `CHANGELOG.md` under `[Unreleased]` for meaningful user-visible features, fixes, breaking changes, deprecations, or removals. Follow [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/): use English change categories, ISO release dates, reverse chronological version sections, and version/comparison links. Keep entries concise and distinguish released-tag content from later changes.

Update `docs/` when changing user-facing behavior, configuration, architecture, commands, or workflow assumptions. Project documentation is written in Chinese; keep each topic in its owner document:

- User-visible workflows and UI behavior: `docs/USAGE.md`; the feature list, format table and screenshots in the root `README.md` when a headline feature changes.
- CLI commands, JSONL sessions and command-line unlock/export: `docs/CLI.md` (also shipped as the README of the CLI package).
- Format capabilities, deliberate differences from reference libraries, limitations and verification boundaries: the matching `docs/formats/<FORMAT>.md`; shared reader rules in `docs/formats/README.md`.
- Workspace layout, crate boundaries, internals, invariants, storage layout, the desktop app's structure and the feature-to-command map: `docs/ARCHITECTURE.md`; build and tooling: `docs/DEVELOPMENT.md`; plugin ABI/LPP: `docs/PLUGIN_PROTOCOL.md`.

Write documentation as a description of the current state, not a work log. Do not record per-session results such as "本次/本轮" narratives, test pass counts, local toolchain failures, absolute local paths, or names and hashes of private evidence; put what you actually ran in the pull request description instead. Public fixtures keep their source, license and hash. Use placeholders such as `<镜像路径>` in example commands. Long per-function reference-library tables belong inside a collapsible `<details>` block. Documentation screenshots go in `docs/images/`, use the light theme, a 1600×1000 window at 1.5× scale, and only public, synthetic or explicitly permitted evidence. When moving or renaming a document, update every reference, including vendored `LOVELYCARVE.md` / `UPSTREAM.md` notes, code comments, packaging scripts (`scripts/pack-cli.ps1` ships `docs/CLI.md`, `scripts/pack-plugins.ps1` ships `docs/LINUXINFO.md`), the agent skill under `skills/lovelycarve-cli` (it points to `docs/formats/`) and the plugin submodule, which links to `docs/PLUGIN_PROTOCOL.md` and `docs/LINUXINFO.md` on `main`.

## Commit and Pull Request Guidelines

Follow the [pull request conventions in CONTRIBUTING.md](CONTRIBUTING.md#拉取请求) when preparing, updating, reviewing, or merging a PR. Adapt external contributions to the repository's existing style and architecture before merging, preserve author attribution, and verify the final PR head after resolving conflicts or changing scope.

Recent history uses Conventional Commits with optional scopes, for example `feat(index): ...`, `fix(fstree): ...`, `perf(index): ...`, and `chore: ...`. Keep subjects imperative and specific.

Every PR merge commit subject must end with its actual PR number in parentheses, for example `feat(plugin): persist plugin logs beside the executable (#3)`. Include this suffix explicitly when overriding `gh pr merge --subject`, and verify the resulting remote commit title after merging. Do not add a PR number to direct commits that do not come from a PR.

Preserve the contributor's original author name and email on PR merge commits, and record the actual maintainer performing the merge as committer (currently `b3nguang`). For an external contribution, GitHub should show the contributor as authored and the maintainer as committed. Verify both identities on the resulting remote commit, including their GitHub account associations; do not replace the original author with the maintainer or invent a co-author merely to display another avatar.

Pull requests should describe user-visible behavior, list the local `just` recipes used for verification, link related issues, and include screenshots or recordings for UI changes. Claim hosted CI coverage only when the relevant workflow run has succeeded.
