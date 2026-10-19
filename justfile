# LovelyCarve development tasks. Run `just` or `just --list` for the catalog.
# Configuration and prerequisites: docs/DEVELOPMENT.md.
set windows-shell := ["pwsh", "-NoProfile", "-Command"]

# Override with environment variables or `just name=value <recipe>`.
# An empty VS_PATH lets vswhere find the latest C++ tools installation.
vs_path := env_var_or_default("VS_PATH", "")
openssl_dir := env_var_or_default("OPENSSL_DIR", "C:\\Program Files\\OpenSSL-Win64")
openssl_lib := env_var_or_default("OPENSSL_LIB", openssl_dir + "\\lib\\VC\\x64\\MT")
perl_bin := env_var_or_default("PERL_BIN", "")
opencv_dir := env_var_or_default("OPENCV_DIR", justfile_directory() + "\\opencv-contrib-install")
opencv_lib := env_var_or_default("OPENCV_LIB", "opencv_world4130")
opencv_msvc := env_var_or_default("OPENCV_MSVC", "vc18")
opencv_src := env_var_or_default("OPENCV_SRC", justfile_directory() + "\\opencv\\sources")
opencv_contrib := env_var_or_default("OPENCV_CONTRIB", justfile_directory() + "\\opencv_contrib-4.x\\opencv_contrib-4.x\\modules")
opencv_install := env_var_or_default("OPENCV_INSTALL", opencv_dir)

# Dot-source the environment in the same shell as the native command.
opensslenv := ". '" + justfile_directory() + "\\scripts\\windows-env.ps1' -OpenSSLDir '" + openssl_dir + "' -OpenSSLLib '" + openssl_lib + "' -PerlBin '" + perl_bin + "'"
windowsenv := opensslenv + " -VisualStudio -VsPath '" + vs_path + "'"
vsenv := windowsenv + " -OpenCV -OpenCVDir '" + opencv_dir + "' -OpenCVLib '" + opencv_lib + "' -OpenCVMsvc '" + opencv_msvc + "'"
cargo_env := if os() == "windows" { opensslenv + "; " } else { "" }
python := if os() == "windows" { "python" } else { "python3" }
cli_executable := if os() == "windows" { "src-tauri/target/debug/lovelycarve-cli.exe" } else { "src-tauri/target/debug/lovelycarve-cli" }

[private]
default:
    @just --list --unsorted

# Initialize the pinned plugin submodule and install frontend dependencies.
[group("setup")]
bootstrap:
    git submodule update --init --recursive
    bun install --frozen-lockfile

# Run the complete frontend checks required for a contribution.
[group("verify")]
verify-fe: ipc-check frontend-structure-check i18n-check typecheck test-fe build-fe

# Run the backend checks required for a contribution.
[group("verify")]
verify-rust: fmt check clippy test

# Run the SDK, process host and linuxinfo checks.
[group("verify")]
verify-plugins: fmt clippy-plugins test-plugins

# Start Vite after preparing offline viewer assets.
[group("frontend")]
dev-fe:
    bun scripts/copy-file-viewer-assets.mjs
    bun run dev

# Type-check Vue and TypeScript.
[group("frontend")]
typecheck:
    bun run typecheck

# Run frontend unit tests.
[group("frontend")]
test-fe:
    bunx vitest run

# Type-check and build the frontend bundle.
[group("frontend")]
build-fe:
    bun run build

# Check the typed frontend IPC boundary.
[group("frontend")]
ipc-check:
    bun run check:ipc

# Check frontend module ownership and explicit cross-feature contracts.
[group("frontend")]
frontend-structure-check:
    bun scripts/check-frontend-structure.mjs

# Check translations, interpolation parameters and untranslated template text.
[group("frontend")]
i18n-check:
    bun scripts/check-i18n.mjs

