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
cargo_env := if os() == "windows" { windowsenv + "; " } else { "" }
python := if os() == "windows" { "python" } else { "python3" }
python_version := env_var_or_default("LOVELYCARVE_PYTHON", "3.15")
# OpenSSL's Windows libcrypto.lib is an import library even inside MT; Python
# wheels use the static library names so consumers need no OpenSSL DLL on PATH.
python_openssl_libs := env_var_or_default("OPENSSL_LIBS", "libssl_static:libcrypto_static")
python_openssl_lib := env_var_or_default("OPENSSL_LIB", openssl_dir + "\\lib\\VC\\x64\\MD")
python_cargo_env := if os() == "windows" { cargo_env + "$env:OPENSSL_LIB_DIR='" + python_openssl_lib + "'; $env:OPENSSL_LIBS='" + python_openssl_libs + "'; " } else { "" }
cli_executable := if os() == "windows" { "target/debug/lovelycarve-cli.exe" } else { "target/debug/lovelycarve-cli" }
# The engines, backend and CLI build together. The desktop app builds on its
# own: GPUI turns on serde_json's `preserve_order`, which must not reach the
# CLI's JSON output (see the workspace Cargo.toml).
backend_packages := "-p lovelycarve-core -p lovelycarve-backend -p lovelycarve-cli"

[private]
default:
    @just --list --unsorted

# Initialize the pinned plugin submodule.
[group("setup")]
bootstrap:
    git submodule update --init --recursive

# Run the engine, backend, CLI and desktop checks required for a contribution.
[group("verify")]
verify-rust: fmt check clippy test test-desktop

# Run the SDK, process host and linuxinfo checks.
[group("verify")]
verify-plugins: fmt clippy-plugins test-plugins

# Install the read-only Python package into python/.venv using uv's interpreter.
[group("python")]
develop-python:
    {{ python_cargo_env }}uv sync --project python --python {{ python_version }}

# Build an installable Python wheel (reader-only engines, no QR/OCR).
[group("python")]
build-python:
    {{ python_cargo_env }}uv build --project python --python {{ python_version }} --wheel --out-dir dist-pack/python

# Verify the installed Python package against synthetic disk and archive fixtures.
[group("python")]
test-python: develop-python
    uv run --project python --no-sync python -m unittest discover -s python/tests -v

# Format only the Python binding's Rust crate.
[group("python")]
fmt-python:
    cargo fmt --manifest-path python/Cargo.toml

# Lint the Python binding without changing the workspace build.
[group("python")]
clippy-python *args:
    {{ python_cargo_env }}cargo clippy --manifest-path python/Cargo.toml {{ args }} -- -D warnings

# Format the workspace crates, the plugin workspace and all maintained reader crates.
[group("backend")]
fmt:
    cargo fmt
    cd plugins && cargo fmt
    cd vendor/qcow2 && cargo fmt
    cd vendor/vdi && cargo fmt
    cd vendor/ewf && cargo fmt
    cd vendor/ntfs && cargo fmt
    cd vendor/vmdk && cargo fmt
    cd vendor/bitlocker && cargo fmt
    cd vendor/vhd && cargo fmt
    cd vendor/vhdx && cargo fmt
    cd vendor/luks && cargo fmt
    cd vendor/veracrypt && cargo fmt
    cd vendor/vsc && cargo fmt
    cd vendor/gzipf && cargo fmt
    cd vendor/smraw && cargo fmt
    cd vendor/phdi && cargo fmt
    cd vendor/refs-reader && cargo fmt
    cd vendor/sqlite-core && cargo fmt
    cd vendor/ad1 && cargo fmt
    cd vendor/ios-backup && cargo fmt
    cd vendor/apfs && cargo fmt
    cd vendor/hfsplus && cargo fmt
    cd vendor/filevault && cargo fmt
    cd vendor/cmpfs && cargo fmt
    cd vendor/udif && cargo fmt
    cd vendor/ext4-view && cargo fmt
    cd vendor/lamfold && cargo fmt
    cd vendor/lamfold-cramfs && cargo fmt
    cd vendor/lamfold-romfs && cargo fmt
    cd vendor/lamfold-udf && cargo fmt
    cd vendor/minix-reader && cargo fmt
    cd vendor/classic-hfs && cargo fmt
    cd vendor/jfs-reader && cargo fmt
    cd vendor/reiserfs-reader && cargo fmt
    cd vendor/amiga-reader && cargo fmt
    cd vendor/befs-reader && cargo fmt
    cd vendor/gfs2-reader && cargo fmt
    cd vendor/ocfs2-reader && cargo fmt
    cd vendor/nilfs2-reader && cargo fmt
    cd vendor/bcachefs-reader && cargo fmt
    cd vendor/rust-fs-bcachefs && cargo fmt
    cd vendor/rust-fs-core && cargo fmt
    cd vendor/amiga-ffs && cargo fmt
    cd vendor/amiga-rdb && cargo fmt
    cd vendor/lamfold-erofs && cargo fmt
    cd vendor/f2fs-reader && cargo fmt
    cd vendor/qnx-reader && cargo fmt
    cd vendor/minifs && cargo fmt
    cd vendor/backhand && cargo fmt
    cd vendor/lzma-rs && cargo fmt
    cd vendor/sercomm-firmware && cargo fmt
    cd vendor/ubidump && cargo fmt
    cd vendor/jffs2 && cargo fmt
    cd vendor/yaffs2-reader && cargo fmt
    cd vendor/rsqlite-vfs && cargo fmt
    cd vendor/rufs && cargo fmt
    cd vendor/xfs-fuse && cargo fmt
    cd vendor/fatfs && cargo fmt
    cd vendor/exfat && cargo fmt
    cd vendor/gpt && cargo fmt
    cd vendor/notatin && cargo fmt
    cd vendor/magika && cargo fmt

