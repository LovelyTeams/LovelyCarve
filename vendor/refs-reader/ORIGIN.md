# Source and read-only port

The on-disk parsing algorithms are ported from Erik Larsson's **refsprogs**,
with the LovelyCarve named-stream / snapshot additions from b3nguang:

- Repository: https://github.com/b3nguang/refsprogs
- Branch: `feature/build-librefs-dll`
- Revision: `391c1bd56986142fc359288b7d2cb41cb3eb3680`
- Source: `include/refs/layout.h`, `librefs/node.c`, `librefs/volume.c`, `librefs/fsapi.c`
- Upstream copyright: Erik Larsson, 2022–2026 (see individual upstream files).
- License: GPL version 2 or later; see LICENSE.

This crate is LovelyCarve's production read-only ReFS backend. It implements
bounded boot / superblock / checkpoint parsing, fragmented metadata pages,
virtual block maps, object tables, filename / hardlink records, attributes,
inline / external / indexed attribute lists, resident data, inline / indexed
extent maps, named streams, stream snapshots, reparse targets and CoW shared
extents. Range and streaming reads share the data plan, reject short source
reads and preserve explicit unwritten / sparse spans as zeros. A bounded
per-volume directory cache avoids repeated path descents; optional caller
checks allow cancellation between metadata reads without a runtime dependency.
The reader is standard-library-only and forbids unsafe code.

Local corrections include physical checkpoint addressing and the 3.14 stored
root-array offset (including SHA-256 layouts), written/hole extent flags,
classic ADS descriptor routing, MACB timestamp ordering / filename time retention,
and version-specific shared-block resolution.
On-disk facts about WRITTEN (0x10), HOLE (0x20), stream-summary flags and
CoW holders were checked against xbqt/forefst's structure analysis at revision
`d4d5fd28942eb14ebe11d90f4eb9342f5e3d7b92` and three public ReFS 3.14 images.
That independent Python implementation was executed as an oracle under the
ignored fixture directory; no Python implementation was copied into this crate.

The application-facing read-only port is complete. This is not a port of C
write stubs, diagnostic printers or a compatible DLL ABI. See the repository's
`docs/formats/REFS.md` for reproducible fixture provenance and coverage.
The public image coverage is ReFS 3.14 with 4 KiB and 64 KiB clusters; earlier
layout branches are retained but are not claimed to be image-validated.
Metadata/data CRC or SHA authentication, damaged-volume salvage, deleted-file
recovery and journal replay are outside this port's validated behavior.