# Upgrade both viewer packages and refresh offline assets.
[group("frontend")]
update-file-viewer version *args:
    bun update --exact @file-viewer/vue3-full@{{ version }} @file-viewer/vite-plugin@{{ version }} {{ args }}
    bun scripts/copy-file-viewer-assets.mjs

# Refresh frontend dependencies within the reviewed manifest constraints.
[group("frontend")]
update-fe:
    bun update
    bun scripts/copy-file-viewer-assets.mjs

# Format the backend, the plugin workspace and all maintained reader crates.
[group("backend")]
fmt:
    cd src-tauri && cargo fmt
    cd plugins && cargo fmt
    cd src-tauri/vendor/qcow2 && cargo fmt
    cd src-tauri/vendor/vdi && cargo fmt
    cd src-tauri/vendor/ewf && cargo fmt
    cd src-tauri/vendor/ntfs && cargo fmt
    cd src-tauri/vendor/vmdk && cargo fmt
    cd src-tauri/vendor/bitlocker && cargo fmt
    cd src-tauri/vendor/vhd && cargo fmt
    cd src-tauri/vendor/vhdx && cargo fmt
    cd src-tauri/vendor/luks && cargo fmt
    cd src-tauri/vendor/veracrypt && cargo fmt
    cd src-tauri/vendor/vsc && cargo fmt
    cd src-tauri/vendor/gzipf && cargo fmt
    cd src-tauri/vendor/smraw && cargo fmt
    cd src-tauri/vendor/phdi && cargo fmt
    cd src-tauri/vendor/refs-reader && cargo fmt
    cd src-tauri/vendor/sqlite-core && cargo fmt
    cd src-tauri/vendor/ad1 && cargo fmt
    cd src-tauri/vendor/ios-backup && cargo fmt
    cd src-tauri/vendor/apfs && cargo fmt
    cd src-tauri/vendor/hfsplus && cargo fmt
    cd src-tauri/vendor/filevault && cargo fmt
    cd src-tauri/vendor/cmpfs && cargo fmt
    cd src-tauri/vendor/udif && cargo fmt
    cd src-tauri/vendor/ext4-view && cargo fmt
    cd src-tauri/vendor/backhand && cargo fmt
    cd src-tauri/vendor/rsqlite-vfs && cargo fmt
    cd src-tauri/vendor/rufs && cargo fmt
    cd src-tauri/vendor/xfs-fuse && cargo fmt
    cd src-tauri/vendor/fatfs && cargo fmt
    cd src-tauri/vendor/exfat && cargo fmt
    cd src-tauri/vendor/gpt && cargo fmt
    cd src-tauri/vendor/notatin && cargo fmt
    cd src-tauri/vendor/magika && cargo fmt

# Refresh backend dependencies within the reviewed manifest constraints.
[group("backend")]
update-rust:
    cargo update --manifest-path src-tauri/Cargo.toml

# Compile-check the standalone CLI backend and the desktop build, without QR/OCR.
[group("backend")]
[working-directory("src-tauri")]
check:
    {{ cargo_env }}cargo check --no-default-features
    {{ cargo_env }}cargo check --no-default-features --features desktop

# Lint the standalone CLI backend and the desktop build, without QR/OCR.
[group("backend")]
[working-directory("src-tauri")]
clippy:
    {{ cargo_env }}cargo clippy --no-default-features
    {{ cargo_env }}cargo clippy --no-default-features --features desktop

# Run backend tests; optionally pass a test filter and Cargo arguments.
[group("backend")]
[working-directory("src-tauri")]
test *args:
    {{ cargo_env }}cargo test --no-default-features {{ args }}

# Build lc-plugin-host and stage every plugin package (library and schema.json); pass --release for bundles.
[group("plugins")]
build-plugins *args:
    {{ cargo_env }}bun scripts/build-plugins.mjs {{ args }}

# Package every workspace plugin as an installable .lcpkg.
[group("plugins")]
[windows]
pack-plugins: (build-plugins "--release")
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack-plugins.ps1'

