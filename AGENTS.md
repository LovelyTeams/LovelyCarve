# Repository Guidelines

This file guides AI agents and contributors working in this repository. Follow the local architecture and existing patterns before introducing new abstractions.

## Project Overview

`lovelycarve` is a Tauri v2 desktop forensic evidence tool with a window-free CLI. The backend is Rust with async tasks, memory-mapped raw image access, and pure Rust image, filesystem and encrypted-container readers. Evidence includes disk images, folders, archives, AD1 logical images, iOS local backups, physical disks and assembled RAID arrays. The frontend is Vue 3 (Composition API), TypeScript, and Vite. The app targets Windows, macOS, and Linux, but the current desktop development and release recipes are Windows-only; macOS/Linux artifacts need separate verification. UI copy supports Simplified Chinese and English through `src/composables/useLocale.ts` and the catalogs in `src/i18n/`; Chinese is the default.

Core workflow: open or create a case, load evidence, detect partitions, browse filesystem data, preview native file types, hash / export / carve, build volume snapshots, run global search and timeline projections, and run LPP plugins. Read `docs/ARCHITECTURE.md` before adding backend modules. Read `docs/FRONTEND.md` (the frontend spec: principles, tokens and contrast, type scale, spacing, primitives, states and feedback, tables, dialogs and layering, motion, copy, accessibility, review checklist) before changing UI; the design system itself is the CSS tokens in `src/styles/themes.css` plus the shared `ui-*` primitives in `src/styles/ui.css` and the motion layer in `src/styles/motion.css` — read them too.

Use `docs/README.md` for the documentation map and `CONTRIBUTING.md` / `docs/DEVELOPMENT.md` for setup and verification. Before changing readers, read `docs/formats/README.md` (shared reader rules and the format index) and the relevant capability/limitation guide in `docs/formats/` — `NTFS.md`, `EXT.md`, `FAT.md`, `XFS.md`, `REFS.md`, `APFS.md`, `HFS.md`, `GPT.md`, `MBR.md`, `LVM.md`, `LUKS.md`, `BITLOCKER.md`, `FILEVAULT.md`, `VSS.md` — or `docs/IOS_BACKUPS.md`. Vendored sources and local patches are recorded in `src-tauri/vendor/UPSTREAM.md`; third-party licenses and attribution belong in `THIRD_PARTY_NOTICES.md` and the corresponding vendor directory.

## Project Structure

- `src/`: Vue 3 frontend.
- `src/app/`: bootstrap, main-shell components, workspace navigation/tabs, and cross-feature workflow wiring.
- `src/features/`: twelve business domains, each colocating components, composables, pure models, and tests. Feature collaboration uses small `api.ts` files, explicit component entries, or pure contracts; features must not import `app/`.
- `src/components/`: shared `ui/`, `feedback/`, `layout/`, and `display/` components; no business state dependencies.
- `src/composables/`: generic reactive capabilities such as theme, motion, persistence, notifications, and confirmation; business state lives in its feature's `composables/` (no global store).
- `src/lib/models/`: shared pure contracts such as file-location targets; transport types remain under IPC.
- `src/utils/`: shared pure helpers for formatting, safe HTML, CSV and similar concerns.
- `src/views/`: secondary app windows (the detached `FilePreviewWindow.vue`). The main shell is `src/AppShell.vue`.
- `src/lib/ipc/`: the typed IPC service layer — `client.ts` (sole `invoke` wrapper), `events.ts` (`onEvent`/`emitEvent`), and per-domain command modules. Components/composables call this layer, not Tauri `invoke`/`listen` directly.
- `src/i18n/`: English translations, Chinese context overrides, and the pure translation contract. `useLocale` owns the persisted preference and cross-window synchronization.
- `src/styles/themes.css`: all theme tokens for `light`, `dark`, and `pink`.
- `src/styles/ui.css`: shared UI primitives (buttons, fields, tabs, badges, cards, modal shell, menus, empty states, panel headers), loaded globally through `global.css`.
- `src/styles/motion.css`: the motion layer (shared keyframes, primitive entrances, the `ui-stagger` utility, the theme reveal), loaded globally after `ui.css`.
- `src-tauri/src/`: Rust backend.
- `src-tauri/src/commands/`: per-domain `#[tauri::command]` handlers.
- `src-tauri/src/app/`: startup, shutdown, shared application access/error types, and cross-domain workflows.
- `src-tauri/src/bin/` and `src-tauri/src/cli/`: CLI entry, input, shell, workflows and command discovery; dispatch is generated from the desktop command registry.
- `src-tauri/src/core/`: forensic engines, parsers, indexing, filesystems, search, timeline, carving, and plugin runtime.
- `src-tauri/src/state.rs`: shared application state.
- `src-tauri/src/task_manager.rs` and `src-tauri/src/task_manager/`: immediate task registration, cancellation, resource limits and task models.
- `src-tauri/vendor/`: vendored filesystem and parser crates.
- `src-tauri/examples/`: standalone probes for partition and filesystem work.
- `plugins/`: Git submodule pointing to `LovelyTeams/LovelyCarve-Plugins`, with a separate Cargo workspace — `sdk` (crate `lovelycarve-plugin`: C ABI, LPP wire types, Rust plugin API; also a host dependency), `host` (`lc-plugin-host`, the process-isolation loader), and the single bundled analyzer (`linuxinfo`), a `cdylib` next to its `plugin.toml`. Initialize with `just bootstrap`; commit and push plugin changes before updating the main repository's submodule pointer.
- `docs/`: Chinese documentation — the user guide (`USAGE.md`), CLI manual, iOS backup and plugin guides, development, architecture, frontend and release guides; `docs/formats/` holds per-format capability and limitation guides; `docs/images/` holds the README screenshots.

