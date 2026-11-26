# LovelyCarve integration

rufs 0.6.0 source audited at realchonk/fuse-ufs 36bc35a5fb9e05e214fa3d9619822763d7d8cb9b (BSD-2-Clause).

Portable read-only fork; FUSE services, Unix-specific block device operations and write helpers are outside the shipped adapter. Synchronized serializer maintenance (bincode-next 3.1.1 and MSRV 1.90) while preserving portable I/O and inode decoding. Fixed-width little/big-endian disk layout is tested against literal bytes.

The UFS1 read path follows the 4.4BSD dinode and cylinder-group layouts as
published in FreeBSD and OpenBSD source headers. It widens 32-bit inode pointers
into the common read model and leaves UFS1 mutation unavailable. Full and
ranged reads share logical-byte validation. Native references and regeneration
are recorded in the host tests/fixtures/ufs1 directory.

Upstream audit and verification commands: [../UPSTREAM.md](../UPSTREAM.md).