# Test the SDK, lc-plugin-host and the plugins.
[group("plugins")]
test-plugins *args:
    cargo test --workspace --manifest-path plugins/Cargo.toml {{ args }}

# Lint the plugin workspace.
[group("plugins")]
clippy-plugins:
    cargo clippy --workspace --manifest-path plugins/Cargo.toml --all-targets

# Refresh the reviewed plugin workspace dependency lockfile.
[group("plugins")]
update-plugins:
    cargo update --manifest-path plugins/Cargo.toml

# Generate platform icons and both frontend brand variants from the source images.
[group("desktop")]
icons:
    bun scripts/generate-icons.mjs

# Refresh the Tauri v2 JS packages and Rust dependency family together.
[group("desktop")]
update-tauri:
    bun update --exact @tauri-apps/api @tauri-apps/cli @tauri-apps/plugin-dialog @tauri-apps/plugin-fs
    cargo update --manifest-path src-tauri/Cargo.toml -p tauri -p tauri-build -p tauri-codegen -p tauri-macros -p tauri-plugin -p tauri-runtime -p tauri-runtime-wry -p tauri-utils -p tauri-plugin-dialog -p tauri-plugin-fs

# Run the Tauri desktop app without QR/OCR (Windows).
[group("desktop")]
[windows]
dev:
    bun scripts/copy-file-viewer-assets.mjs
    {{ cargo_env }}bun scripts/build-plugins.mjs
    {{ windowsenv }}; bun run tauri dev

# Run the Tauri desktop app with WeChat QR and CUDA OCR (Windows).
[group("desktop")]
[windows]
dev-full:
    bun scripts/copy-file-viewer-assets.mjs
    {{ cargo_env }}bun scripts/build-plugins.mjs
    {{ vsenv }}; bun run tauri dev --features qrcode-wechat,ocr-cuda --config '{{ justfile_directory() }}\src-tauri\tauri.full.conf.json'

# Build the desktop backend without QR/OCR (Windows).
[group("desktop")]
[windows]
[working-directory("src-tauri")]
build:
    {{ windowsenv }}; cargo build --no-default-features --features desktop

# Build the desktop backend with WeChat QR and CUDA OCR (Windows).
[group("desktop")]
[windows]
[working-directory("src-tauri")]
build-full:
    {{ vsenv }}; $env:TAURI_CONFIG=Get-Content -Raw -LiteralPath '{{ justfile_directory() }}\src-tauri\tauri.full.conf.json'; cargo build --no-default-features --features 'desktop qrcode-wechat ocr-cuda'

# Build the desktop release bundle without QR/OCR (Windows).
[group("desktop")]
[windows]
build-release *args:
    {{ cargo_env }}bun scripts/build-plugins.mjs --release
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\prune-bundle-dlls.ps1'
    {{ windowsenv }}; bun run tauri build --config '{{ justfile_directory() }}\src-tauri\tauri.plugins.conf.json' {{ args }}

# Build the desktop release bundle with QR/OCR, models and plugins (Windows).
[group("desktop")]
[windows]
build-release-full *args:
    {{ cargo_env }}bun scripts/build-plugins.mjs --release
    {{ vsenv }}; bun run tauri build --features qrcode-wechat,ocr-cuda --config '{{ justfile_directory() }}\src-tauri\tauri.plugins.conf.json' --config '{{ justfile_directory() }}\src-tauri\tauri.full.conf.json' {{ args }}

# Build and package the portable Windows desktop app.
[group("desktop")]
[windows]
pack: build-release
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1'

# Build and package the portable Windows desktop app with QR/OCR.
[group("desktop")]
[windows]
pack-full: build-release-full
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1' -QrOcr

# Package the existing desktop release without rebuilding.
[group("desktop")]
[windows]
repack:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1'

# Package an existing desktop release with QR/OCR resources.
[group("desktop")]
[windows]
repack-full:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1' -QrOcr

