# LovelyCarve integration

ext4-view 1.0.0 from crates.io, source 8facbf4c2986b7e2dbdef92514d696f1559a2b90 (MIT OR Apache-2.0).

Preserved local inode-index APIs (block_size, read_raw_inode, open_inode_by_index, read_dir_inode, DirEntry::inode_index), 128-byte inode metadata-checksum handling and tolerant read-only legacy journal compatibility. Unit-test fixture decompression uses Rust zstd rather than a system command; paths are relative to CARGO_MANIFEST_DIR. See test_data/README.md for fixture hashes.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).

The 2026-10-12 libfsext capability audit adds checked inode/external/EA-inode
attributes, complete inline file and directory reads, legacy directories without
FILETYPE, revision-0 inode geometry, meta_bg descriptor placement, signed extended
timestamps, volume/special-inode metadata APIs, unwritten extent handling, legacy
GDT CRC16 and 64 KiB directory record lengths. Missing indirect subtrees are holes.
The application layer also maps all indirect levels and rejects allocated reads
outside the evidence volume or short reads. No libfsext code or dependency is used.

See [the capability matrix](../../docs/formats/EXT.md) for API/desktop distinctions and
remaining limitations. `just test-vendor ext4-view` runs the local regressions;
the generated fixture's provenance and hash are in test_data/README.md.