# Refresh workspace dependencies within the reviewed manifest constraints.
[group("backend")]
update-rust:
    cargo update

# Compile-check the engines, backend and CLI, then the desktop app, without QR/OCR.
[group("backend")]
check *args:
    {{ cargo_env }}cargo check {{ backend_packages }} {{ args }}
    {{ cargo_env }}cargo check -p lovelycarve-desktop {{ args }}

# Lint the engines, backend and CLI, then the desktop app, without QR/OCR.
[group("backend")]
clippy:
    {{ cargo_env }}cargo clippy {{ backend_packages }}
    {{ cargo_env }}cargo clippy -p lovelycarve-desktop

# Run the engine, backend and CLI tests; optionally pass a test filter and Cargo arguments.
[group("backend")]
test *args:
    {{ cargo_env }}cargo test {{ backend_packages }} {{ args }}

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
    {{ cargo_env }}cargo test --workspace --manifest-path plugins/Cargo.toml {{ args }}

# Lint the plugin workspace.
[group("plugins")]
clippy-plugins:
    {{ cargo_env }}cargo clippy --workspace --manifest-path plugins/Cargo.toml --all-targets

# Refresh the reviewed plugin workspace dependency lockfile.
[group("plugins")]
update-plugins:
    cargo update --manifest-path plugins/Cargo.toml

# Build the plugins and run the desktop app (development build without QR/OCR); program arguments follow `--`.
[group("desktop")]
dev *args:
    {{ cargo_env }}bun scripts/build-plugins.mjs
    {{ cargo_env }}cargo run -p lovelycarve-desktop {{ args }}

# Build the desktop app (development build); pass --release for an optimised build.
[group("desktop")]
build *args:
    {{ cargo_env }}cargo build -p lovelycarve-desktop {{ args }}

# Compile-check only the desktop app.
[group("desktop")]
check-desktop *args:
    {{ cargo_env }}cargo check -p lovelycarve-desktop {{ args }}

# Run the desktop app's unit tests.
[group("desktop")]
test-desktop *args:
    {{ cargo_env }}cargo test -p lovelycarve-desktop {{ args }}

# Build the release plugins and the release desktop app (Windows).
[group("desktop")]
[windows]
build-release *args:
    {{ cargo_env }}bun scripts/build-plugins.mjs --release
    {{ windowsenv }}; cargo build -p lovelycarve-desktop --release {{ args }}

# Build and package the portable Windows desktop app; pass -NoUpx to skip UPX.
[group("desktop")]
[windows]
pack *args: build-release
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1' {{ args }}

# Package the existing desktop release without rebuilding; pass -NoUpx to skip UPX.
[group("desktop")]
[windows]
repack *args:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack.ps1' {{ args }}

# Run the CLI without QR/OCR.
[group("cli")]
run-cli *args:
    @{{ cargo_env }}cargo run -p lovelycarve-cli -- {{ args }}

# Build the standalone release CLI without QR/OCR.
[group("cli")]
[windows]
build-cli:
    {{ windowsenv }}; cargo build -p lovelycarve-cli --release

