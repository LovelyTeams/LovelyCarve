# LovelyCarve QCOW reader

## Origin and licensing

Original vendored crate: Dave Vasilevsky's MIT-licensed
[vasi/qcow2-rs, 3ed3ff55b96a2f519216ace7b3a02ac29bf8ba65](https://github.com/vasi/qcow2-rs/commit/3ed3ff55b96a2f519216ace7b3a02ac29bf8ba65).
The retained feature-name parser, feature formatting, integer helpers and error
infrastructure preserve LICENSE-MIT. Original LovelyCarve implementations and
new tests are AGPL-3.0-only; see LICENSE-AGPL. The crate declares both licenses.

The 2026-10-13 capability audit uses all **42** public declarations in
[libyal/libqcow, 3e390fbb465f37f3da92c1362c5df3da04ff9efe](https://github.com/libyal/libqcow/commit/3e390fbb465f37f3da92c1362c5df3da04ff9efe)
and QEMU's public QCOW format specification as references. No libqcow C code,
documentation text, fixtures, bindings or binaries are incorporated. libqcow is
LGPL-3.0-or-later; its format document is GFDL-1.3-or-later.

## Local changes

- Bounded QCOW1/2/3 headers, version-specific table geometry, modern v3 header
  lengths and v2 extensions; reserved descriptor bits and checked source ranges.
- Read-only raw DEFLATE cluster decoding using pure Rust flate2/miniz_oxide.
  Validates complete streams and exact decoded cluster length. Compressed sector
  padding at EOF may be absent; missing DEFLATE bytes still fail.
- Explicit backing-source attachment and automatic relative RAW/QCOW chains,
  capped at 32 files with canonical-path cycle checks. Declared format overrides
  probing; missing backing files never silently become sparse bytes.
- Legacy AES-128-CBC sector reads with ASCII/UTF-16 ASCII passwords or exact
  16-byte keys. Keys and temporary decrypted buffers zero on drop. The format
  has no password verifier; wrong keys can produce unauthenticated plaintext.
- Logical zero clusters override backing data; v2 reserved zero bits fail.
- Independent Read/Seek cursors, positioned access, metadata, signature probing,
  locked-state inspection and operation-scoped cancellation predicates.
- L1 cached once (32 MiB maximum), 4096 L2 descriptor cache entries and eight
  decoded clusters (at most 16 MiB retained per image).
- Host opens chains and compressed disks automatically; image_load exposes
  transient qcowPassword / qcowKeyHex for CLI and typed IPC consumers.

## Limits and verification

See [QCOW format guide](../../../docs/formats/QCOW.md) for the complete API table,
integration boundary, read rules and unsupported features. Clusters are limited
to 512 bytes–2 MiB, L1 to 32 MiB, and seek/media positions to signed 64-bit.
QCOW1 L2 bits are bounded to 1–21. No repair/write, historical snapshot view,
QCOW LUKS payload, external data file, extended L2 or Zstd decoding.

`just test-qcow2`, `just clippy-qcow2`, `just test qcow --lib --jobs 1`.
Invented independent Python zlib/OpenSSL fixtures, hashes and regeneration
instructions are in tests/data. No real QCOW evidence or compiled libqcow
differential oracle was supplied; these are not claimed as verified.