# Run the CLI without QR/OCR.
[group("cli")]
[working-directory("src-tauri")]
run-cli *args:
    @{{ cargo_env }}cargo run --no-default-features --bin lovelycarve-cli -- {{ args }}

# Build the standalone release CLI (no Tauri) without QR/OCR.
[group("cli")]
[windows]
[working-directory("src-tauri")]
build-cli:
    {{ windowsenv }}; cargo build --jobs 1 --no-default-features --release --bin lovelycarve-cli

# Build the standalone release CLI (no Tauri) without QR/OCR.
[group("cli")]
[unix]
[working-directory("src-tauri")]
build-cli:
    cargo build --no-default-features --release --bin lovelycarve-cli

# Build the standalone release CLI with WeChat QR and CUDA OCR.
[group("cli")]
[windows]
[working-directory("src-tauri")]
build-cli-full:
    {{ vsenv }}; cargo build --jobs 1 --no-default-features --features 'qrcode-wechat ocr-cuda' --release --bin lovelycarve-cli

# Build the standalone release CLI with WeChat QR and CPU OCR.
[group("cli")]
[unix]
[working-directory("src-tauri")]
build-cli-full:
    cargo build --no-default-features --features 'qrcode-wechat ocr' --release --bin lovelycarve-cli

# Build the development CLI and run end-to-end smoke tests.
[group("cli")]
test-cli:
    {{ cargo_env }}cd src-tauri && cargo build --no-default-features --bin lovelycarve-cli
    {{ python }} scripts/test-cli.py {{ cli_executable }}

# Smoke-test an existing CLI executable.
[group("cli")]
smoke-cli executable:
    {{ python }} scripts/test-cli.py '{{ executable }}'

# Build and package the Windows CLI without QR/OCR.
[group("cli")]
[windows]
pack-cli: build-cli
    {{ cargo_env }}bun scripts/build-plugins.mjs --release
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack-cli.ps1'

# Build and package the full Windows CLI with QR/OCR and plugins.
[group("cli")]
[windows]
pack-cli-full: build-cli-full
    {{ cargo_env }}bun scripts/build-plugins.mjs --release
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack-cli.ps1' -RequireQrOcr

# Package an existing Windows CLI release using its actual capabilities.
[group("cli")]
[windows]
repack-cli:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack-cli.ps1'

# Test the read-only Rust ReFS port independently of the desktop host.
[group("readers")]
[working-directory("src-tauri/vendor/refs-reader")]
test-refs *args:
    cargo test {{ args }}

[group("readers")]
[working-directory("src-tauri")]
test-refs-oracle *args:
    @{{ cargo_env }}cargo test --no-default-features --features refs-dll-oracle --example probe_refs_rust {{ args }}

# Check the Rust ReFS parser without desktop dependencies.
[group("readers")]
[working-directory("src-tauri/vendor/refs-reader")]
clippy-refs:
    cargo clippy --all-targets

# Run the library tests of a vendored crate without a dedicated reader recipe.
[group("readers")]
test-vendor name *args:
    cargo test --manifest-path src-tauri/vendor/{{ name }}/Cargo.toml --lib {{ args }}

# Test the standalone single/split RAW reader and new-output writer.
[group("readers")]
[working-directory("src-tauri/vendor/smraw")]
test-smraw *args:
    cargo test {{ args }}

[group("readers")]
[working-directory("src-tauri/vendor/smraw")]
clippy-smraw *args:
    cargo clippy --all-targets {{ args }}

# Test the standalone pure Rust GZIP member reader and its independent corpus.
[group("readers")]
[working-directory("src-tauri/vendor/gzipf")]
test-gzipf *args:
    cargo test {{ args }}

# Lint the GZIP reader without desktop dependencies.
[group("readers")]
[working-directory("src-tauri/vendor/gzipf")]
clippy-gzipf *args:
    cargo clippy --all-targets {{ args }}