## Build, Test, and Development Commands

Use the repository-root `Justfile` as the task entry point. Do not replace an existing recipe with a direct `bun`, `cargo`, or Tauri command. Run `just bootstrap` for initial setup; it initializes the pinned plugin submodule and installs frontend dependencies with the frozen lockfile. Run `just --list` when the available tasks may have changed, and use `just --show <recipe>` or `just --dry-run <recipe>` to inspect platform-specific commands and dependencies. Prerequisites and environment overrides are documented in `docs/DEVELOPMENT.md`; toolchain versions come from `src-tauri/Cargo.toml` and `package.json`.

- `just verify-fe`: run IPC, structure and translation checks, type-checking, frontend tests and the production build.
- `just verify-rust`: run `fmt`, `check`, `clippy` and backend tests.
- `just verify-plugins`: run formatting, plugin linting and plugin tests.
- `just dev-fe`: run the Vite frontend, including the file-viewer asset preparation step.
- `just dev`: run the Tauri desktop app without QR/OCR using the configured Visual Studio and OpenSSL environment; `just dev-full` also enables QR/OCR and configures OpenCV.
- `just typecheck`: run `vue-tsc --noEmit`.
- `just build-fe`: type-check and build the frontend production bundle.
- `just test-fe`: run the complete frontend Vitest suite once.
- `just ipc-check`: enforce the typed frontend IPC boundary.
- `just i18n-check`: check translation coverage, interpolation parameters, and untranslated template text.
- `just frontend-structure-check`: enforce feature/shared/model dependency boundaries, valid import/style paths, and absence of runtime dependency cycles.
- `just fmt`: format the main Rust crate, the plugin workspace and the maintained vendored reader crates.
- `just check`: compile-check the Rust backend without the optional OpenCV QR feature.
- `just check-examples`: compile-check every explicitly declared example without the optional OpenCV QR feature.
- `just clippy`: lint the Rust backend without the optional OpenCV QR feature.
- `just test`: run the Rust backend tests without the optional OpenCV QR feature.
- `just test <name>`: run a targeted Rust test by substring.
- `just test-ewf`: test the vendored pure Rust EWF reader and sample corpus.
- `just test-bitlocker`: test the vendored pure Rust BitLocker decryptor; set `BDE_ORACLE_IMAGE` to run the full external dfvfs oracle.
- `just test-vhd` / `just test-vhdx`: test the vendored Rust VHD/VHDX readers and committed image fixtures.
- `just test-vmdk`: test the vendored pure Rust VMDK reader and committed image fixtures.
- `just test-qcow2` / `just test-vdi`: test the standalone vendored image readers.
- `just test-vsc`: test the Volume Shadow Copy reader; set `VSC_ORACLE_IMAGE` for the external pyvshadow oracle.
- `just test-refs` / `just clippy-refs`: test / lint the standalone pure Rust ReFS reader.
- `just test-ad1`: test the AD1 reader; set `AD1_USERBSS` for the real-image stored-hash oracle.
- `just test-ios-backup`: test the iOS backup reader, auditor and independent encrypted fixtures.
- `just test-sqlite-core`: test SQLite record recovery; oracle tests need `sqlite3` on PATH or `SQLITE3_BIN`.
- `just test-vendor <directory> [args]`: run library tests for a vendored crate without a dedicated reader recipe, such as `fatfs`, `exfat` or `ext4-view`.
- `just build-plugins [--release]`: build the plugin workspace (`lc-plugin-host` and the plugin libraries) and copy each library next to its `plugin.toml`; `just dev`, `just build-release` and `just pack-cli` run it first.
- `just test-plugins` / `just clippy-plugins`: test / lint the plugin workspace.
- `just build` / `just build-full`: build the backend without / with QR/OCR. Cargo default features are empty.
- `just build-release` / `just build-release-full`: create the Windows release desktop bundle without / with QR/OCR; both merge `src-tauri/tauri.plugins.conf.json`, and the full entry also merges `src-tauri/tauri.full.conf.json` for OpenCV and models. Tauri's MSI bundler also packs every DLL left in `src-tauri/target/release`, so the default entry first removes the undeclared ones (`scripts/prune-bundle-dlls.ps1`).
- `just run-example probe_partitions <image-path>`: run the standalone partition probe.
- `just run-example probe_partitions <image-path> --mbr`: inspect raw MBR/EBR slots, provenance and recovery warnings.
- `just run-example-fast <name> [args]`: run a probe with `release-fast` and one Cargo build job.
- `just run-cli [args]`: run the window-free `lovelycarve-cli` shell / complete command surface (development build without QR/OCR). See `docs/CLI.md`.
- `just build-cli`: build the QR/OCR-free release CLI; `just build-cli-full` enables QR/OCR. `just pack-cli` / `just pack-cli-full` package the matching Windows CLI.
- `just test-cli`: build and smoke-test CLI navigation, JSONL sessions, encrypted-volume unlock/export, carving and clean shutdown.
- `just smoke-cli <executable>`: smoke-test an existing CLI binary; `just repack-cli` packages an existing Windows release according to its actual capabilities.
- `just test-luks` / `just clippy-luks` / `just test-veracrypt`: test / lint the standalone vendored encrypted-container readers.
- `just test-filevault` / `just clippy-filevault`: test / lint the FileVault 2 (CoreStorage) core and forensic auditor; set `FVDE_ORACLE_IMAGE` for the real-sample oracle.
- `just test-apfs` / `just clippy-apfs` and `just test-vendor hfsplus` / `just clippy-hfsplus`: test / lint the standalone APFS and HFS+ readers.
- `just pack-plugins`: build and package the bundled analyzer and its process-isolation host for Windows.
- `just pack` / `just repack`: create or recreate the portable Windows archive without QR/OCR; use `just pack-full` / `just repack-full` for a full-featured build.
- `cd src-tauri; cargo sort -w`: sort Rust manifests after dependency edits; this is allowed directly because no `just` recipe currently wraps it.
- `just bench-filedb-store`: synthetic filedb metadata-index benchmark (`release-fast` profile).
- `just update-fe` / `just update-rust` / `just update-plugins`: refresh dependencies within reviewed manifest constraints.
- `just update-tauri`: update the frontend and backend Tauri package families together; their patch versions need not match.
- `just update-file-viewer <version> [args]`: update both viewer packages to the same exact version and refresh offline assets.
- `just icons`: regenerate platform icons and frontend brand variants from `src-tauri/icons/source*.png`.

