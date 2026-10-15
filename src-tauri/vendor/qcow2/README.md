# qcow2 — LovelyCarve read-only fork

Reads QCOW version 1 and QCOW2 versions 2/3 as logical disks. Supports normal,
sparse and zero clusters, raw DEFLATE, RAW/QCOW backing chains and legacy
AES-128-CBC sector encryption. Positioned reads and independent Read/Seek
cursors share bounded per-image caches.

```rust
use positioned_io::ReadAt;
use qcow2::Qcow2;

let image = Qcow2::open_path("image.qcow2")?;
let reader = image.reader()?;
let mut bytes = [0; 4096];
reader.read_exact_at(1024 * 1024, &mut bytes)?;
```

`open_path` resolves relative backing files; `open(io)` leaves any declared
backing source to `set_backing`. `open_with_key` / `open_path_with_key` accept
`EncryptionKey::from_password` (ASCII), `from_utf16_password` or `from_bytes`.
The legacy format has no password verifier. `info()` and `is_locked()` permit
inspection without a key. `read_at_cancellable` accepts an operation-scoped
predicate; cancellation never becomes a successful short read.

No QCOW LUKS payload, Zstd, external data file, extended L2, historical snapshot
selection, writing, refcount repair or compaction. Only snapshot count is exposed.
See [LOVELYCARVE.md](LOVELYCARVE.md) and [the full 42-API comparison](../../../docs/formats/QCOW.md)
for limits, provenance, MIT / AGPL-3.0-only licensing and validation boundaries.

From the repository root: `just test-qcow2`, `just clippy-qcow2` and
`just test qcow --lib --jobs 1`. New independent fixtures and hashes live in
tests/data; the original MIT qcow2-rs tests/test.qcow2 fixture is retained.
