# apfs — LovelyCarve maintained reader

Pure Rust, read-only APFS reader based on Dil4rd/dpp 0.4.1 (MIT), with
LovelyCarve correctness and forensic metadata patches. See
[LOVELYCARVE.md](LOVELYCARVE.md) for provenance and API changes and
[APFS capability audit](../../docs/formats/APFS.md) for the complete
libfsapfs comparison and verification limits.

- Multiple volumes with original slot, UUID, group, role and transaction identity.
- Committed checkpoint / transaction-bounded OMAP lookups and tombstones.
- Both directory key layouts, UTF-8 names and volume-specific name matching.
- File metadata, OID access, shared private-ID clone streams and symlink targets.
- Checked full, streamed and seekable extent reads; explicit sparse extents.
- Embedded / stream xattrs with independent metadata, extents and seekable reads.
- Transparent decmpfs 1/3/4/7/8/9/10/11/12 through the shared cmpfs crate.
- Snapshot metadata enumeration (names, XIDs, times, original superblock blocks).

FileVault/keybag unlocking, historical snapshot browsing, Fusion, APFS v1,
T2 encryption, sealed volumes and decmpfs LZBitmap are unsupported. Compressed
files are decoded into memory before reading/seeking; uncompressed files and
stream-backed attributes use direct extents. Missing tail extents, overlaps,
invalid arithmetic and allocated source overruns fail explicitly.

```rust
use apfs::ApfsVolume;
use std::io::{BufReader, Read, Seek, SeekFrom};

let source = BufReader::new(std::fs::File::open("container.raw")?);
let mut volume = ApfsVolume::open_index(source, 0)?;
let entries = volume.list_directory("/")?;
let mut file = volume.open_file("/example.bin")?;
file.seek(SeekFrom::Start(4096))?;
let mut bytes = [0u8; 32];
file.read_exact(&mut bytes)?;
```

From the repository root: `just test-apfs`, `just clippy-apfs`,
`just test-vendor cmpfs`. Tests use invented deterministic structures;
real-image oracle coverage is not implied. MIT license text remains in LICENSE.