Windows `*-full` builds enable `qrcode-wechat,ocr-cuda` (CUDA OCR with CPU fallback); Unix `build-cli-full` enables `qrcode-wechat,ocr`. Default builds need neither OpenCV nor CUDA. Windows recipes use the default MSVC linker and load `scripts/windows-env.ps1` in the same shell; LLVM/libclang is configured only for OpenCV builds. Override paths through the documented environment or just variables rather than hard-coding local paths into recipes.

Vue type-checking uses the `typescript` npm alias for `typescript-native-bridge`, with the alias and override pinned to the same complete version. Preserve compatibility with `vue-tsc`; do not replace it directly with `typescript@7`. Rich previews use matching `@file-viewer/vue3-full` / `@file-viewer/vite-plugin` versions and offline assets; generated `public/file-viewer/` resources are not committed.

Frontend builds use Vite 8's bundled Oxc minifier. Do not restore `build.minify: 'esbuild'` or obsolete `esbuild.supported` settings without explicitly managing the optional dependency. Preserve the existing platform targets; the map renderer's BigInt dependency still requires Safari 14+ for map/shp previews.

The shipping `release` profile is size-first (`opt-level = "s"`, fat LTO). Byte-crunching crates (hashers, zstd / xz / bzip2 / zlib, SQLCipher, memchr / aho-corasick / regex-automata) get `opt-level = 3` through `[profile.release.package.*]` overrides in `src-tauri/Cargo.toml`. Add a new hot-path dependency there, and compare against `--profile release-fast` when tuning.

Rust recipes can hold Cargo's global lock for a long time. Be patient with `just build`, `just check`, `just test`, and `just clippy`; do not kill them merely because they appear idle.

Local `just` recipes are the verification source of truth. This repository has no GitHub Actions workflows; contributors and maintainers run checks and release builds locally.

ReFS uses the vendored pure Rust `refs-reader`; production builds do not load or ship `librefs-13.dll`. The explicit `refs-dll-oracle` feature and `just probe-refs` / `just test-refs-oracle` recipes retain the legacy DLL for comparison only. Compression does not require separate bzip2 or zlib runtime DLLs.

## Backend Architecture

`src-tauri/src/lib.rs` owns the Tauri builder, plugins, shared state registration, and the `invoke_handler!` list. Every new command must be registered there.

`src-tauri/src/commands.rs` is the command facade and transport helper location. Command implementations live under the ten domain modules in `src-tauri/src/commands/`. Cross-domain orchestration lives in `src-tauri/src/app/workflows/`; application workflows must not depend on commands. Startup and shared shutdown helpers live in `app/setup.rs` and `app/lifecycle.rs`.