# Build the standalone release CLI without QR/OCR.
[group("cli")]
[unix]
build-cli:
    cargo build -p lovelycarve-cli --release

# Build the standalone release CLI with WeChat QR and CUDA OCR.
[group("cli")]
[windows]
build-cli-full:
    {{ vsenv }}; cargo build -p lovelycarve-cli --features 'qrcode-wechat ocr-cuda' --release

# Build the standalone release CLI with WeChat QR and CPU OCR.
[group("cli")]
[unix]
build-cli-full:
    cargo build -p lovelycarve-cli --features 'qrcode-wechat ocr' --release

# Build the development CLI and run end-to-end smoke tests.
[group("cli")]
test-cli:
    {{ cargo_env }}cargo build -p lovelycarve-cli
    {{ python }} scripts/test-cli.py {{ cli_executable }}

# Smoke-test an existing CLI executable.
[group("cli")]
smoke-cli executable:
    {{ python }} scripts/test-cli.py '{{ executable }}'

# Test the structured DSH tool and its CLI transport using synthetic evidence.
[group("cli")]
test-dsh:
    node --test integrations/dsh-lovelycarve/test/*.test.js

# Regenerate the agent skill's command reference from the CLI command catalogue.
[group("cli")]
update-cli-skill:
    {{ cargo_env }}cargo test -p lovelycarve-cli --lib skill_tests::update_command_reference -- --ignored --exact

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

# Stage the existing Windows CLI release and runtime beside the agent skill.
[group("cli")]
[windows]
stage-cli-skill:
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\pack-cli.ps1' -StageSkill

# Refresh the DSH forensic preset, skill and existing Windows release runtime.
[group("cli")]
[windows]
stage-dsh: stage-cli-skill
    pwsh -NoProfile -File '{{ justfile_directory() }}\scripts\stage-dsh.ps1'

# Test the read-only Rust ReFS port independently of the workspace.
[group("readers")]
[working-directory("vendor/refs-reader")]
test-refs *args:
    cargo test {{ args }}

# Run the ReFS probe tests against the explicitly enabled legacy DLL oracle.
[group("readers")]
test-refs-oracle *args:
    @{{ cargo_env }}cargo test -p lovelycarve-core --features refs-dll-oracle --example probe_refs_rust {{ args }}

# Check the Rust ReFS parser without workspace dependencies.
[group("readers")]
[working-directory("vendor/refs-reader")]
clippy-refs:
    cargo clippy --all-targets

# Lint the standalone pure Rust JFFS2 reader.
[group("readers")]
[working-directory("vendor/jffs2")]
clippy-jffs2:
    cargo clippy --all-targets -- -D warnings

# Lint the maintained SquashFS reader and enabled legacy variants.
[group("readers")]
[working-directory("vendor/backhand")]
clippy-squashfs:
    cargo clippy --all-targets --features v1_v2,v3_ddwrt,v3_lzma

# Lint the bounded Sercomm ZIP/NAND reader.
[group("readers")]
[working-directory("vendor/sercomm-firmware")]
clippy-sercomm:
    cargo clippy --all-targets -- -D warnings

[group("readers")]
[working-directory("vendor/yaffs2-reader")]
clippy-yaffs2:
    cargo clippy --all-targets -- -D warnings

# Run the library tests of a vendored crate without a dedicated reader recipe.
[group("readers")]
test-vendor name *args:
    cargo test --manifest-path vendor/{{ name }}/Cargo.toml --lib {{ args }}

# Validate the Minix V1/V2/V3 reader, including util-linux reference images.
[group("readers")]
test-minix:
    cargo test --manifest-path vendor/minix-reader/Cargo.toml --lib

# Validate the classic HFS reader (data/resource forks and B-tree corruption).
[group("readers")]
test-classic-hfs:
    cargo test --manifest-path vendor/classic-hfs/Cargo.toml --lib

[group("readers")]
clippy-classic-hfs:
    cargo clippy --manifest-path vendor/classic-hfs/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-hfs-fixtures:
    {{ python }} scripts/hfs-fixtures.py

[group("readers")]
test-jfs:
    cargo test --manifest-path vendor/jfs-reader/Cargo.toml --lib

# Pure Rust ReiserFS 3.5/3.6 reader and independent fixtures.
[group("readers")]
test-reiserfs:
    cargo test --manifest-path vendor/reiserfs-reader/Cargo.toml --lib

[group("readers")]
test-amiga:
    cargo test --manifest-path vendor/amiga-reader/Cargo.toml --lib

[group("readers")]
test-befs *args:
    cargo test --manifest-path vendor/befs-reader/Cargo.toml --lib {{ args }}

[group("readers")]
test-gfs2 *args:
    cargo test --manifest-path vendor/gfs2-reader/Cargo.toml --lib {{ args }}

[group("readers")]
clippy-gfs2:
    cargo clippy --manifest-path vendor/gfs2-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-gfs2-fixtures:
    {{ python }} scripts/gfs2-fixtures.py

[group("readers")]
test-gfs2-native:
    cargo test --manifest-path vendor/gfs2-reader/Cargo.toml --lib native_linux -- --ignored --nocapture

[group("readers")]
test-ocfs2 *args:
    cargo test --manifest-path vendor/ocfs2-reader/Cargo.toml --lib {{ args }}

[group("readers")]
clippy-ocfs2:
    cargo clippy --manifest-path vendor/ocfs2-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-ocfs2-fixtures:
    {{ python }} scripts/ocfs2-fixtures.py

# Audit external mkfs.ocfs2 / Linux / fsck inputs; requires OCFS2_ORACLE_DIR.
[group("readers")]
test-ocfs2-native:
    cargo test --manifest-path vendor/ocfs2-reader/Cargo.toml --lib native_linux -- --ignored --nocapture

[group("readers")]
test-nilfs2 *args:
    cargo test --manifest-path vendor/nilfs2-reader/Cargo.toml --lib {{ args }}

[group("readers")]
test-bcachefs *args:
    cargo test --manifest-path vendor/rust-fs-bcachefs/Cargo.toml --lib {{ args }}

[group("readers")]
clippy-bcachefs:
    cargo clippy --manifest-path vendor/rust-fs-bcachefs/Cargo.toml --all-targets -- -D warnings

[group("readers")]
test-bcachefs-reader *args:
    cargo test --manifest-path vendor/bcachefs-reader/Cargo.toml --lib {{ args }}

[group("readers")]
generate-bcachefs-fixtures:
    {{ python }} scripts/bcachefs-fixtures.py

[group("readers")]
clippy-bcachefs-reader:
    cargo clippy --manifest-path vendor/bcachefs-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
test-bcachefs-native:
    cargo test --manifest-path vendor/bcachefs-reader/Cargo.toml --lib native_linux -- --ignored --nocapture

[group("readers")]
clippy-nilfs2:
    cargo clippy --manifest-path vendor/nilfs2-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
test-nilfs2-native:
    cargo test --manifest-path vendor/nilfs2-reader/Cargo.toml --lib native_linux -- --ignored --nocapture

[group("readers")]
generate-nilfs2-fixtures:
    {{ python }} scripts/nilfs2-fixtures.py

[group("readers")]
clippy-befs:
    cargo clippy --manifest-path vendor/befs-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-befs-fixtures:
    {{ python }} scripts/befs-fixtures.py

[group("readers")]
test-befs-native *args:
    {{ python }} scripts/befs-native-oracle.py {{ args }}

[group("readers")]
test-amiga-ffs:
    cargo test --manifest-path vendor/amiga-ffs/Cargo.toml --lib

[group("readers")]
test-amiga-rdb:
    cargo test --manifest-path vendor/amiga-rdb/Cargo.toml --lib

[group("readers")]
clippy-amiga:
    cargo clippy --manifest-path vendor/amiga-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-amiga-fixtures:
    {{ python }} scripts/amiga-fixtures.py

[group("readers")]
clippy-reiserfs:
    cargo clippy --manifest-path vendor/reiserfs-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-reiserfs-fixtures:
    {{ python }} scripts/reiserfs-fixtures.py

[group("readers")]
clippy-jfs:
    cargo clippy --manifest-path vendor/jfs-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-jfs-fixtures:
    {{ python }} scripts/jfs-fixtures.py

[group("readers")]
clippy-minix:
    cargo clippy --manifest-path vendor/minix-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-minix-fixtures:
    {{ python }} scripts/minix-fixtures.py

# Validate/lint the UDF physical-partition reader and its reference fixtures.
[group("readers")]
test-udf:
    cargo test --manifest-path vendor/lamfold-udf/Cargo.toml --lib

[group("readers")]
clippy-udf:
    cargo clippy --manifest-path vendor/lamfold-udf/Cargo.toml --all-targets -- -D warnings
    cargo check --manifest-path vendor/lamfold-udf/Cargo.toml --no-default-features

[group("readers")]
generate-udf-fixtures:
    {{ python }} scripts/udf-fixtures.py

# Lint the standalone MINIFS reader, including regression tests.
[group("readers")]
clippy-minifs:
    cargo clippy --manifest-path vendor/minifs/Cargo.toml --all-targets -- -D warnings

# Validate the standalone flash filesystem readers.
[group("readers")]
test-qnx:
    cargo test --manifest-path vendor/qnx-reader/Cargo.toml --lib

[group("readers")]
clippy-qnx:
    cargo clippy --manifest-path vendor/qnx-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-qnx-fixtures:
    {{ python }} scripts/qnx-fixtures.py

[group("readers")]
test-qnx-reference oracle:
    {{ python }} scripts/qnx-reference.py '{{ oracle }}'

[group("readers")]
test-erofs:
    cargo test --manifest-path vendor/lamfold-erofs/Cargo.toml --lib --features deflate,zstd,lzma

[group("readers")]
clippy-erofs:
    cargo clippy --manifest-path vendor/lamfold-erofs/Cargo.toml --all-targets --features deflate,zstd,lzma -- -D warnings

[group("readers")]
clippy-f2fs:
    cargo clippy --manifest-path vendor/f2fs-reader/Cargo.toml --all-targets -- -D warnings

[group("readers")]
generate-f2fs-fixtures:
    {{ python }} scripts/f2fs-fixtures.py

# Generate independent MINIFS fixtures, or a Python oracle for external firmware.
[group("readers")]
generate-minifs-fixtures *args:
    {{ python }} scripts/minifs-fixtures.py {{ args }}

# Lint the adapted read-only UBI library and its synthetic tests.
[group("readers")]
[working-directory("vendor/ubidump")]
clippy-ubi:
    cargo clippy --all-targets -- -D warnings

# Test the standalone single/split RAW reader and new-output writer.
[group("readers")]
[working-directory("vendor/smraw")]
test-smraw *args:
    cargo test {{ args }}

# Lint the standalone single/split RAW reader and every test target.
[group("readers")]
[working-directory("vendor/smraw")]
clippy-smraw *args:
    cargo clippy --all-targets {{ args }}

# Test the standalone pure Rust GZIP member reader and its independent corpus.
[group("readers")]
[working-directory("vendor/gzipf")]
test-gzipf *args:
    cargo test {{ args }}

# Lint the GZIP reader without workspace dependencies.
[group("readers")]
[working-directory("vendor/gzipf")]
clippy-gzipf *args:
    cargo clippy --all-targets {{ args }}

# Test the FileVault core and forensic auditor, including optional libfvde oracle.
[group("readers")]
[working-directory("vendor/filevault")]
test-filevault *args:
    cargo test --workspace {{ args }}

# Lint the FileVault core, forensic auditor and every test target.
[group("readers")]
[working-directory("vendor/filevault")]
clippy-filevault:
    cargo clippy --workspace --all-targets

# Lint the standalone HFS+ / HFSX reader and every test target.
[group("readers")]
[working-directory("vendor/hfsplus")]
clippy-hfsplus:
    cargo clippy --all-targets

# Test the APFS reader and deterministic transaction/stream fixtures.
[group("readers")]
[working-directory("vendor/apfs")]
test-apfs *args:
    cargo test --lib {{ args }}

# Lint the standalone APFS reader without workspace dependencies.
[group("readers")]
[working-directory("vendor/apfs")]
clippy-apfs:
    cargo clippy --all-targets

# Test the EWF reader and corpus regressions.
[group("readers")]
[working-directory("vendor/ewf")]
test-ewf *args:
    cargo test {{ args }}

# Test VMDK with full encodings, tracing and fixture helpers.
[group("readers")]
[working-directory("vendor/vmdk")]
test-vmdk *args:
    cargo test --features full-encoding,trace,test-helpers {{ args }}

# Test the VHD reader and image fixtures.
[group("readers")]
[working-directory("vendor/vhd")]
test-vhd *args:
    cargo test {{ args }}

# Test the VHDX reader and image fixtures.
[group("readers")]
[working-directory("vendor/vhdx")]
test-vhdx *args:
    cargo test {{ args }}

# Test the QCOW 1/2/3 reader and independent fixtures.
[group("readers")]
[working-directory("vendor/qcow2")]
test-qcow2 *args:
    cargo test {{ args }}

# Lint the QCOW reader and every test target.
[group("readers")]
[working-directory("vendor/qcow2")]
clippy-qcow2:
    cargo clippy --all-targets -- -D warnings

# Regenerate invented QCOW fixtures (Python cryptography required).
[group("readers")]
generate-qcow-fixtures:
    {{ python }} vendor/qcow2/tests/data/generate.py

# Test the standalone Parallels reader and malformed-image regressions.
[group("readers")]
[working-directory("vendor/phdi")]
test-phdi *args:
    cargo test {{ args }}

# Lint the Parallels reader and every test target.
[group("readers")]
[working-directory("vendor/phdi")]
clippy-phdi:
    cargo clippy --all-targets -- -D warnings

# Test the VDI reader.
[group("readers")]
[working-directory("vendor/vdi")]
test-vdi:
    cargo test

# Test the BitLocker reader and decryptor.
[group("readers")]
[working-directory("vendor/bitlocker")]
test-bitlocker *args:
    cargo test {{ args }}

# Lint the BitLocker reader, including its independent corpus tests.
[group("readers")]
[working-directory("vendor/bitlocker")]
clippy-bitlocker *args:
    cargo clippy --all-targets {{ args }}

# Test the Volume Shadow Copy reader; set VSC_ORACLE_IMAGE to dfVFS vss.raw for the pyvshadow oracle.
[group("readers")]
[working-directory("vendor/vsc")]
test-vsc *args:
    cargo test {{ args }}

# Lint the standalone Volume Shadow Copy reader and its probes.
[group("readers")]
[working-directory("vendor/vsc")]
clippy-vsc:
    cargo clippy --all-targets -- -D warnings

# Inspect a raw NTFS volume's VSS header, stores and raw block/range metadata.
[group("readers")]
[working-directory("vendor/vsc")]
probe-vsc *args:
    cargo run --example inspect -- {{ args }}

# Compare generated pyvshadow whole-volume and range dumps (see tests/oracle/README.md).
[group("readers")]
[working-directory("vendor/vsc")]
compare-vsc directory count:
    cargo run --release --example differential -- {{ directory }} {{ count }}

# Test the SQLite record carver; its sqlite3 oracle tests need sqlite3 on PATH or SQLITE3_BIN.
[group("readers")]
[working-directory("vendor/sqlite-core")]
test-sqlite-core *args:
    cargo test {{ args }}

# Test the AD1 logical-image reader; set AD1_USERBSS to a real FTK image for the stored-hash oracle.
[group("readers")]
[working-directory("vendor/ad1")]
test-ad1 *args:
    cargo test {{ args }}

# Test the iOS backup reader, auditor and independent encrypted fixtures.
[group("readers")]
[working-directory("vendor/ios-backup")]
test-ios-backup *args:
    cargo test {{ args }}

# Test the LUKS reader.
[group("readers")]
[working-directory("vendor/luks")]
test-luks *args:
    cargo test {{ args }}

# Lint the LUKS reader and its independent corpus tests.
[group("readers")]
[working-directory("vendor/luks")]
clippy-luks *args:
    cargo clippy --all-targets {{ args }}

# Test the VeraCrypt reader.
[group("readers")]
[working-directory("vendor/veracrypt")]
test-veracrypt *args:
    cargo test {{ args }}

# Compare the Rust ReFS port against the explicitly enabled legacy DLL oracle.
[group("tools")]
probe-refs *args:
    @{{ cargo_env }}cargo run -p lovelycarve-core --features refs-dll-oracle --example probe_refs_rust -- {{ args }}

# Run an engine example without QR/OCR.
[group("tools")]
run-example name *args:
    @{{ cargo_env }}cargo run -p lovelycarve-core --example {{ name }} -- {{ args }}

# Compile all explicitly registered probes and benchmarks without QR/OCR.
[group("tools")]
check-examples:
    {{ cargo_env }}cargo check -p lovelycarve-core --examples

# Run an example with release-fast and one Cargo build job.
[group("tools")]
run-example-fast name *args:
    @{{ cargo_env }}cargo run --jobs 1 -p lovelycarve-core --profile release-fast --example {{ name }} -- {{ args }}

# Benchmark the filedb index with release-fast.
[group("tools")]
bench-filedb-store entries="1000000" *args:
    @{{ cargo_env }}cargo run --jobs 1 -p lovelycarve-core --profile release-fast --example benchmark_filedb_store -- --entries {{ entries }} {{ args }}

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
qr-test image count="1":
    {{ vsenv }}; cargo run -p lovelycarve-core --features qrcode-wechat --example qr_test -- '{{ image }}' {{ count }}