# Test the FileVault core and forensic auditor, including optional libfvde oracle.
[group("readers")]
[working-directory("src-tauri/vendor/filevault")]
test-filevault *args:
    cargo test --workspace {{ args }}

[group("readers")]
[working-directory("src-tauri/vendor/filevault")]
clippy-filevault:
    cargo clippy --workspace --all-targets

[group("readers")]
[working-directory("src-tauri/vendor/hfsplus")]
clippy-hfsplus:
    cargo clippy --all-targets

# Test the APFS reader and deterministic transaction/stream fixtures.
[group("readers")]
[working-directory("src-tauri/vendor/apfs")]
test-apfs *args:
    cargo test --lib {{ args }}

# Lint the standalone APFS reader without desktop dependencies.
[group("readers")]
[working-directory("src-tauri/vendor/apfs")]
clippy-apfs:
    cargo clippy --all-targets

# Test the EWF reader and corpus regressions.
[group("readers")]
[working-directory("src-tauri/vendor/ewf")]
test-ewf *args:
    cargo test {{ args }}

# Test VMDK with full encodings, tracing and fixture helpers.
[group("readers")]
[working-directory("src-tauri/vendor/vmdk")]
test-vmdk *args:
    cargo test --features full-encoding,trace,test-helpers {{ args }}

# Test the VHD reader and image fixtures.
[group("readers")]
[working-directory("src-tauri/vendor/vhd")]
test-vhd *args:
    cargo test {{ args }}

# Test the VHDX reader and image fixtures.
[group("readers")]
[working-directory("src-tauri/vendor/vhdx")]
test-vhdx *args:
    cargo test {{ args }}

# Test the QCOW 1/2/3 reader and independent fixtures.
[group("readers")]
[working-directory("src-tauri/vendor/qcow2")]
test-qcow2 *args:
    cargo test {{ args }}

# Lint the QCOW reader and every test target.
[group("readers")]
[working-directory("src-tauri/vendor/qcow2")]
clippy-qcow2:
    cargo clippy --all-targets -- -D warnings

# Regenerate invented QCOW fixtures (Python cryptography required).
[group("readers")]
generate-qcow-fixtures:
    {{ python }} src-tauri/vendor/qcow2/tests/data/generate.py

# Test the standalone Parallels reader and malformed-image regressions.
[group("readers")]
[working-directory("src-tauri/vendor/phdi")]
test-phdi *args:
    cargo test {{ args }}

# Lint the Parallels reader and every test target.
[group("readers")]
[working-directory("src-tauri/vendor/phdi")]
clippy-phdi:
    cargo clippy --all-targets -- -D warnings

# Test the VDI reader.
[group("readers")]
[working-directory("src-tauri/vendor/vdi")]
test-vdi:
    cargo test

# Test the BitLocker reader and decryptor.
[group("readers")]
[working-directory("src-tauri/vendor/bitlocker")]
test-bitlocker *args:
    cargo test {{ args }}

# Lint the BitLocker reader, including its independent corpus tests.
[group("readers")]
[working-directory("src-tauri/vendor/bitlocker")]
clippy-bitlocker *args:
    cargo clippy --all-targets {{ args }}

# Test the Volume Shadow Copy reader; set VSC_ORACLE_IMAGE to dfVFS vss.raw for the pyvshadow oracle.
[group("readers")]
[working-directory("src-tauri/vendor/vsc")]
test-vsc *args:
    cargo test {{ args }}

# Lint the standalone Volume Shadow Copy reader and its probes.
[group("readers")]
[working-directory("src-tauri/vendor/vsc")]
clippy-vsc:
    cargo clippy --all-targets -- -D warnings

# Inspect a raw NTFS volume's VSS header, stores and raw block/range metadata.
[group("readers")]
[working-directory("src-tauri/vendor/vsc")]
probe-vsc *args:
    cargo run --example inspect -- {{ args }}