`src-tauri/src/state.rs` owns long-lived runtime state; auxiliary types live in `state/`. Commands should receive state through `tauri::State` and avoid hidden globals. Immediate tasks live in `task_manager`; persistent plugin tasks remain in `core/extensions/task_scheduler`.

`AppState.images` stores `Arc<core::filesystem::evidence_session::EvidenceSession>`. A session owns lazy Docker and deleted-NTFS projections and external volume handles; `core::evidence::image::DiskImage` owns source bytes, read caches and decryptors. Byte-only operations borrow `session.disk_image()`. Native filesystem readers accept the byte reader; filesystem traversal accepts the session. External providers implement `filesystem::external_volume::ExternalVolume`; its registry must be drained before asynchronous close. Keep concrete plugin handles out of the byte reader.

Native decrypted partition views are attached to `EvidenceSession` with the source image and partition IDs preserved; filesystem dispatch resolves the attached view. Do not replace source identity or geometry with decrypted offsets. Verified `index_only` sessions may browse persisted directory metadata when the original evidence is missing, but byte reads must fail explicitly; do not start automatic snapshot, hash or platform-detection work on them.

Plugin execution, analysis publication, timeline projection and result cleanup are wired through `app/workflows/scheduler.rs`. The persistent scheduler calls the existing `TaskRunner` contract; it does not open analysis or timeline stores. Validated JSON/IPC analysis models live under `core/analysis/results/models.rs`; LPP and ABI definitions remain in the plugin SDK.

`src-tauri/src/core/` contains twelve domain engines: evidence, filesystem, catalog, casework, analysis, carving, search, timeline, preview, extensions, platform, and common. Place new code in its owning domain; core must not depend on app, commands, state, cli, or Tauri. Keep each file focused on one concern. Add new parsers, filesystems, or scanners as explicit modules or trait implementations instead of growing broad switch statements. The current `mod.rs` files are existing exceptions; do not create new `mod.rs` paths for new modules.

Long tasks should return a task identifier or useful result and propagate errors to the UI. Progress or completion events must be intentional and documented by the command that emits them.

CLI command discovery and dispatch are generated from the registered Rust handlers during the build. Keep command names, argument serialization and help groups stable, and verify CLI behavior when changing a shared command or workflow. Do not edit generated dispatch output.

### Backend invariants

These are easy to break silently; check them when touching the related code.

- **Cancellation.** Long work registers through `state.tasks` (`TaskManager`) and receives a `tokio_util::sync::CancellationToken`. Pass `&CancellationToken` straight into blocking code and poll `is_cancelled()` — it is `Sync` and needs no runtime. Do not add a polling thread that mirrors the token into an `AtomicBool`. (`bde_vmk` / BitLocker unlock still take `&AtomicBool` on purpose to stay tokio-agnostic.)
- **Read integrity.** Anything that streams the full image (hash, RAW export, decrypted export) must fail when it ends with `offset != total`. Validate file logical lengths, extent/run/cluster bounds and offset arithmetic too. A short read must never produce a "complete" digest or export. Format-defined holes, unwritten extents and uninitialized tails return zeroes; allocated data beyond the volume or a truncated source is an error.
- **Read-fault audit.** Tolerant evidence backends may synthesize zeroes for source bad sectors only with recorded `BadRegion` provenance. Preserve operation-scoped fault collection and the `bad_regions`, `synthetic_bytes` and `read_faults_detected` result fields in hash/export workflows; do not silently treat those bytes as authentic source content. See `docs/ARCHITECTURE.md`.
- **Evidence path identity.** Compare evidence paths only through `core::casework::case::evidence_path_key` (case-insensitive, separator-normalised on Windows). This covers the `image_load` single-flight lock, runtime-id reuse, and case evidence lookup.
- **Image unload.** `app::workflows::evidence::remove_image_runtime` is best-effort: it keeps cleaning after a failure and returns the first error. Every per-image cache in `AppState` (preview sessions, `flat_walk_cache`, media temp, session snapshots, plugin registries, Dokan mounts) must be purged there. Add new per-image state to that function in the same change.
- **Ranged file reads.** `fs_dispatch::Volume::read_range` is the seekable read path. NTFS, ext2/3/4, FAT/exFAT, XFS, ReFS, APFS and HFS+/HFSX implement it; full, streamed and ranged reads must return identical logical bytes. `FsSession::read_range_direct` also serves contiguous image spans (`unalloc:` / `diskgap:` / `sysfile:` / `deleted:run:`) and AD1 block ranges. Adapters without seeking return `Ok(None)` and fall back to streaming, which costs O(offset). Implement `read_range` when the extent/run/cluster map allows seeking; FAT range reads still traverse cluster metadata and are not O(1).
- **Partition provenance.** Preserve MBR/EBR raw slots, type bytes, boot flags, CHS and record offsets separately from the refined filesystem kind. Validate complete data/link ranges, bound traversal, and report cycles, malformed records and limits; never wrap partition IDs. GPT keeps raw slots, GUIDs and attributes, prefers copies whose header and array CRCs both verify, and takes the disk GUID and partitions from the same copy. See `docs/formats/MBR.md` and `docs/formats/GPT.md`.
- **Filesystem metadata.** Preserve timestamp role, precision and timezone evidence; unknown FAT/exFAT local time must not be labelled UTC. Keep same-name deleted records distinct and validate recoverable content separately from directory identity. ext inline data/EA-inode and indirect/extent maps must obey their format boundaries. See `docs/formats/README.md`, `docs/formats/EXT.md` and `docs/formats/FAT.md`.
- **Encrypted containers.** Verify recovered LUKS master keys against the header digest; LUKS2 keyslot-area and volume-key sizes are independent. Unsupported segments, integrity or reencryption layouts must fail explicitly. Random and sequential decryption must agree, regardless of cache or call order. See `docs/formats/LUKS.md` and `docs/formats/BITLOCKER.md`.
- **Host file names.** Names from evidence written to the host go through `fs_tree_export::sanitize_export_component` (illegal characters, trailing dots / spaces, Win32 device names such as `aux.c` / `CON`). Create export files with `create_new`, never truncate an existing host file.
- **Partial outputs.** Snapshot builders declare `volume_snapshot::PartialOutput` before creating the output file and call `keep()` only after the encoder finishes, so errors and cancellation never leave a truncated `.dirz`.
- **Large reads bypass the block cache.** `DiskImage::read_into` requests of at least 1 MiB skip the 64 KiB located cache for FFI-backed images, so bulk scans do not evict hot metadata blocks. Keep small, random metadata reads below that size.
- **Plugins.** Every discovered plugin is trusted and dispatchable; there is no approval or trust store. The directory fingerprint is display-only and computed lazily (`PluginDefinition::fingerprint`, blocking). Never hash plugin directories during startup. Plugins are libraries behind the C ABI in `plugins/sdk/src/abi.rs`; both isolation modes (`lc-plugin-host` over LPP frames, or in-process) go through `PluginConnection` and must carry the same methods and limits. A loaded library is never unloaded, and an in-process instance is destroyed only after its last call returns.
- **CSV exports.** Use `commands::quote_csv_field` in Rust or the shared frontend CSV export helpers in `src/utils/exportFormat.ts`; both neutralize formula prefixes while preserving plain negative numbers. Do not add a separate quoting implementation.

## IPC Contract

A feature crossing the frontend/backend boundary normally requires all of these in the same change:

1. Rust core logic in `src-tauri/src/core/`.
2. A Tauri command wrapper under `src-tauri/src/commands/`.
3. Registration in `src-tauri/src/lib.rs`.
4. A typed command function and matching types in the relevant `src/lib/ipc/<domain>.ts`.
5. A composable or component that consumes the wrapper.

Skipping one link often fails only at runtime. After IPC changes, run `just ipc-check`, `just typecheck`, and the relevant frontend/backend build or test recipes.

## Rust Coding Guidelines

- Prioritize correctness and clarity over speed unless performance is the task.
- Prefer existing files unless the change is a new logical component.
- Avoid `unwrap()` and `expect()` in production code; propagate with `?` or handle explicitly.
- Be careful with indexing and slicing that can panic.
- Never discard fallible results with `let _ =`. Propagate, log visibly, or handle with `match`.
- Ensure async failures reach the UI layer with meaningful context.
- Inline variables in format strings: use `format!("hello {name}")`. `clippy::uninlined_format_args` is enabled in `[lints.clippy]` in `src-tauri/Cargo.toml`. Existing violations are warnings; `cargo clippy --fix` rewrites them.
- Collapse nested `if` statements when practical.
- Prefer method references over redundant closures.
- Avoid ambiguous bool or `Option` parameters at call sites; prefer enums, named methods, or newtypes.
- Prefer exhaustive `match` arms over wildcard `_` arms.
- Newly added traits need doc comments explaining their role and implementer expectations.
- Use full words for variable names; avoid abbreviations such as `q` for `queue`.
- Do not create single-use helper functions unless they clarify nontrivial logic.
- Use variable shadowing to scope clones for async moves and minimize borrowed lifetimes.

## Module Size and Structure

Target Rust modules under roughly 500 lines excluding tests. If a file approaches 800 lines, add new functionality in a sibling module unless there is a strong reason not to. When extracting code, move related tests and type/module docs with the implementation. Avoid turning `core` into a catch-all; consider whether an existing crate or a new crate boundary is more appropriate.

## Frontend Guidelines

Vue 3 single-file components with `<script setup lang="ts">` and the Composition API. The design system is the CSS tokens in `src/styles/themes.css`, the primitives in `src/styles/ui.css` and the motion layer in `src/styles/motion.css` — read them before editing UI. The rules below summarize `docs/FRONTEND.md` (Chinese, authoritative for frontend conventions); change both together when a rule changes.