# Compare generated pyvshadow whole-volume and range dumps (see tests/oracle/README.md).
[group("readers")]
[working-directory("src-tauri/vendor/vsc")]
compare-vsc directory count:
    cargo run --release --example differential -- {{ directory }} {{ count }}

# Test the SQLite record carver; its sqlite3 oracle tests need sqlite3 on PATH or SQLITE3_BIN.
[group("readers")]
[working-directory("src-tauri/vendor/sqlite-core")]
test-sqlite-core *args:
    cargo test {{ args }}

# Test the AD1 logical-image reader; set AD1_USERBSS to a real FTK image for the stored-hash oracle.
[group("readers")]
[working-directory("src-tauri/vendor/ad1")]
test-ad1 *args:
    cargo test {{ args }}

# Test the iOS backup reader, auditor and independent encrypted fixtures.
[group("readers")]
[working-directory("src-tauri/vendor/ios-backup")]
test-ios-backup *args:
    cargo test {{ args }}

# Test the LUKS reader.
[group("readers")]
[working-directory("src-tauri/vendor/luks")]
test-luks *args:
    cargo test {{ args }}

# Lint the LUKS reader and its independent corpus tests.
[group("readers")]
[working-directory("src-tauri/vendor/luks")]
clippy-luks *args:
    cargo clippy --all-targets {{ args }}

# Test the VeraCrypt reader.
[group("readers")]
[working-directory("src-tauri/vendor/veracrypt")]
test-veracrypt *args:
    cargo test {{ args }}

# Compare the Rust ReFS port against the explicitly enabled legacy DLL oracle.
[group("tools")]
[working-directory("src-tauri")]
probe-refs *args:
    @{{ cargo_env }}cargo run --no-default-features --features refs-dll-oracle --example probe_refs_rust -- {{ args }}

# Run a backend example without QR/OCR.
[group("tools")]
[working-directory("src-tauri")]
run-example name *args:
    @{{ cargo_env }}cargo run --no-default-features --example {{ name }} -- {{ args }}

# Compile all explicitly registered probes and benchmarks without QR/OCR.
[group("tools")]
[working-directory("src-tauri")]
check-examples:
    {{ cargo_env }}cargo check --no-default-features --examples

# Run an example with release-fast and one Cargo build job.
[group("tools")]
[working-directory("src-tauri")]
run-example-fast name *args:
    @{{ cargo_env }}cargo run --jobs 1 --no-default-features --profile release-fast --example {{ name }} -- {{ args }}

# Benchmark the filedb index with release-fast.
[group("tools")]
[working-directory("src-tauri")]
bench-filedb-store entries="1000000" *args:
    @{{ cargo_env }}cargo run --jobs 1 --no-default-features --profile release-fast --example benchmark_filedb_store -- --entries {{ entries }} {{ args }}

# Show the resolved Visual Studio and OpenCV paths.
[group("opencv")]
qr-env:
    @echo "VS_PATH        = {{ vs_path }}"
    @echo "OPENCV_DIR     = {{ opencv_dir }}"
    @echo "OPENCV_LIB     = {{ opencv_lib }}"
    @echo "OPENCV_MSVC    = {{ opencv_msvc }}"
    @echo "OPENCV_SRC     = {{ opencv_src }}"
    @echo "OPENCV_CONTRIB = {{ opencv_contrib }}"
    @echo "OPENCV_INSTALL = {{ opencv_install }}"

# Build contrib OpenCV from source into OPENCV_INSTALL.
[group("opencv")]
[windows]
build-opencv-contrib:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\build_opencv_contrib.ps1' -VsPath '{{ vs_path }}' -Src '{{ opencv_src }}' -Contrib '{{ opencv_contrib }}' -Install '{{ opencv_install }}'

# Decode a QR image with the WeChat detector.
[group("opencv")]
[windows]
[working-directory("src-tauri")]
qr-test image count="1":
    {{ vsenv }}; cargo run --no-default-features --features qrcode-wechat --example qr_test -- '{{ image }}' {{ count }}