- Component state is `ref` / `computed` / `watch`. Business state lives in `features/<domain>/composables/useXxx.ts`; generic capabilities live in root `composables/`. Do not add Vuex, Pinia, or any global store without explicit direction.
- Keep pure models free of reactive state, host globals, and IPC calls. Feature internals must not import their own API barrel; API barrels must not eagerly export heavy viewer components. Cross-feature dependencies are declared in `scripts/lib/frontend-structure.mjs`; run `just frontend-structure-check` after structural changes.
- Main-window wiring owns workspace singleton disposers; instance scopes own subscriptions, timers, media leases, and Blob URLs. Deactivation releases visible-panel keyboard/menu resources without cancelling backend work. Guard asynchronous publication by context generation, immediately release late handles, and flush tag/derived writes before switching cases. See `docs/FRONTEND_STRUCTURE.md` for the ownership matrix.
- Tauri access goes through `src/lib/ipc/`; never call `invoke`/`listen` inline in a component or composable.
- Every UI must work in `light`, `dark`, and `pink`. Consume colors, radii, shadows, and transitions through existing `var(--*)` tokens; each token is defined in all three theme blocks (`:root`, `[data-theme='dark']`, `[data-theme='pink']`), so a new token must be added to all three.
- Do not use hex colors, literal `white`/`black`, or hard-coded `rgba(...)` outside `themes.css`, except translucent overlays where no token fits. Component CSS is `<style scoped>`.
- Use the tokens that exist; do not invent names. Notably: surfaces are `--bg-primary`/`--bg-secondary`/`--surface-primary`/`--surface-secondary`/`--surface-tertiary`; text is `--text-primary`/`--text-secondary` (no `--muted`); lines are `--border-color` (no `--border`); radii `--radius-sm|md|lg`; shadows `--shadow-sm`/`--card-shadow`/`--card-shadow-hover`/`--modal-shadow`; severity chips `--sev-{critical,warn,success,info}-*`.
- Fonts are local only. The app CSP (`src-tauri/tauri.conf.json`) allows no remote style or font hosts, so bundle font files under `src/assets/fonts/` instead of linking a font CDN. `--font-ui` (Inter) is the default body/UI font; `--font-mono` (Cascadia Mono) for offsets, hashes, sectors, IDs, recovery keys, and pill/status labels; `--font-brand` (Quicksand) is brand/wordmark only. Only Quicksand is bundled: `--font-ui` and `--font-mono` are system stacks that fall back to the platform UI / CJK and monospace fonts. There is no serif font and no `--font-sans`.
- Build controls from the `ui-*` primitives in `src/styles/ui.css` instead of re-declaring button, input, badge, or modal chrome in scoped CSS: `ui-btn` (`primary` / `danger` / `ghost` / `link` / `sm` / `lg`), `ui-icon-btn`, `ui-input` / `ui-select` / `ui-textarea` / `ui-search`, `ui-check` / `ui-switch`, `ui-seg`, `ui-tabs`, `ui-badge`, `ui-card`, `ui-alert`, `ui-progress`, `ui-empty`, `ui-toolbar` / `ui-toolbar-actions`, `ui-footbar` / `ui-footbar-stat`, `ui-menu`, `ui-choice`, `ui-disclosure`. Keep scoped CSS for layout and genuinely component-specific visuals. Each primitive is a single-class selector with its modifiers and states inside `:where()`, so any scoped rule overrides it without `!important`; keep that property when adding one. Card = `--surface-secondary` + 1px `--border-color` + `var(--radius-lg)` + `var(--card-shadow)`.
- Dropdowns use the `UiSelect` component (`src/components/ui/UiSelect.vue`: `v-model` plus `options: { value, label, disabled? }[]`), never a native `<select>`, whose system popup ignores the theme. The trigger keeps the `ui-select` look and the width of the longest option; the list is a `ui-menu` in the top layer, teleported to `body`, so floating layers that close on an outside pointer or scroll must let events inside `.ui-select-list` through.
- No coloured stripe on the left edge of cards, list rows, table rows, toasts or panels (`border-left`, a `::before` bar or an `inset Npx 0 0` shadow alike). Show status with a `ui-badge`, an icon or a status banner, categories with a legend swatch, and selection with the tint plus primary text; selected table rows get the 1px frame from `data-table.css` (70% `--primary-color`, 3:1 on the canvas in every theme), and snapshot rows keep their tint when selected. Deleted rows use ordinary row backgrounds, hover and selection feedback, with red text, a struck-through name and a deletion badge. Neutral 1px dividers, tree guides and the Markdown blockquote rule are fine.
- Modals are `.ui-modal-overlay` > `.ui-modal` with `ui-modal-header` / `ui-modal-body` / `ui-modal-footer`, normally inside `<Transition name="ui-modal">`. The structured file viewers (PE / ELF / Mach-O / ar / GGUF) share `src/styles/report-viewer.css` and the SQLite / Realm browsers share `src/styles/db-browser.css`, both included through `<style scoped src>` like `data-table.css`.
- Filled controls (primary / danger buttons, switches) use `--primary-solid` / `--danger-solid` and their `-hover` tokens with `--accent-contrast` labels; `--primary-color` stays for text, icons, borders, focus rings, and tints. Keep those labels at 4.5:1 contrast or better in every theme. `--primary-color` and `--text-secondary` are text colors too: keep them at 4.5:1 on the surfaces, `--sidebar-bg` and `--sidebar-active-bg` in every theme (the softer brand tones live in gradients, glows and tints). Status bar labels use `--statusbar-fg`.
- Validation: mark the failing `ui-input` / `ui-select` / `ui-textarea` (or the input inside `ui-search`) with `aria-invalid="true"` and state the problem in a `ui-field-error` linked through `aria-describedby`; a toast alone is not field feedback. Font sizes are whole pixels; primitive spacing follows the 4px grid.
- Do not add new top-level chrome outside `src/AppShell.vue` (the main shell).
- Workspace pages carry no title bar: they open on a `ui-toolbar` (search and filters on the left, page actions in `ui-toolbar-actions` on the right), then the work surface, then a `ui-footbar` that holds the page context (current task, state) and counts. The sidebar already names the page.
- `lucide-vue-next` is the icon system (`import { Icon } from 'lucide-vue-next'`, `<Icon :size="16" />`). Do not add other icon libraries or emoji in the UI.
- The spin indicator is lucide `Loader2` with the `.spin` class (exempt from the reduced-motion freeze in `base.css`; with motion off it turns at half speed); `AsyncPanelLoading.vue` covers lazy panels. Prefer skeletons or segmented progress for long tasks.
- Motion is switchable (设置 → 界面 → 界面动画: 跟随系统 / 开启 / 关闭). `useUiMotion.ts` resolves the preference and the OS reduced-motion setting into `data-motion="full" | "reduced"` on `<html>` before mount; that attribute is the only switch, so never query `prefers-reduced-motion` in CSS (it would defeat an explicit 开启). With `reduced`, `base.css` settles every transition and animation instantly (spinners excepted); give an indeterminate bar a static `:root[data-motion='reduced']` fallback so it does not freeze mid-slide.
- Timing comes from tokens: `--transition-fast` (0.15s ease) / `--transition-normal` (0.25s ease) for plain state changes, and `--motion-fast|normal|slow` (150 / 250 / 400ms) with `--ease-out` (entrances), `--ease-in` (exits) and `--ease-spring` (small pops) for motion. Entrances take the longer duration, exits the shorter.
- Decorative motion lives under `:root[data-motion='full']` (use `:where(:root[data-motion='full'])` in the global sheets to keep single-class specificity) and reuses the keyframes in `motion.css` (`ui-rise-in`, `ui-pop-in`, `ui-zoom-in`, `ui-bounce-in`, `ui-grow-x|y`, `ui-bump`, idle `ui-float` / `ui-twinkle` / `ui-breathe`). They animate the individual `translate` / `scale` / `rotate` properties, declare only the start state and run with `backwards` fill, so they compose with a component's own `transform`. `ui-menu`, `ui-alert` and `ui-empty` animate in on their own; add `ui-stagger` to short, non-virtualized lists and card grids (never virtual lists: recycled or keyed-moved rows restart the entrance) and `ui-bump` plus a value `:key` to counters. Idle loops are for illustrations and drop targets only.
- JS-driven motion goes through `animateUi` / `motionTiming` (`useUiMotion.ts`), `UiTransition` (`collapse` / `horizontal` / `fade` / `pop`), `v-content-motion` (light fade; `.rise` for page and tab switches) and `v-disclosure-motion`; each settles immediately with motion off. The theme switch passes the clicked control to `setTheme(theme, from)` for the circular view-transition reveal.
- UI copy uses `tr()` from `composables/useLocale.ts`, with Chinese source messages and English translations in `src/i18n/en.json`. Context-specific keys also need `src/i18n/zh-CN.json` entries. Use full-message interpolation; keep option labels reactive. Pure presentation models accept an optional `Translator` defaulting to `sourceText`, without importing locale state. Translate application labels only; preserve evidence names/paths/content, plugin-provided fields, persisted tag values, filter values and IPC enums. Run `just i18n-check` after copy changes; it is part of `verify-fe`.

After frontend changes, always run `just build-fe` (or at least `just typecheck` while iterating). Use `just test-fe` whenever behavior or state logic changes.

## Testing Guidelines

Rust tests are colocated in `#[cfg(test)]` modules. Use focused tests for parser behavior, filesystem traversal, indexing, search, timeline, and command-level changes. Prefer whole-object `assert_eq!` comparisons instead of field-by-field assertions. Use `pretty_assertions::assert_eq` in tests when available. Avoid mutating process environment in tests; pass dependencies or flags from the caller instead.

The frontend uses Vitest with colocated `*.test.ts` files (e.g. `src/composables/*.test.ts`, `src/utils/*.test.ts`). Run it through `just test-fe`; do not invoke Vitest directly. Also run `just typecheck` and `just build-fe` for frontend work.

Use this minimum local verification matrix:

- Frontend-only changes: `just typecheck`, `just test-fe`, and `just build-fe`; structural changes also need `just frontend-structure-check` and `just ipc-check`.
- Rust-only changes: `just fmt`, `just check`, `just clippy`, and `just test`; module moves or public reader-interface changes also need `just check-examples`.
- IPC changes: all relevant frontend and Rust checks above, plus `just ipc-check`.
- Plugin workspace changes (`plugins/`): `just fmt`, `just clippy-plugins`, `just test-plugins`; SDK changes also need the Rust checks above, since the host depends on the SDK.
- Vendored reader changes: `just fmt` plus the corresponding `just test-*` recipe, or `just test-vendor <directory>` when none exists; host integration changes also need Rust checks. Use reader lint recipes where available.
- CLI command/workflow changes: relevant Rust checks plus `just test-cli`, or `just smoke-cli <executable>` when verifying an already-built binary.
- Documentation-only changes: check relative links, referenced paths, recipe names and descriptions against the current implementation; application builds are not required.

The `verify-fe`, `verify-rust` and `verify-plugins` recipes collect the corresponding checks; they do not replace reader, example or CLI checks required by the changed scope. Use minimal synthetic fixtures and independent known outputs for parser, addressing and corruption regressions. Keep public fixture provenance, licenses and hashes; never commit real evidence, credentials, local case/cache/export directories or machine-specific paths. Distinguish source/API comparisons from actually executed oracle tests, and report any skipped or unavailable verification accurately.

## Logging and Output

Keep CLI/user output separate from diagnostics. Use `println!` or `eprintln!` only for requested data, copyable/actionable content, hard top-level errors, or interactive prompts. Everything else belongs in `tracing`.

Default behavior should be quiet on success. The desktop app's default filter is `warn`; set `RUST_LOG` (for example `RUST_LOG=lovelycarve_lib=info`) to see more. Lifecycle messages are `tracing::info!`, recoverable fallbacks are `tracing::warn!`, and module-level detail is `tracing::debug!`. Do not print success confirmations when the exit code already communicates success.

## Changelog and Documentation

Update `CHANGELOG.md` under `[Unreleased]` for meaningful user-visible features, fixes, breaking changes, deprecations, or removals. Follow [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/): use English change categories, ISO release dates, reverse chronological version sections, and version/comparison links. Keep entries concise and distinguish released-tag content from later changes.

Update `docs/` when changing user-facing behavior, configuration, architecture, commands, or workflow assumptions. Project documentation is written in Chinese; keep each topic in its owner document:

- User-visible workflows and UI behavior: `docs/USAGE.md`; the feature list, format table and screenshots in the root `README.md` when a headline feature changes.
- CLI commands, JSONL sessions and command-line unlock/export: `docs/CLI.md` (also shipped as the README of the CLI package).
- Format capabilities, deliberate differences from reference libraries, limitations and verification boundaries: the matching `docs/formats/<FORMAT>.md`; shared reader rules in `docs/formats/README.md`.
- Internals, invariants, storage layout and the feature-to-command map: `docs/ARCHITECTURE.md`; frontend conventions: `docs/FRONTEND.md` and `docs/FRONTEND_STRUCTURE.md`; build and tooling: `docs/DEVELOPMENT.md`; plugin ABI/LPP: `docs/PLUGIN_PROTOCOL.md`.

Write documentation as a description of the current state, not a work log. Do not record per-session results such as "本次/本轮" narratives, test pass counts, local toolchain failures, absolute local paths, or names and hashes of private evidence; put what you actually ran in the pull request description instead. Public fixtures keep their source, license and hash. Use placeholders such as `<镜像路径>` in example commands. Long per-function reference-library tables belong inside a collapsible `<details>` block. Screenshots in `docs/images/` use the light theme, a 1600×1000 viewport at 1.5× scale, and only public, synthetic or explicitly permitted evidence. When moving or renaming a document, update every reference, including vendored `LOVELYCARVE.md` / `UPSTREAM.md` notes, code comments, packaging scripts (`scripts/pack-cli.ps1` ships `docs/CLI.md`, `scripts/pack-plugins.ps1` ships `docs/LINUXINFO.md`) and the plugin submodule, which links to `docs/PLUGIN_PROTOCOL.md` and `docs/LINUXINFO.md` on `main`.

## Commit and Pull Request Guidelines

Recent history uses Conventional Commits with optional scopes, for example `feat(index): ...`, `fix(fstree): ...`, `perf(index): ...`, and `chore: ...`. Keep subjects imperative and specific.

Pull requests should describe user-visible behavior, list the local `just` recipes used for verification, link related issues, and include screenshots or recordings for UI changes. Claim hosted CI coverage only when the relevant workflow run has succeeded.
